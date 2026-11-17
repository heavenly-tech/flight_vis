use crate::club::{
    auth::{digest_token, new_device_token, new_id},
    db::{now_ms, ClubDb},
};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

pub const MAX_DEVICES_PER_PILOT: usize = 10;
pub const MIN_INSTALL_ID_LEN: usize = 8;
pub const MAX_INSTALL_ID_LEN: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceRegisterError {
    InvalidInstallId,
    CapExceeded,
    Db,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRegisterResponse {
    pub id: String,
    pub token: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_id: Option<String>,
    pub created: bool,
}

impl ClubDb {
    pub fn count_devices_for_pilot(&self, pilot_id: &str) -> usize {
        self.with_conn(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM devices WHERE pilot_id = ?1",
                params![pilot_id],
                |r| r.get(0),
            )?;
            Ok(n as usize)
        })
        .unwrap_or(0)
    }

    /// App auto-registration: stable device row per `(pilot_id, install_id)`, fresh bearer token each call.
    pub fn register_device_for_install(
        &self,
        pilot_id: &str,
        label: &str,
        model: Option<&str>,
        install_id: &str,
    ) -> Result<DeviceRegisterResponse, DeviceRegisterError> {
        let install_id = install_id.trim();
        if install_id.len() < MIN_INSTALL_ID_LEN || install_id.len() > MAX_INSTALL_ID_LEN {
            return Err(DeviceRegisterError::InvalidInstallId);
        }
        let label = if label.trim().is_empty() {
            "Dispositivo"
        } else {
            label.trim()
        };
        let model = model.map(str::trim).filter(|s| !s.is_empty());
        let token = new_device_token();
        let hash = digest_token(&token);
        let now = now_ms();

        self.with_conn_any(|conn| -> Result<DeviceRegisterResponse, DeviceRegisterError> {
            if let Some(existing_id) = conn
                .query_row(
                    "SELECT id FROM devices WHERE pilot_id = ?1 AND install_id = ?2",
                    params![pilot_id, install_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(|_| DeviceRegisterError::Db)?
            {
                conn.execute(
                    "UPDATE devices SET token_hash = ?1, label = ?2, model = ?3 WHERE id = ?4",
                    params![hash, label, model, existing_id],
                )
                .map_err(|_| DeviceRegisterError::Db)?;
                conn.execute(
                    "DELETE FROM device_pair_tokens WHERE device_id = ?1",
                    params![existing_id],
                )
                .ok();
                return Ok(DeviceRegisterResponse {
                    id: existing_id,
                    token,
                    label: label.to_string(),
                    model: model.map(String::from),
                    install_id: Some(install_id.to_string()),
                    created: false,
                });
            }
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM devices WHERE pilot_id = ?1",
                    params![pilot_id],
                    |r| r.get(0),
                )
                .map_err(|_| DeviceRegisterError::Db)?;
            if count as usize >= MAX_DEVICES_PER_PILOT {
                return Err(DeviceRegisterError::CapExceeded);
            }
            let id = new_id();
            conn.execute(
                "INSERT INTO devices (id, pilot_id, token_hash, label, model, install_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, pilot_id, hash, label, model, install_id, now],
            )
            .map_err(|_| DeviceRegisterError::Db)?;
            Ok(DeviceRegisterResponse {
                id,
                token,
                label: label.to_string(),
                model: model.map(String::from),
                install_id: Some(install_id.to_string()),
                created: true,
            })
        })
    }

    fn end_active_flights_for_device(
        conn: &rusqlite::Connection,
        pilot_id: &str,
        device_id: &str,
        now: i64,
    ) -> rusqlite::Result<()> {
        let mut stmt = conn.prepare(
            "SELECT f.id FROM flights f
             INNER JOIN flight_devices fd ON fd.flight_id = f.id
             WHERE fd.device_id = ?1 AND f.pilot_id = ?2 AND f.status = 'active'",
        )?;
        let ids: Vec<String> = stmt
            .query_map(params![device_id, pilot_id], |r| r.get(0))?
            .filter_map(Result::ok)
            .collect();
        for flight_id in ids {
            conn.execute(
                "UPDATE flights SET status = 'ended', ended_at = ?1 WHERE id = ?2",
                params![now, flight_id],
            )?;
        }
        Ok(())
    }

    fn revoke_device_row(
        conn: &rusqlite::Connection,
        pilot_id: &str,
        device_id: &str,
    ) -> rusqlite::Result<bool> {
        let history: i64 = conn.query_row(
            "SELECT COUNT(*) FROM flight_devices WHERE device_id = ?1",
            params![device_id],
            |r| r.get(0),
        )?;
        if history == 0 {
            let deleted = conn.execute(
                "DELETE FROM devices WHERE pilot_id = ?1 AND id = ?2",
                params![pilot_id, device_id],
            )?;
            return Ok(deleted > 0);
        }
        let revoked_hash = digest_token(&format!("revoked-{}", new_id()));
        let updated = conn.execute(
            "UPDATE devices SET token_hash = ?1 WHERE pilot_id = ?2 AND id = ?3",
            params![revoked_hash, pilot_id, device_id],
        )?;
        conn.execute(
            "DELETE FROM device_pair_tokens WHERE device_id = ?1",
            params![device_id],
        )
        .ok();
        Ok(updated > 0)
    }

    pub fn revoke_device_by_install(
        &self,
        pilot_id: &str,
        install_id: &str,
    ) -> Result<bool, rusqlite::Error> {
        let install_id = install_id.trim();
        self.with_conn(|conn| {
            let device_id: Option<String> = conn
                .query_row(
                    "SELECT id FROM devices WHERE pilot_id = ?1 AND install_id = ?2",
                    params![pilot_id, install_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(device_id) = device_id else {
                return Ok(false);
            };
            Self::end_active_flights_for_device(conn, pilot_id, &device_id, now_ms())?;
            Self::revoke_device_row(conn, pilot_id, &device_id)
        })
    }

    pub fn revoke_device_by_id(
        &self,
        pilot_id: &str,
        device_id: &str,
    ) -> Result<bool, rusqlite::Error> {
        self.with_conn(|conn| {
            Self::end_active_flights_for_device(conn, pilot_id, device_id, now_ms())?;
            Self::revoke_device_row(conn, pilot_id, device_id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::club::db::ClubDb;

    fn approved_pilot(db: &ClubDb) -> String {
        let club = db.default_club_id().unwrap();
        let id = db
            .register(&club, "P", "p@test", "L1", "password12")
            .unwrap();
        db.approve_pilot(&id).unwrap();
        id
    }

    #[test]
    fn register_returns_token_and_resolves() {
        let db = ClubDb::open_in_memory().unwrap();
        let pilot = approved_pilot(&db);
        let reg = db
            .register_device_for_install(
                &pilot,
                "Pixel 8",
                Some("Google Pixel 8"),
                "install-abc-12345",
            )
            .unwrap();
        assert!(reg.created);
        assert_eq!(
            db.resolve_device_token(&reg.token).as_deref(),
            Some(reg.id.as_str())
        );
        assert_eq!(db.device_pilot_id(&reg.id).as_deref(), Some(pilot.as_str()));
    }

    #[test]
    fn register_idempotent_same_install_id() {
        let db = ClubDb::open_in_memory().unwrap();
        let pilot = approved_pilot(&db);
        let a = db
            .register_device_for_install(&pilot, "Phone", None, "install-repeat-99")
            .unwrap();
        let b = db
            .register_device_for_install(&pilot, "Phone 2", None, "install-repeat-99")
            .unwrap();
        assert!(a.created);
        assert!(!b.created);
        assert_eq!(a.id, b.id);
        assert_ne!(a.token, b.token);
        assert!(db.resolve_device_token(&a.token).is_none());
        assert_eq!(
            db.resolve_device_token(&b.token).as_deref(),
            Some(a.id.as_str())
        );
    }

    #[test]
    fn revoke_by_install_removes_token() {
        let db = ClubDb::open_in_memory().unwrap();
        let pilot = approved_pilot(&db);
        let reg = db
            .register_device_for_install(&pilot, "X", None, "install-revoke-1")
            .unwrap();
        assert!(db.revoke_device_by_install(&pilot, "install-revoke-1").unwrap());
        assert!(db.resolve_device_token(&reg.token).is_none());
    }
}