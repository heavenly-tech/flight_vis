use crate::club::{
    auth::{digest_token, new_device_token, new_id},
    db::{now_ms, ClubDb, DbError, DeviceRegisterError, MAX_DEVICES_PER_PILOT},
};
use serde::Serialize;

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

pub const MIN_INSTALL_ID_LEN: usize = 8;
pub const MAX_INSTALL_ID_LEN: usize = 128;

impl ClubDb {
    async fn end_active_flights_for_device(
        &self,
        pilot_id: &str,
        device_id: &str,
        now: i64,
    ) -> Result<(), DbError> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT f.id FROM flights f
             INNER JOIN flight_devices fd ON fd.flight_id = f.id
             WHERE fd.device_id = $1 AND f.pilot_id = $2 AND f.status = 'active'",
        )
        .bind(device_id)
        .bind(pilot_id)
        .fetch_all(&self.pool)
        .await?;
        for flight_id in ids {
            sqlx::query("UPDATE flights SET status = 'ended', ended_at = $1 WHERE id = $2")
                .bind(now)
                .bind(&flight_id)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    async fn revoke_device_row(&self, pilot_id: &str, device_id: &str) -> Result<bool, DbError> {
        let history: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM flight_devices WHERE device_id = $1")
                .bind(device_id)
                .fetch_one(&self.pool)
                .await?;
        if history == 0 {
            let deleted = sqlx::query("DELETE FROM devices WHERE pilot_id = $1 AND id = $2")
                .bind(pilot_id)
                .bind(device_id)
                .execute(&self.pool)
                .await?;
            return Ok(deleted.rows_affected() > 0);
        }
        let revoked_hash = digest_token(&format!("revoked-{}", new_id()));
        let updated =
            sqlx::query("UPDATE devices SET token_hash = $1 WHERE pilot_id = $2 AND id = $3")
                .bind(&revoked_hash)
                .bind(pilot_id)
                .bind(device_id)
                .execute(&self.pool)
                .await?;
        let _ = sqlx::query("DELETE FROM device_pair_tokens WHERE device_id = $1")
            .bind(device_id)
            .execute(&self.pool)
            .await;
        Ok(updated.rows_affected() > 0)
    }

    pub async fn register_device_for_install(
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

        if let Some(existing_id) = sqlx::query_scalar::<_, String>(
            "SELECT id FROM devices WHERE pilot_id = $1 AND install_id = $2",
        )
        .bind(pilot_id)
        .bind(install_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| DeviceRegisterError::Db)?
        {
            sqlx::query("UPDATE devices SET token_hash = $1, label = $2, model = $3 WHERE id = $4")
                .bind(&hash)
                .bind(label)
                .bind(model)
                .bind(&existing_id)
                .execute(&self.pool)
                .await
                .map_err(|_| DeviceRegisterError::Db)?;
            let _ = sqlx::query("DELETE FROM device_pair_tokens WHERE device_id = $1")
                .bind(&existing_id)
                .execute(&self.pool)
                .await;
            return Ok(DeviceRegisterResponse {
                id: existing_id,
                token,
                label: label.to_string(),
                model: model.map(String::from),
                install_id: Some(install_id.to_string()),
                created: false,
            });
        }

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE pilot_id = $1")
            .bind(pilot_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| DeviceRegisterError::Db)?;
        if count as usize >= MAX_DEVICES_PER_PILOT {
            return Err(DeviceRegisterError::CapExceeded);
        }
        let id = new_id();
        sqlx::query(
            "INSERT INTO devices (id, pilot_id, token_hash, label, model, install_id, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&id)
        .bind(pilot_id)
        .bind(&hash)
        .bind(label)
        .bind(model)
        .bind(install_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|_| DeviceRegisterError::Db)?;
        Ok(DeviceRegisterResponse {
            id,
            token,
            label: label.to_string(),
            model: model.map(String::from),
            install_id: Some(install_id.to_string()),
            created: true,
        })
    }

    pub async fn revoke_device_by_install(
        &self,
        pilot_id: &str,
        install_id: &str,
    ) -> Result<bool, DbError> {
        let install_id = install_id.trim();
        let device_id: Option<String> =
            sqlx::query_scalar("SELECT id FROM devices WHERE pilot_id = $1 AND install_id = $2")
                .bind(pilot_id)
                .bind(install_id)
                .fetch_optional(&self.pool)
                .await?;
        let Some(device_id) = device_id else {
            return Ok(false);
        };
        let now = now_ms();
        self.end_active_flights_for_device(pilot_id, &device_id, now)
            .await?;
        self.revoke_device_row(pilot_id, &device_id).await
    }

    pub async fn revoke_device_by_id(
        &self,
        pilot_id: &str,
        device_id: &str,
    ) -> Result<bool, DbError> {
        let now = now_ms();
        self.end_active_flights_for_device(pilot_id, device_id, now)
            .await?;
        self.revoke_device_row(pilot_id, device_id).await
    }
}
