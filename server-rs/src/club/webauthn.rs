//! WebAuthn / passkey registration and login (webauthn-rs).

use crate::club::{
    auth::{new_id, new_session_id},
    db::{now_ms, ClubDb, DbError, PilotRow},
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use webauthn_rs::prelude::*;

const CHALLENGE_TTL: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct WebauthnState {
    webauthn: Arc<Webauthn>,
    pending: Arc<Mutex<HashMap<String, PendingChallenge>>>,
}

#[derive(Clone)]
enum PendingChallenge {
    Register {
        pilot_id: String,
        reg: PasskeyRegistration,
        created: Instant,
    },
    Login {
        auth: PasskeyAuthentication,
        created: Instant,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebauthnRouteError {
    InvalidSession,
    InvalidPayload,
    NotFound,
    Verification,
    ClubUnavailable,
}

impl WebauthnState {
    pub fn from_env() -> Option<Self> {
        let rp_id = std::env::var("WEBAUTHN_RP_ID")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                std::env::var("FLIGHT_CLUB_PUBLIC_URL")
                    .ok()
                    .and_then(|url| url::Url::parse(&url).ok())
                    .and_then(|u| u.host_str().map(String::from))
            })?;
        let origin =
            std::env::var("FLIGHT_CLUB_PUBLIC_URL").unwrap_or_else(|_| format!("https://{rp_id}"));
        let rp_origin: Url = origin.parse().ok()?;
        let webauthn = WebauthnBuilder::new(&rp_id, &rp_origin)
            .ok()?
            .rp_name("flight_vis")
            .build()
            .ok()?;
        Some(Self {
            webauthn: Arc::new(webauthn),
            pending: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn sweep(&self) {
        let mut map = self.pending.lock().unwrap();
        let now = Instant::now();
        map.retain(|_, v| match v {
            PendingChallenge::Register { created, .. }
            | PendingChallenge::Login { created, .. } => {
                now.duration_since(*created) < CHALLENGE_TTL
            }
        });
    }

    pub fn register_begin(
        &self,
        pilot: &PilotRow,
    ) -> Result<(serde_json::Value, String), WebauthnRouteError> {
        let digest = Sha256::digest(pilot.id.as_bytes());
        let bytes: [u8; 16] = digest[..16]
            .try_into()
            .map_err(|_| WebauthnRouteError::InvalidPayload)?;
        let user_unique_id = Uuid::from_bytes(bytes);
        let (ccr, reg_state) = self
            .webauthn
            .start_passkey_registration(user_unique_id, &pilot.email, &pilot.name, None)
            .map_err(|_| WebauthnRouteError::Verification)?;
        let nonce = new_session_id();
        self.sweep();
        self.pending.lock().unwrap().insert(
            nonce.clone(),
            PendingChallenge::Register {
                pilot_id: pilot.id.clone(),
                reg: reg_state,
                created: Instant::now(),
            },
        );
        let json = serde_json::to_value(ccr).map_err(|_| WebauthnRouteError::InvalidPayload)?;
        Ok((json, nonce))
    }

    pub async fn register_finish(
        &self,
        db: &ClubDb,
        nonce: &str,
        credential: RegisterPublicKeyCredential,
    ) -> Result<(), WebauthnRouteError> {
        let pending = {
            let mut map = self.pending.lock().unwrap();
            map.remove(nonce)
        };
        let PendingChallenge::Register {
            pilot_id,
            reg,
            created,
            ..
        } = pending.ok_or(WebauthnRouteError::InvalidSession)?
        else {
            return Err(WebauthnRouteError::InvalidSession);
        };
        if created.elapsed() > CHALLENGE_TTL {
            return Err(WebauthnRouteError::InvalidSession);
        }
        let passkey = self
            .webauthn
            .finish_passkey_registration(&credential, &reg)
            .map_err(|_| WebauthnRouteError::Verification)?;
        store_credential(db, &pilot_id, &passkey)
            .await
            .map_err(|_| WebauthnRouteError::Verification)?;
        Ok(())
    }

    pub async fn login_begin(
        &self,
        db: &ClubDb,
        email: &str,
        club_id: &str,
    ) -> Result<(serde_json::Value, String), WebauthnRouteError> {
        let creds = load_credentials(db, club_id, email)
            .await
            .map_err(|_| WebauthnRouteError::NotFound)?;
        if creds.is_empty() {
            return Err(WebauthnRouteError::NotFound);
        }
        let (rcr, auth_state) = self
            .webauthn
            .start_passkey_authentication(&creds)
            .map_err(|_| WebauthnRouteError::Verification)?;
        let nonce = new_session_id();
        self.sweep();
        self.pending.lock().unwrap().insert(
            nonce.clone(),
            PendingChallenge::Login {
                auth: auth_state,
                created: Instant::now(),
            },
        );
        let json = serde_json::to_value(rcr).map_err(|_| WebauthnRouteError::InvalidPayload)?;
        Ok((json, nonce))
    }

    pub async fn login_finish(
        &self,
        db: &ClubDb,
        club_id: &str,
        nonce: &str,
        credential: PublicKeyCredential,
    ) -> Result<(String, PilotRow), WebauthnRouteError> {
        let pending = {
            let mut map = self.pending.lock().unwrap();
            map.remove(nonce)
        };
        let PendingChallenge::Login { auth, created, .. } =
            pending.ok_or(WebauthnRouteError::InvalidSession)?
        else {
            return Err(WebauthnRouteError::InvalidSession);
        };
        if created.elapsed() > CHALLENGE_TTL {
            return Err(WebauthnRouteError::InvalidSession);
        }
        let auth_result = self
            .webauthn
            .finish_passkey_authentication(&credential, &auth)
            .map_err(|_| WebauthnRouteError::Verification)?;
        let cred_id = auth_result.cred_id().clone();
        let row = sqlx::query(
            "SELECT w.id, w.pilot_id, w.counter, p.id, p.club_id, p.name, p.email, p.license_no, p.status, p.role, p.locale
             FROM webauthn_credentials w
             JOIN pilots p ON p.id = w.pilot_id
             WHERE p.club_id = $1 AND w.credential_id = $2",
        )
        .bind(club_id)
        .bind(cred_id.as_ref())
        .fetch_optional(&db.pool)
        .await
        .map_err(|_| WebauthnRouteError::Verification)?;
        let Some(row) = row else {
            return Err(WebauthnRouteError::NotFound);
        };
        let w_id: String = row
            .try_get(0)
            .map_err(|_| WebauthnRouteError::Verification)?;
        let counter: i64 = row
            .try_get(2)
            .map_err(|_| WebauthnRouteError::Verification)?;
        let new_counter = auth_result.counter() as i64;
        if new_counter > 0 && new_counter <= counter {
            return Err(WebauthnRouteError::Verification);
        }
        sqlx::query("UPDATE webauthn_credentials SET counter = $1 WHERE id = $2")
            .bind(new_counter)
            .bind(&w_id)
            .execute(&db.pool)
            .await
            .map_err(|_| WebauthnRouteError::Verification)?;
        let pilot = PilotRow {
            id: row
                .try_get(3)
                .map_err(|_| WebauthnRouteError::Verification)?,
            club_id: row
                .try_get(4)
                .map_err(|_| WebauthnRouteError::Verification)?,
            name: row
                .try_get(5)
                .map_err(|_| WebauthnRouteError::Verification)?,
            email: row
                .try_get(6)
                .map_err(|_| WebauthnRouteError::Verification)?,
            license_no: row
                .try_get(7)
                .map_err(|_| WebauthnRouteError::Verification)?,
            status: row
                .try_get(8)
                .map_err(|_| WebauthnRouteError::Verification)?,
            role: row
                .try_get(9)
                .map_err(|_| WebauthnRouteError::Verification)?,
            locale: row
                .try_get(10)
                .map_err(|_| WebauthnRouteError::Verification)?,
        };
        if db.club_operational(&pilot.club_id).await.is_err() {
            return Err(WebauthnRouteError::ClubUnavailable);
        }
        let now = now_ms();
        let session_id = db
            .create_pilot_session(&pilot.id, now)
            .await
            .map_err(|_| WebauthnRouteError::Verification)?;
        Ok((session_id, pilot))
    }
}

async fn store_credential(db: &ClubDb, pilot_id: &str, passkey: &Passkey) -> Result<(), DbError> {
    let id = new_id();
    let cred_id = passkey.cred_id().as_ref();
    let pk = serde_cbor::to_vec(passkey).map_err(|e| {
        DbError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        ))
    })?;
    sqlx::query(
        "INSERT INTO webauthn_credentials (id, pilot_id, credential_id, public_key, counter, created_at)
         VALUES ($1, $2, $3, $4, 0, $5)",
    )
    .bind(&id)
    .bind(pilot_id)
    .bind(cred_id)
    .bind(&pk)
    .bind(now_ms())
    .execute(&db.pool)
    .await?;
    Ok(())
}

async fn load_credentials(
    db: &ClubDb,
    club_id: &str,
    email: &str,
) -> Result<Vec<Passkey>, DbError> {
    let rows = sqlx::query(
        "SELECT w.public_key FROM webauthn_credentials w
         JOIN pilots p ON p.id = w.pilot_id
         WHERE p.club_id = $1 AND lower(p.email) = lower($2)",
    )
    .bind(club_id)
    .bind(email.trim())
    .fetch_all(&db.pool)
    .await?;
    let mut creds = Vec::new();
    for row in rows {
        let pk: Vec<u8> = row.try_get(0)?;
        let passkey: Passkey = serde_cbor::from_slice(&pk).map_err(|e| {
            DbError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            ))
        })?;
        creds.push(passkey);
    }
    Ok(creds)
}
