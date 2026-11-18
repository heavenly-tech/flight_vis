//! Club admin roster CRUD, matrix, CSV import staging, and audit queries.

use crate::club::access_request::normalize_tail;
use crate::club::auth::new_id;
use crate::club::csv_util::{CsvRow, MAX_CSV_ROWS};
use crate::club::db::{now_ms, CheckoutError, ClubDb, DbError};
use serde::{Deserialize, Serialize};
use sqlx::Row;

const IMPORT_TTL_MS: i64 = 30 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: String,
    pub actor_pilot_id: Option<String>,
    pub action: String,
    pub entity_type: String,
    pub entity_id: String,
    pub detail: Option<String>,
    pub before_json: Option<String>,
    pub after_json: Option<String>,
    pub bulk_batch_id: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatrixPilot {
    pub id: String,
    pub name: String,
    pub email: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatrixType {
    pub id: String,
    pub code: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatrixAircraft {
    pub id: String,
    pub tail: String,
    pub type_id: String,
    pub type_code: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatrixCheckout {
    pub id: String,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthMatrix {
    pub pilots: Vec<MatrixPilot>,
    pub types: Vec<MatrixType>,
    pub aircraft: Vec<MatrixAircraft>,
    /// Key: `{pilot_id}:{type_id}`
    pub type_checkouts: std::collections::HashMap<String, MatrixCheckout>,
    /// Key: `{pilot_id}:{aircraft_id}`
    pub tail_checkouts: std::collections::HashMap<String, MatrixCheckout>,
}

#[derive(Debug)]
pub enum RosterError {
    NotFound,
    EmailExists,
    DuplicateTail,
    InvalidTail,
    InvalidStatus,
    InvalidEmail,
    UnknownType,
    UnknownTail,
    PilotWrongClub,
    BatchExpired,
    BatchNotFound,
    BatchMismatch,
    NothingToCommit,
    Db(#[allow(dead_code)] DbError),
}

impl From<DbError> for RosterError {
    fn from(e: DbError) -> Self {
        Self::Db(e)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImportRowAction {
    Add,
    Update,
    Unchanged,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPreviewRow {
    pub line: usize,
    pub action: ImportRowAction,
    pub errors: Vec<String>,
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPreview {
    pub batch_id: String,
    pub kind: String,
    pub rows: Vec<ImportPreviewRow>,
    pub can_commit: bool,
}

impl ClubDb {
    #[allow(clippy::too_many_arguments)]
    pub async fn record_audit_full(
        &self,
        club_id: &str,
        actor_pilot_id: Option<&str>,
        action: &str,
        entity_type: &str,
        entity_id: &str,
        detail: Option<&str>,
        before_json: Option<&str>,
        after_json: Option<&str>,
        bulk_batch_id: Option<&str>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id,
             detail, created_at, before_json, after_json, bulk_batch_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
        )
        .bind(new_id())
        .bind(club_id)
        .bind(actor_pilot_id)
        .bind(action)
        .bind(entity_type)
        .bind(entity_id)
        .bind(detail)
        .bind(now_ms())
        .bind(before_json)
        .bind(after_json)
        .bind(bulk_batch_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn pilot_has_flights(&self, pilot_id: &str) -> bool {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM flights WHERE pilot_id = $1")
            .bind(pilot_id)
            .fetch_one(&self.pool)
            .await
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub async fn aircraft_has_flights(&self, aircraft_id: &str) -> bool {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM flights WHERE aircraft_id = $1")
            .bind(aircraft_id)
            .fetch_one(&self.pool)
            .await
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    pub async fn pilot_in_club(&self, pilot_id: &str, club_id: &str) -> bool {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pilots WHERE id = $1 AND club_id = $2")
            .bind(pilot_id)
            .bind(club_id)
            .fetch_one(&self.pool)
            .await
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn admin_create_pilot(
        &self,
        club_id: &str,
        admin_id: &str,
        name: &str,
        email: &str,
        license_no: &str,
        status: &str,
        role: &str,
    ) -> Result<String, RosterError> {
        let status = normalize_pilot_status(status)?;
        let role = if role == "admin" { "admin" } else { "pilot" };
        let email = email.trim().to_ascii_lowercase();
        if !email.contains('@') {
            return Err(RosterError::InvalidEmail);
        }
        let id = new_id();
        let now = now_ms();
        let member_id = new_id();
        let mut tx = self.pool.begin().await?;
        let r = sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, '', $8)",
        )
        .bind(&id)
        .bind(club_id)
        .bind(name.trim())
        .bind(&email)
        .bind(license_no.trim())
        .bind(status)
        .bind(role)
        .bind(now)
        .execute(&mut *tx)
        .await;
        if let Err(e) = r {
            if let Some(db) = e.as_database_error() {
                if db.code() == Some(std::borrow::Cow::Borrowed("23505")) {
                    let _ = tx.rollback().await;
                    return Err(RosterError::EmailExists);
                }
            }
            return Err(RosterError::Db(e));
        }
        let mem_status = if status == "approved" {
            "approved"
        } else {
            "pending"
        };
        sqlx::query(
            "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(member_id)
        .bind(&id)
        .bind(club_id)
        .bind(role)
        .bind(mem_status)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let after = serde_json::json!({
            "name": name.trim(),
            "email": email,
            "license_no": license_no.trim(),
            "status": status,
            "role": role,
        });
        sqlx::query(
            "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id,
             detail, created_at, before_json, after_json)
             VALUES ($1, $2, $3, 'create', 'pilot', $4, NULL, $5, NULL, $6)",
        )
        .bind(new_id())
        .bind(club_id)
        .bind(admin_id)
        .bind(&id)
        .bind(now)
        .bind(after.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn admin_update_pilot(
        &self,
        club_id: &str,
        admin_id: &str,
        pilot_id: &str,
        name: &str,
        email: &str,
        license_no: &str,
        status: &str,
    ) -> Result<(), RosterError> {
        if !self.pilot_in_club(pilot_id, club_id).await {
            return Err(RosterError::NotFound);
        }
        let status = normalize_pilot_status(status)?;
        let email = email.trim().to_ascii_lowercase();
        if !email.contains('@') {
            return Err(RosterError::InvalidEmail);
        }
        let before: Option<String> = sqlx::query_scalar(
            "SELECT row_to_json(p)::text FROM (
                SELECT name, email, license_no, status, role FROM pilots WHERE id = $1
             ) p",
        )
        .bind(pilot_id)
        .fetch_optional(&self.pool)
        .await?;
        let mut tx = self.pool.begin().await?;
        let r = sqlx::query(
            "UPDATE pilots SET name = $1, email = $2, license_no = $3, status = $4 WHERE id = $5 AND club_id = $6",
        )
        .bind(name.trim())
        .bind(&email)
        .bind(license_no.trim())
        .bind(status)
        .bind(pilot_id)
        .bind(club_id)
        .execute(&mut *tx)
        .await;
        if let Err(e) = r {
            if let Some(db) = e.as_database_error() {
                if db.code() == Some(std::borrow::Cow::Borrowed("23505")) {
                    let _ = tx.rollback().await;
                    return Err(RosterError::EmailExists);
                }
            }
            return Err(RosterError::Db(e));
        }
        if status == "approved" {
            sqlx::query(
                "UPDATE club_memberships SET status = 'approved' WHERE pilot_id = $1 AND club_id = $2",
            )
            .bind(pilot_id)
            .bind(club_id)
            .execute(&mut *tx)
            .await?;
        }
        let after = serde_json::json!({
            "name": name.trim(),
            "email": email,
            "license_no": license_no.trim(),
            "status": status,
        });
        sqlx::query(
            "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id,
             detail, created_at, before_json, after_json)
             VALUES ($1, $2, $3, 'update', 'pilot', $4, NULL, $5, $6, $7)",
        )
        .bind(new_id())
        .bind(club_id)
        .bind(admin_id)
        .bind(pilot_id)
        .bind(now_ms())
        .bind(before)
        .bind(after.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn admin_remove_pilot(
        &self,
        club_id: &str,
        admin_id: &str,
        pilot_id: &str,
    ) -> Result<(), RosterError> {
        if !self.pilot_in_club(pilot_id, club_id).await {
            return Err(RosterError::NotFound);
        }
        let has_flights = self.pilot_has_flights(pilot_id).await;
        let before: Option<String> = sqlx::query_scalar(
            "SELECT row_to_json(p)::text FROM (
                SELECT name, email, status, role FROM pilots WHERE id = $1
             ) p",
        )
        .bind(pilot_id)
        .fetch_optional(&self.pool)
        .await?;
        let mut tx = self.pool.begin().await?;
        if has_flights {
            sqlx::query("UPDATE pilots SET status = 'inactive' WHERE id = $1 AND club_id = $2")
                .bind(pilot_id)
                .bind(club_id)
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query("DELETE FROM pilots WHERE id = $1 AND club_id = $2")
                .bind(pilot_id)
                .bind(club_id)
                .execute(&mut *tx)
                .await?;
        }
        let action = if has_flights { "deactivate" } else { "delete" };
        let after = if has_flights {
            Some(r#"{"status":"inactive"}"#.to_string())
        } else {
            None
        };
        sqlx::query(
            "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id,
             detail, created_at, before_json, after_json)
             VALUES ($1, $2, $3, $4, 'pilot', $5, NULL, $6, $7, $8)",
        )
        .bind(new_id())
        .bind(club_id)
        .bind(admin_id)
        .bind(action)
        .bind(pilot_id)
        .bind(now_ms())
        .bind(before)
        .bind(after)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn admin_upsert_aircraft_roster(
        &self,
        club_id: &str,
        admin_id: &str,
        tail: &str,
        type_code: &str,
        active: bool,
    ) -> Result<String, RosterError> {
        let tail = normalize_tail(tail).map_err(|_| RosterError::InvalidTail)?;
        let type_id: String =
            sqlx::query_scalar("SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2")
                .bind(club_id)
                .bind(type_code.trim())
                .fetch_optional(&self.pool)
                .await?
                .ok_or(RosterError::UnknownType)?;
        let existing: Option<(String, bool)> =
            sqlx::query_as("SELECT id, active FROM aircraft WHERE club_id = $1 AND tail = $2")
                .bind(club_id)
                .bind(&tail)
                .fetch_optional(&self.pool)
                .await?;
        let now = now_ms();
        let mut tx = self.pool.begin().await?;
        let ac_id = if let Some((id, was_active)) = existing {
            sqlx::query("UPDATE aircraft SET type_id = $1, active = $2 WHERE id = $3")
                .bind(&type_id)
                .bind(active)
                .bind(&id)
                .execute(&mut *tx)
                .await?;
            let before =
                serde_json::json!({ "tail": tail, "active": was_active, "type": type_code });
            let after = serde_json::json!({ "tail": tail, "active": active, "type": type_code });
            sqlx::query(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id,
                 detail, created_at, before_json, after_json)
                 VALUES ($1, $2, $3, 'update', 'aircraft', $4, NULL, $5, $6, $7)",
            )
            .bind(new_id())
            .bind(club_id)
            .bind(admin_id)
            .bind(&id)
            .bind(now)
            .bind(before.to_string())
            .bind(after.to_string())
            .execute(&mut *tx)
            .await?;
            id
        } else {
            let id = new_id();
            sqlx::query(
                "INSERT INTO aircraft (id, club_id, tail, type_id, active) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(&id)
            .bind(club_id)
            .bind(&tail)
            .bind(&type_id)
            .bind(active)
            .execute(&mut *tx)
            .await?;
            let after = serde_json::json!({ "tail": tail, "active": active, "type": type_code });
            sqlx::query(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id,
                 detail, created_at, before_json, after_json)
                 VALUES ($1, $2, $3, 'create', 'aircraft', $4, NULL, $5, NULL, $6)",
            )
            .bind(new_id())
            .bind(club_id)
            .bind(admin_id)
            .bind(&id)
            .bind(now)
            .bind(after.to_string())
            .execute(&mut *tx)
            .await?;
            id
        };
        tx.commit().await?;
        Ok(ac_id)
    }

    pub async fn admin_remove_aircraft(
        &self,
        club_id: &str,
        admin_id: &str,
        aircraft_id: &str,
    ) -> Result<(), RosterError> {
        let row: Option<(String, String)> =
            sqlx::query_as("SELECT tail, type_id FROM aircraft WHERE id = $1 AND club_id = $2")
                .bind(aircraft_id)
                .bind(club_id)
                .fetch_optional(&self.pool)
                .await?;
        let Some((tail, _type_id)) = row else {
            return Err(RosterError::NotFound);
        };
        let has_flights = self.aircraft_has_flights(aircraft_id).await;
        let before = serde_json::json!({ "tail": tail });
        let mut tx = self.pool.begin().await?;
        if has_flights {
            sqlx::query("UPDATE aircraft SET active = FALSE WHERE id = $1")
                .bind(aircraft_id)
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query("DELETE FROM aircraft WHERE id = $1 AND club_id = $2")
                .bind(aircraft_id)
                .bind(club_id)
                .execute(&mut *tx)
                .await?;
        }
        let action = if has_flights { "deactivate" } else { "delete" };
        let after = if has_flights {
            Some(r#"{"active":false}"#.to_string())
        } else {
            None
        };
        sqlx::query(
            "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id,
             detail, created_at, before_json, after_json)
             VALUES ($1, $2, $3, $4, 'aircraft', $5, NULL, $6, $7, $8)",
        )
        .bind(new_id())
        .bind(club_id)
        .bind(admin_id)
        .bind(action)
        .bind(aircraft_id)
        .bind(now_ms())
        .bind(before.to_string())
        .bind(after)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn admin_update_aircraft_by_id(
        &self,
        club_id: &str,
        admin_id: &str,
        aircraft_id: &str,
        tail: &str,
        type_code: &str,
        active: bool,
    ) -> Result<String, RosterError> {
        let tail = normalize_tail(tail).map_err(|_| RosterError::InvalidTail)?;
        let exists: Option<String> =
            sqlx::query_scalar("SELECT id FROM aircraft WHERE id = $1 AND club_id = $2")
                .bind(aircraft_id)
                .bind(club_id)
                .fetch_optional(&self.pool)
                .await?;
        if exists.is_none() {
            return Err(RosterError::NotFound);
        }
        let type_id: String =
            sqlx::query_scalar("SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2")
                .bind(club_id)
                .bind(type_code.trim())
                .fetch_optional(&self.pool)
                .await?
                .ok_or(RosterError::UnknownType)?;
        let conflict: Option<String> = sqlx::query_scalar(
            "SELECT id FROM aircraft WHERE club_id = $1 AND tail = $2 AND id != $3",
        )
        .bind(club_id)
        .bind(&tail)
        .bind(aircraft_id)
        .fetch_optional(&self.pool)
        .await?;
        if conflict.is_some() {
            return Err(RosterError::DuplicateTail);
        }
        let before: Option<String> = sqlx::query_scalar(
            "SELECT row_to_json(a)::text FROM (
                SELECT tail, active, type_id FROM aircraft WHERE id = $1
             ) a",
        )
        .bind(aircraft_id)
        .fetch_optional(&self.pool)
        .await?;
        let now = now_ms();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE aircraft SET tail = $1, type_id = $2, active = $3 WHERE id = $4 AND club_id = $5",
        )
        .bind(&tail)
        .bind(&type_id)
        .bind(active)
        .bind(aircraft_id)
        .bind(club_id)
        .execute(&mut *tx)
        .await?;
        let after = serde_json::json!({ "tail": tail, "active": active, "type": type_code });
        sqlx::query(
            "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id,
             detail, created_at, before_json, after_json)
             VALUES ($1, $2, $3, 'update', 'aircraft', $4, NULL, $5, $6, $7)",
        )
        .bind(new_id())
        .bind(club_id)
        .bind(admin_id)
        .bind(aircraft_id)
        .bind(now)
        .bind(before)
        .bind(after.to_string())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(aircraft_id.to_string())
    }

    pub async fn auth_matrix(&self, club_id: &str) -> Result<AuthMatrix, DbError> {
        let pilots = sqlx::query(
            "SELECT id, name, email, status FROM pilots WHERE club_id = $1 AND status != 'inactive' ORDER BY email",
        )
        .bind(club_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .filter_map(|r| {
            Some(MatrixPilot {
                id: r.try_get(0).ok()?,
                name: r.try_get(1).ok()?,
                email: r.try_get(2).ok()?,
                status: r.try_get(3).ok()?,
            })
        })
        .collect();
        let types =
            sqlx::query("SELECT id, code FROM aircraft_types WHERE club_id = $1 ORDER BY code")
                .bind(club_id)
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .filter_map(|r| {
                    Some(MatrixType {
                        id: r.try_get(0).ok()?,
                        code: r.try_get(1).ok()?,
                    })
                })
                .collect();
        let aircraft = sqlx::query(
            "SELECT a.id, a.tail, a.type_id, t.code, a.active
             FROM aircraft a JOIN aircraft_types t ON t.id = a.type_id
             WHERE a.club_id = $1 ORDER BY a.tail",
        )
        .bind(club_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .filter_map(|r| {
            Some(MatrixAircraft {
                id: r.try_get(0).ok()?,
                tail: r.try_get(1).ok()?,
                type_id: r.try_get(2).ok()?,
                type_code: r.try_get(3).ok()?,
                active: r.try_get(4).ok()?,
            })
        })
        .collect();
        let rows = sqlx::query(
            "SELECT c.id, c.pilot_id, c.type_id, c.aircraft_id, c.expires_at
             FROM checkouts c
             JOIN pilots p ON p.id = c.pilot_id
             WHERE p.club_id = $1",
        )
        .bind(club_id)
        .fetch_all(&self.pool)
        .await?;
        let mut type_checkouts = std::collections::HashMap::new();
        let mut tail_checkouts = std::collections::HashMap::new();
        for r in rows {
            let id: String = r.try_get(0)?;
            let pilot_id: String = r.try_get(1)?;
            let type_id: String = r.try_get(2)?;
            let aircraft_id: Option<String> = r.try_get(3)?;
            let expires_at: Option<i64> = r.try_get(4)?;
            let entry = MatrixCheckout { id, expires_at };
            if let Some(ac_id) = aircraft_id {
                tail_checkouts.insert(format!("{pilot_id}:{ac_id}"), entry);
            } else {
                type_checkouts.insert(format!("{pilot_id}:{type_id}"), entry);
            }
        }
        Ok(AuthMatrix {
            pilots,
            types,
            aircraft,
            type_checkouts,
            tail_checkouts,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn toggle_checkout_roster(
        &self,
        club_id: &str,
        admin_id: &str,
        pilot_id: &str,
        type_code: &str,
        tail: Option<&str>,
        enable: bool,
        expires_at: Option<i64>,
    ) -> Result<(), RosterError> {
        if !self.pilot_in_club(pilot_id, club_id).await {
            return Err(RosterError::PilotWrongClub);
        }
        if enable {
            self.add_checkout_for_club(club_id, admin_id, pilot_id, type_code, tail, expires_at)
                .await
        } else {
            self.remove_checkout_for_club(club_id, admin_id, pilot_id, type_code, tail)
                .await
        }
    }

    async fn add_checkout_for_club(
        &self,
        club_id: &str,
        admin_id: &str,
        pilot_id: &str,
        type_code: &str,
        tail: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<(), RosterError> {
        if let Some(t) = tail {
            let tail = normalize_tail(t).map_err(|_| RosterError::InvalidTail)?;
            let ac_club: Option<String> =
                sqlx::query_scalar("SELECT club_id FROM aircraft WHERE tail = $1")
                    .bind(&tail)
                    .fetch_optional(&self.pool)
                    .await?;
            if ac_club.as_deref() != Some(club_id) {
                return Err(RosterError::UnknownTail);
            }
        }
        match self
            .add_checkout_with_club(pilot_id, type_code, tail, expires_at, Some(club_id))
            .await
        {
            Ok(()) => {
                self.record_audit_full(
                    club_id,
                    Some(admin_id),
                    "assign",
                    "checkout",
                    pilot_id,
                    Some(type_code),
                    None,
                    None,
                    None,
                )
                .await?;
                Ok(())
            }
            Err(CheckoutError::UnknownTail) => Err(RosterError::UnknownTail),
            Err(CheckoutError::UnknownType) => Err(RosterError::UnknownType),
            Err(CheckoutError::Db(e)) => Err(RosterError::Db(e)),
        }
    }

    pub async fn add_checkout_with_club(
        &self,
        pilot_id: &str,
        type_code: &str,
        tail: Option<&str>,
        expires_at: Option<i64>,
        club_id: Option<&str>,
    ) -> Result<(), CheckoutError> {
        let pilot_club: String = sqlx::query_scalar("SELECT club_id FROM pilots WHERE id = $1")
            .bind(pilot_id)
            .fetch_one(&self.pool)
            .await
            .map_err(CheckoutError::Db)?;
        let club_id = club_id.unwrap_or(pilot_club.as_str());
        let (type_id, aircraft_id) = if let Some(tail) = tail {
            let row = sqlx::query(
                "SELECT a.id, a.type_id FROM aircraft a WHERE a.club_id = $1 AND a.tail = $2",
            )
            .bind(club_id)
            .bind(tail)
            .fetch_optional(&self.pool)
            .await
            .map_err(CheckoutError::Db)?;
            let Some(row) = row else {
                return Err(CheckoutError::UnknownTail);
            };
            (
                row.try_get::<String, _>(1)?,
                Some(row.try_get::<String, _>(0)?),
            )
        } else {
            let type_id: String = sqlx::query_scalar(
                "SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2",
            )
            .bind(club_id)
            .bind(type_code)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => CheckoutError::UnknownType,
                other => CheckoutError::Db(other),
            })?;
            (type_id, None)
        };
        let existing = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM checkouts WHERE pilot_id = $1 AND type_id = $2
             AND (($3::text IS NULL AND aircraft_id IS NULL) OR aircraft_id = $3)",
        )
        .bind(pilot_id)
        .bind(&type_id)
        .bind(aircraft_id.as_deref())
        .fetch_one(&self.pool)
        .await
        .map_err(CheckoutError::Db)?;
        if existing > 0 {
            sqlx::query(
                "UPDATE checkouts SET expires_at = $1, club_id = $2
                 WHERE pilot_id = $3 AND type_id = $4
                   AND (($5::text IS NULL AND aircraft_id IS NULL) OR aircraft_id = $5)",
            )
            .bind(expires_at)
            .bind(club_id)
            .bind(pilot_id)
            .bind(&type_id)
            .bind(aircraft_id.as_deref())
            .execute(&self.pool)
            .await
            .map_err(CheckoutError::Db)?;
        } else {
            sqlx::query(
                "INSERT INTO checkouts (id, pilot_id, type_id, aircraft_id, expires_at, club_id)
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(new_id())
            .bind(pilot_id)
            .bind(&type_id)
            .bind(aircraft_id)
            .bind(expires_at)
            .bind(club_id)
            .execute(&self.pool)
            .await
            .map_err(CheckoutError::Db)?;
        }
        Ok(())
    }

    async fn remove_checkout_for_club(
        &self,
        club_id: &str,
        admin_id: &str,
        pilot_id: &str,
        type_code: &str,
        tail: Option<&str>,
    ) -> Result<(), RosterError> {
        let type_id: String =
            sqlx::query_scalar("SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2")
                .bind(club_id)
                .bind(type_code)
                .fetch_optional(&self.pool)
                .await?
                .ok_or(RosterError::UnknownType)?;
        let aircraft_id: Option<String> = if let Some(t) = tail {
            let tail = normalize_tail(t).map_err(|_| RosterError::InvalidTail)?;
            let id: Option<String> =
                sqlx::query_scalar("SELECT id FROM aircraft WHERE club_id = $1 AND tail = $2")
                    .bind(club_id)
                    .bind(&tail)
                    .fetch_optional(&self.pool)
                    .await?;
            Some(id.ok_or(RosterError::UnknownTail)?)
        } else {
            None
        };
        if let Some(ac) = aircraft_id.as_deref() {
            sqlx::query(
                "DELETE FROM checkouts WHERE pilot_id = $1 AND type_id = $2 AND aircraft_id = $3",
            )
            .bind(pilot_id)
            .bind(&type_id)
            .bind(ac)
            .execute(&self.pool)
            .await?;
        } else {
            sqlx::query(
                "DELETE FROM checkouts WHERE pilot_id = $1 AND type_id = $2 AND aircraft_id IS NULL",
            )
            .bind(pilot_id)
            .bind(&type_id)
            .execute(&self.pool)
            .await?;
        }
        self.record_audit_full(
            club_id,
            Some(admin_id),
            "unassign",
            "checkout",
            pilot_id,
            Some(type_code),
            None,
            None,
            None,
        )
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn bulk_checkout_assign(
        &self,
        club_id: &str,
        admin_id: &str,
        pilot_ids: &[String],
        type_code: &str,
        tail: Option<&str>,
        assign: bool,
        expires_at: Option<i64>,
    ) -> Result<String, RosterError> {
        let batch_id = new_id();
        for pid in pilot_ids {
            if !self.pilot_in_club(pid, club_id).await {
                return Err(RosterError::PilotWrongClub);
            }
        }
        for pid in pilot_ids {
            self.toggle_checkout_roster(
                club_id, admin_id, pid, type_code, tail, assign, expires_at,
            )
            .await?;
            self.record_audit_full(
                club_id,
                Some(admin_id),
                if assign {
                    "bulk_assign"
                } else {
                    "bulk_unassign"
                },
                "checkout",
                pid,
                Some(type_code),
                None,
                None,
                Some(&batch_id),
            )
            .await?;
        }
        Ok(batch_id)
    }

    pub async fn remove_checkout_by_id(
        &self,
        club_id: &str,
        admin_id: &str,
        checkout_id: &str,
    ) -> Result<(), RosterError> {
        let row = sqlx::query(
            "SELECT c.pilot_id, t.code, a.tail
             FROM checkouts c
             JOIN pilots p ON p.id = c.pilot_id
             JOIN aircraft_types t ON t.id = c.type_id
             LEFT JOIN aircraft a ON a.id = c.aircraft_id
             WHERE c.id = $1 AND p.club_id = $2",
        )
        .bind(checkout_id)
        .bind(club_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Err(RosterError::NotFound);
        };
        let pilot_id: String = row.try_get(0)?;
        let type_code: String = row.try_get(1)?;
        let tail: Option<String> = row.try_get(2)?;
        sqlx::query("DELETE FROM checkouts WHERE id = $1")
            .bind(checkout_id)
            .execute(&self.pool)
            .await?;
        self.record_audit_full(
            club_id,
            Some(admin_id),
            "unassign",
            "checkout",
            &pilot_id,
            Some(&type_code),
            tail.as_deref(),
            None,
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn list_audit(
        &self,
        club_id: &str,
        entity_type: Option<&str>,
        offset: i64,
        limit: i64,
    ) -> Result<(Vec<AuditEntry>, i64), DbError> {
        let limit = limit.clamp(1, 100);
        let total: i64 = if let Some(et) = entity_type {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM audit_log WHERE club_id = $1 AND entity_type = $2",
            )
            .bind(club_id)
            .bind(et)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query_scalar("SELECT COUNT(*) FROM audit_log WHERE club_id = $1")
                .bind(club_id)
                .fetch_one(&self.pool)
                .await?
        };
        let rows = if let Some(et) = entity_type {
            sqlx::query(
                "SELECT id, actor_pilot_id, action, entity_type, entity_id, detail, before_json,
                        after_json, bulk_batch_id, created_at
                 FROM audit_log WHERE club_id = $1 AND entity_type = $2
                 ORDER BY created_at DESC LIMIT $3 OFFSET $4",
            )
            .bind(club_id)
            .bind(et)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT id, actor_pilot_id, action, entity_type, entity_id, detail, before_json,
                        after_json, bulk_batch_id, created_at
                 FROM audit_log WHERE club_id = $1
                 ORDER BY created_at DESC LIMIT $2 OFFSET $3",
            )
            .bind(club_id)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?
        };
        let entries = rows
            .into_iter()
            .filter_map(|r| {
                Some(AuditEntry {
                    id: r.try_get(0).ok()?,
                    actor_pilot_id: r.try_get(1).ok()?,
                    action: r.try_get(2).ok()?,
                    entity_type: r.try_get(3).ok()?,
                    entity_id: r.try_get(4).ok()?,
                    detail: r.try_get(5).ok()?,
                    before_json: r.try_get(6).ok()?,
                    after_json: r.try_get(7).ok()?,
                    bulk_batch_id: r.try_get(8).ok()?,
                    created_at: r.try_get(9).ok()?,
                })
            })
            .collect();
        Ok((entries, total))
    }

    pub async fn import_preview(
        &self,
        club_id: &str,
        admin_id: &str,
        kind: &str,
        csv_rows: &[CsvRow],
    ) -> Result<ImportPreview, RosterError> {
        if csv_rows.is_empty() {
            return Err(RosterError::NothingToCommit);
        }
        let (preview_rows, ops) = match kind {
            "pilots" => self.preview_pilots_csv(club_id, csv_rows).await?,
            "aircraft" => self.preview_aircraft_csv(club_id, csv_rows).await?,
            "authorizations" => self.preview_auth_csv(club_id, csv_rows).await?,
            _ => return Err(RosterError::NotFound),
        };
        let can_commit = preview_rows
            .iter()
            .all(|r| r.action != ImportRowAction::Error);
        let batch_id = new_id();
        let now = now_ms();
        let preview = ImportPreview {
            batch_id: batch_id.clone(),
            kind: kind.to_string(),
            rows: preview_rows,
            can_commit,
        };
        let payload = serde_json::json!({ "preview": preview, "ops": ops });
        sqlx::query(
            "INSERT INTO roster_import_batches (id, club_id, actor_pilot_id, kind, preview_json, created_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&batch_id)
        .bind(club_id)
        .bind(admin_id)
        .bind(kind)
        .bind(payload.to_string())
        .bind(now)
        .bind(now + IMPORT_TTL_MS)
        .execute(&self.pool)
        .await?;
        Ok(preview)
    }

    pub async fn import_commit(
        &self,
        club_id: &str,
        admin_id: &str,
        batch_id: &str,
    ) -> Result<usize, RosterError> {
        let now = now_ms();
        let row = sqlx::query(
            "SELECT kind, preview_json, actor_pilot_id FROM roster_import_batches
             WHERE id = $1 AND club_id = $2 AND expires_at > $3",
        )
        .bind(batch_id)
        .bind(club_id)
        .bind(now)
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Err(RosterError::BatchNotFound);
        };
        let kind: String = row.try_get(0)?;
        let json: String = row.try_get(1)?;
        let owner: String = row.try_get(2)?;
        if owner != admin_id {
            return Err(RosterError::BatchMismatch);
        }
        let payload: serde_json::Value =
            serde_json::from_str(&json).map_err(|_| RosterError::BatchExpired)?;
        let ops = payload
            .get("ops")
            .cloned()
            .ok_or(RosterError::BatchExpired)?;
        let mut tx = self.pool.begin().await?;
        let applied = self
            .apply_import_ops(&mut tx, club_id, admin_id, &kind, &ops, batch_id)
            .await?;
        sqlx::query("DELETE FROM roster_import_batches WHERE id = $1")
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(applied)
    }

    async fn apply_import_ops(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        club_id: &str,
        admin_id: &str,
        kind: &str,
        ops: &serde_json::Value,
        batch_id: &str,
    ) -> Result<usize, RosterError> {
        let arr = ops.as_array().ok_or(RosterError::BatchExpired)?;
        let mut n = 0usize;
        for op in arr {
            match kind {
                "pilots" => {
                    self.apply_pilot_op(tx, club_id, admin_id, op, batch_id)
                        .await?;
                    n += 1;
                }
                "aircraft" => {
                    self.apply_aircraft_op(tx, club_id, admin_id, op, batch_id)
                        .await?;
                    n += 1;
                }
                "authorizations" => {
                    self.apply_auth_op(tx, club_id, admin_id, op, batch_id)
                        .await?;
                    n += 1;
                }
                _ => return Err(RosterError::NotFound),
            }
        }
        Ok(n)
    }

    async fn preview_pilots_csv(
        &self,
        club_id: &str,
        rows: &[CsvRow],
    ) -> Result<(Vec<ImportPreviewRow>, serde_json::Value), RosterError> {
        let mut out = Vec::new();
        let mut ops = Vec::new();
        let header = rows[0]
            .fields
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect::<Vec<_>>();
        let data_rows = if header.first().map(|h| h.contains("email")).unwrap_or(false) {
            &rows[1..]
        } else {
            rows
        };
        if data_rows.len() > MAX_CSV_ROWS {
            return Err(RosterError::Db(sqlx::Error::RowNotFound));
        }
        for row in data_rows {
            let mut errors = Vec::new();
            let name = row
                .fields
                .first()
                .map(|s| s.trim())
                .unwrap_or("")
                .to_string();
            let email = row
                .fields
                .get(1)
                .map(|s| s.trim().to_ascii_lowercase())
                .unwrap_or_default();
            let license = row
                .fields
                .get(2)
                .map(|s| s.trim())
                .unwrap_or("")
                .to_string();
            let status = row
                .fields
                .get(3)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("approved");
            if name.is_empty() {
                errors.push("name required".into());
            }
            if !email.contains('@') {
                errors.push("invalid email".into());
            }
            if normalize_pilot_status(status).is_err() {
                errors.push("invalid status".into());
            }
            let existing: Option<String> = if errors.is_empty() {
                sqlx::query_scalar("SELECT id FROM pilots WHERE club_id = $1 AND lower(email) = $2")
                    .bind(club_id)
                    .bind(&email)
                    .fetch_optional(&self.pool)
                    .await?
            } else {
                None
            };
            let (action, summary) = if !errors.is_empty() {
                (ImportRowAction::Error, None)
            } else if let Some(id) = existing {
                ops.push(serde_json::json!({
                    "op": "update",
                    "id": id,
                    "name": name,
                    "email": email,
                    "license_no": license,
                    "status": status,
                }));
                (ImportRowAction::Update, Some(format!("update {email}")))
            } else {
                ops.push(serde_json::json!({
                    "op": "create",
                    "name": name,
                    "email": email,
                    "license_no": license,
                    "status": status,
                }));
                (ImportRowAction::Add, Some(format!("add {email}")))
            };
            out.push(ImportPreviewRow {
                line: row.line,
                action,
                errors,
                summary,
            });
        }
        Ok((out, serde_json::Value::Array(ops)))
    }

    async fn preview_aircraft_csv(
        &self,
        club_id: &str,
        rows: &[CsvRow],
    ) -> Result<(Vec<ImportPreviewRow>, serde_json::Value), RosterError> {
        let mut out = Vec::new();
        let mut ops = Vec::new();
        let header = rows[0]
            .fields
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect::<Vec<_>>();
        let data_rows = if header
            .first()
            .map(|h| h == "tail" || h.contains("matricula"))
            .unwrap_or(false)
        {
            &rows[1..]
        } else {
            rows
        };
        for row in data_rows {
            let mut errors = Vec::new();
            let tail_raw = row.fields.first().map(|s| s.trim()).unwrap_or("");
            let type_code = row.fields.get(1).map(|s| s.trim()).unwrap_or("");
            let active = row
                .fields
                .get(2)
                .map(|s| !matches!(s.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"))
                .unwrap_or(true);
            let tail_norm = normalize_tail(tail_raw);
            if tail_norm.is_err() {
                errors.push("invalid tail (CC-XXX)".into());
            }
            if type_code.is_empty() {
                errors.push("type required".into());
            }
            let type_ok = if errors.is_empty() {
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM aircraft_types WHERE club_id = $1 AND code = $2",
                )
                .bind(club_id)
                .bind(type_code)
                .fetch_one(&self.pool)
                .await?
                    > 0
            } else {
                false
            };
            if !type_ok && errors.is_empty() {
                errors.push("unknown type".into());
            }
            let tail = tail_norm.unwrap_or_default();
            let existing: Option<String> = if errors.is_empty() {
                sqlx::query_scalar("SELECT id FROM aircraft WHERE club_id = $1 AND tail = $2")
                    .bind(club_id)
                    .bind(&tail)
                    .fetch_optional(&self.pool)
                    .await?
            } else {
                None
            };
            let (action, summary) = if !errors.is_empty() {
                (ImportRowAction::Error, None)
            } else if existing.is_some() {
                ops.push(serde_json::json!({
                    "op": "upsert",
                    "tail": tail,
                    "type": type_code,
                    "active": active,
                }));
                (ImportRowAction::Update, Some(format!("update {tail}")))
            } else {
                ops.push(serde_json::json!({
                    "op": "upsert",
                    "tail": tail,
                    "type": type_code,
                    "active": active,
                }));
                (ImportRowAction::Add, Some(format!("add {tail}")))
            };
            out.push(ImportPreviewRow {
                line: row.line,
                action,
                errors,
                summary,
            });
        }
        Ok((out, serde_json::Value::Array(ops)))
    }

    async fn preview_auth_csv(
        &self,
        club_id: &str,
        rows: &[CsvRow],
    ) -> Result<(Vec<ImportPreviewRow>, serde_json::Value), RosterError> {
        let mut out = Vec::new();
        let mut ops = Vec::new();
        let header = rows[0]
            .fields
            .iter()
            .map(|s| s.to_ascii_lowercase())
            .collect::<Vec<_>>();
        let data_rows = if header.first().map(|h| h.contains("email")).unwrap_or(false) {
            &rows[1..]
        } else {
            rows
        };
        for row in data_rows {
            let mut errors = Vec::new();
            let email = row
                .fields
                .first()
                .map(|s| s.trim().to_ascii_lowercase())
                .unwrap_or_default();
            let type_code = row.fields.get(1).map(|s| s.trim()).unwrap_or("");
            let tail_raw = row
                .fields
                .get(2)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty());
            let assign = row
                .fields
                .get(4)
                .map(|s| {
                    !matches!(
                        s.trim().to_ascii_lowercase().as_str(),
                        "unassign" | "remove" | "0" | "false"
                    )
                })
                .unwrap_or(true);
            if !email.contains('@') {
                errors.push("invalid pilot email".into());
            }
            if type_code.is_empty() {
                errors.push("type required".into());
            }
            let pilot_id: Option<String> = if errors.is_empty() {
                sqlx::query_scalar("SELECT id FROM pilots WHERE club_id = $1 AND lower(email) = $2")
                    .bind(club_id)
                    .bind(&email)
                    .fetch_optional(&self.pool)
                    .await?
            } else {
                None
            };
            if pilot_id.is_none() && errors.is_empty() {
                errors.push("pilot not in club".into());
            }
            let mut tail: Option<String> = None;
            if let Some(t) = tail_raw {
                match normalize_tail(t) {
                    Ok(n) => {
                        let ac_club: Option<String> =
                            sqlx::query_scalar("SELECT club_id FROM aircraft WHERE tail = $1")
                                .bind(&n)
                                .fetch_optional(&self.pool)
                                .await?;
                        if ac_club.as_deref() != Some(club_id) {
                            errors.push("tail not in club".into());
                        } else {
                            tail = Some(n);
                        }
                    }
                    Err(_) => errors.push("invalid tail".into()),
                }
            }
            let (action, summary) = if !errors.is_empty() {
                (ImportRowAction::Error, None)
            } else {
                let pid = pilot_id.unwrap();
                ops.push(serde_json::json!({
                    "op": if assign { "assign" } else { "unassign" },
                    "pilot_id": pid,
                    "type": type_code,
                    "tail": tail,
                }));
                (
                    if assign {
                        ImportRowAction::Add
                    } else {
                        ImportRowAction::Update
                    },
                    Some(format!(
                        "{} {email}",
                        if assign { "assign" } else { "unassign" }
                    )),
                )
            };
            out.push(ImportPreviewRow {
                line: row.line,
                action,
                errors,
                summary,
            });
        }
        Ok((out, serde_json::Value::Array(ops)))
    }

    async fn apply_pilot_op(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        club_id: &str,
        admin_id: &str,
        op: &serde_json::Value,
        batch_id: &str,
    ) -> Result<(), RosterError> {
        let now = now_ms();
        match op.get("op").and_then(|v| v.as_str()) {
            Some("create") => {
                let id = new_id();
                let member_id = new_id();
                let name = op["name"].as_str().unwrap_or("");
                let email = op["email"].as_str().unwrap_or("").to_ascii_lowercase();
                let license = op["license_no"].as_str().unwrap_or("");
                let status = op["status"].as_str().unwrap_or("approved");
                sqlx::query(
                    "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
                     VALUES ($1, $2, $3, $4, $5, $6, 'pilot', '', $7)",
                )
                .bind(&id)
                .bind(club_id)
                .bind(name)
                .bind(&email)
                .bind(license)
                .bind(status)
                .bind(now)
                .execute(&mut **tx)
                .await?;
                sqlx::query(
                    "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
                     VALUES ($1, $2, $3, 'pilot', $4, $5)",
                )
                .bind(member_id)
                .bind(&id)
                .bind(club_id)
                .bind(if status == "approved" {
                    "approved"
                } else {
                    "pending"
                })
                .bind(now)
                .execute(&mut **tx)
                .await?;
                audit_tx(
                    tx,
                    club_id,
                    admin_id,
                    "bulk_create",
                    "pilot",
                    &id,
                    batch_id,
                    None,
                    Some(&op.to_string()),
                )
                .await?;
            }
            Some("update") => {
                let id = op["id"].as_str().unwrap_or("");
                sqlx::query(
                    "UPDATE pilots SET name = $1, email = $2, license_no = $3, status = $4
                     WHERE id = $5 AND club_id = $6",
                )
                .bind(op["name"].as_str())
                .bind(op["email"].as_str())
                .bind(op["license_no"].as_str())
                .bind(op["status"].as_str())
                .bind(id)
                .bind(club_id)
                .execute(&mut **tx)
                .await?;
                audit_tx(
                    tx,
                    club_id,
                    admin_id,
                    "bulk_update",
                    "pilot",
                    id,
                    batch_id,
                    None,
                    Some(&op.to_string()),
                )
                .await?;
            }
            _ => return Err(RosterError::BatchExpired),
        }
        Ok(())
    }

    async fn apply_aircraft_op(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        club_id: &str,
        admin_id: &str,
        op: &serde_json::Value,
        batch_id: &str,
    ) -> Result<(), RosterError> {
        let tail = op["tail"].as_str().unwrap_or("");
        let type_code = op["type"].as_str().unwrap_or("");
        let active = op["active"].as_bool().unwrap_or(true);
        let type_id: String =
            sqlx::query_scalar("SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2")
                .bind(club_id)
                .bind(type_code)
                .fetch_one(&mut **tx)
                .await
                .map_err(|_| RosterError::UnknownType)?;
        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM aircraft WHERE club_id = $1 AND tail = $2")
                .bind(club_id)
                .bind(tail)
                .fetch_optional(&mut **tx)
                .await?;
        let ac_id = if let Some(id) = existing {
            sqlx::query("UPDATE aircraft SET type_id = $1, active = $2 WHERE id = $3")
                .bind(&type_id)
                .bind(active)
                .bind(&id)
                .execute(&mut **tx)
                .await?;
            id
        } else {
            let id = new_id();
            sqlx::query(
                "INSERT INTO aircraft (id, club_id, tail, type_id, active) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(&id)
            .bind(club_id)
            .bind(tail)
            .bind(&type_id)
            .bind(active)
            .execute(&mut **tx)
            .await?;
            id
        };
        audit_tx(
            tx,
            club_id,
            admin_id,
            "bulk_upsert",
            "aircraft",
            &ac_id,
            batch_id,
            None,
            Some(&op.to_string()),
        )
        .await?;
        Ok(())
    }

    async fn apply_auth_op(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        club_id: &str,
        admin_id: &str,
        op: &serde_json::Value,
        batch_id: &str,
    ) -> Result<(), RosterError> {
        let pilot_id = op["pilot_id"].as_str().unwrap_or("");
        let type_code = op["type"].as_str().unwrap_or("");
        let tail = op["tail"].as_str();
        let assign = op["op"].as_str() == Some("assign");
        let type_id: String =
            sqlx::query_scalar("SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2")
                .bind(club_id)
                .bind(type_code)
                .fetch_one(&mut **tx)
                .await
                .map_err(|_| RosterError::UnknownType)?;
        let aircraft_id: Option<String> = if let Some(t) = tail {
            let id: String =
                sqlx::query_scalar("SELECT id FROM aircraft WHERE club_id = $1 AND tail = $2")
                    .bind(club_id)
                    .bind(t)
                    .fetch_one(&mut **tx)
                    .await
                    .map_err(|_| RosterError::UnknownTail)?;
            Some(id)
        } else {
            None
        };
        if assign {
            let existing = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM checkouts WHERE pilot_id = $1 AND type_id = $2
                 AND (($3::text IS NULL AND aircraft_id IS NULL) OR aircraft_id = $3)",
            )
            .bind(pilot_id)
            .bind(&type_id)
            .bind(aircraft_id.as_deref())
            .fetch_one(&mut **tx)
            .await?;
            if existing == 0 {
                sqlx::query(
                    "INSERT INTO checkouts (id, pilot_id, type_id, aircraft_id, expires_at, club_id)
                     VALUES ($1, $2, $3, $4, NULL, $5)",
                )
                .bind(new_id())
                .bind(pilot_id)
                .bind(&type_id)
                .bind(aircraft_id)
                .bind(club_id)
                .execute(&mut **tx)
                .await?;
            }
        } else if let Some(ac) = aircraft_id.as_deref() {
            sqlx::query(
                "DELETE FROM checkouts WHERE pilot_id = $1 AND type_id = $2 AND aircraft_id = $3",
            )
            .bind(pilot_id)
            .bind(&type_id)
            .bind(ac)
            .execute(&mut **tx)
            .await?;
        } else {
            sqlx::query(
                "DELETE FROM checkouts WHERE pilot_id = $1 AND type_id = $2 AND aircraft_id IS NULL",
            )
            .bind(pilot_id)
            .bind(&type_id)
            .execute(&mut **tx)
            .await?;
        }
        audit_tx(
            tx,
            club_id,
            admin_id,
            if assign {
                "bulk_assign"
            } else {
                "bulk_unassign"
            },
            "checkout",
            pilot_id,
            batch_id,
            None,
            Some(&op.to_string()),
        )
        .await?;
        Ok(())
    }

    pub fn import_template(kind: &str) -> Option<String> {
        match kind {
            "pilots" => Some(
                "name,email,license_no,status\nPablo Demo,pablo@demo.test,LIC-1,approved\n".into(),
            ),
            "aircraft" => Some("tail,type,active\nCC-ABC,C152,true\n".into()),
            "authorizations" => Some(
                "pilot_email,type,tail,expires_at,action\npablo@demo.test,C152,,,assign\n".into(),
            ),
            _ => None,
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn audit_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    club_id: &str,
    admin_id: &str,
    action: &str,
    entity_type: &str,
    entity_id: &str,
    batch_id: &str,
    before_json: Option<&str>,
    after_json: Option<&str>,
) -> Result<(), RosterError> {
    sqlx::query(
        "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id,
         detail, created_at, before_json, after_json, bulk_batch_id)
         VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8, $9, $10)",
    )
    .bind(new_id())
    .bind(club_id)
    .bind(admin_id)
    .bind(action)
    .bind(entity_type)
    .bind(entity_id)
    .bind(now_ms())
    .bind(before_json)
    .bind(after_json)
    .bind(batch_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn normalize_pilot_status(status: &str) -> Result<&'static str, RosterError> {
    match status.trim().to_ascii_lowercase().as_str() {
        "approved" | "pending" | "inactive" => {
            Ok(match status.trim().to_ascii_lowercase().as_str() {
                "approved" => "approved",
                "pending" => "pending",
                _ => "inactive",
            })
        }
        _ => Err(RosterError::InvalidStatus),
    }
}
