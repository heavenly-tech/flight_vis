#!/usr/bin/env python3
from pathlib import Path

def swap(text: str, start_marker: str, end_marker: str, block: str) -> str:
    start = text.index(start_marker)
    end = text.index(end_marker, start)
    return text[:start] + block + text[end:]

DB = Path(__file__).resolve().parents[1] / "src/club/db_pg.rs"
text = DB.read_text()

text = swap(
    text,
    "    pub async fn patch_flight(",
    "    pub async fn end_flight(",
    '''    pub async fn patch_flight(
        &self,
        pilot_id: &str,
        flight_id: &str,
        visibility: Option<FlightVisibility>,
        show_pilot_name_public: Option<bool>,
    ) -> Result<(), PatchError> {
        let owner: String = sqlx::query_scalar("SELECT pilot_id FROM flights WHERE id = $1")
            .bind(flight_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| PatchError::NotFound)?;
        if owner != pilot_id {
            return Err(PatchError::Forbidden);
        }
        if let Some(v) = visibility {
            sqlx::query("UPDATE flights SET visibility = $1 WHERE id = $2")
                .bind(v.as_str())
                .bind(flight_id)
                .execute(&self.pool)
                .await
                .map_err(|_| PatchError::NotFound)?;
        }
        if let Some(flag) = show_pilot_name_public {
            sqlx::query("UPDATE flights SET show_pilot_name_public = $1 WHERE id = $2")
                .bind(flag)
                .bind(flight_id)
                .execute(&self.pool)
                .await
                .map_err(|_| PatchError::NotFound)?;
        }
        Ok(())
    }

''',
)

text = swap(
    text,
    "    pub async fn end_flight(",
    "    pub async fn device_pilot_id(",
    '''    pub async fn end_flight(&self, pilot_id: &str, flight_id: &str, now: i64) -> Result<(), EndError> {
        let row = sqlx::query("SELECT pilot_id, status FROM flights WHERE id = $1")
            .bind(flight_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| EndError::NotFound)?;
        let owner: String = row.try_get(0).map_err(|_| EndError::NotFound)?;
        let status: String = row.try_get(1).map_err(|_| EndError::NotFound)?;
        if owner != pilot_id {
            return Err(EndError::Forbidden);
        }
        if status != "active" {
            return Err(EndError::NotActive);
        }
        sqlx::query("UPDATE flights SET status = 'ended', ended_at = $1 WHERE id = $2")
            .bind(now)
            .bind(flight_id)
            .execute(&self.pool)
            .await
            .map_err(|_| EndError::NotFound)?;
        Ok(())
    }

''',
)

text = swap(
    text,
    "    pub async fn device_pilot_id(",
    "    pub async fn resolve_device_token(",
    '''    pub async fn device_pilot_id(&self, device_id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT pilot_id FROM devices WHERE id = $1")
            .bind(device_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

''',
)

text = swap(
    text,
    "    pub async fn resolve_device_token(",
    "    pub async fn open_flight_for_device(",
    '''    pub async fn resolve_device_token(&self, token: &str) -> Option<String> {
        let hash = digest_token(token);
        sqlx::query_scalar("SELECT device_id FROM devices WHERE token_hash = $1")
            .bind(hash)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

''',
)

text = swap(
    text,
    "    pub async fn open_flight_for_device(",
    "    pub async fn append_fix(",
    '''    pub async fn open_flight_for_device(&self, device_id: &str) -> Option<String> {
        sqlx::query_scalar(
            "SELECT f.id FROM flights f
             JOIN flight_devices fd ON fd.flight_id = f.id
             WHERE fd.device_id = $1 AND f.status = 'active'
             ORDER BY f.started_at DESC LIMIT 1",
        )
        .bind(device_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

''',
)

text = swap(
    text,
    "    pub async fn me_tails(",
    "    pub async fn start_flight(",
    '''    pub async fn me_tails(&self, pilot_id: &str, now: i64) -> Result<Vec<TailInfo>, DbError> {
        let rows = sqlx::query(
            "SELECT a.id, a.tail, t.code FROM aircraft a
             JOIN aircraft_types t ON t.id = a.type_id
             WHERE a.active = TRUE",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut out = Vec::new();
        for row in rows {
            let id: String = row.try_get(0)?;
            let tail: String = row.try_get(1)?;
            let type_code: String = row.try_get(2)?;
            if Self::checkout_ok(&self.pool, pilot_id, &id, now).await? {
                out.push(TailInfo { id, tail, type_code });
            }
        }
        Ok(out)
    }

''',
)

DB.write_text(text)
print("patched core flight helpers")