#!/usr/bin/env python3
from pathlib import Path

DB = Path(__file__).resolve().parents[1] / "src/club/db_pg.rs"
text = DB.read_text()
start = text.index("    pub async fn start_flight(")
end = text.index("    pub async fn patch_flight(", start)
block = '''    pub async fn start_flight(
        &self,
        pilot_id: &str,
        device_id: &str,
        tail: &str,
        visibility: FlightVisibility,
        show_pilot_name_public: bool,
        now: i64,
    ) -> Result<String, StartFlightError> {
        let aircraft_id = sqlx::query_scalar::<_, String>(
            "SELECT id FROM aircraft WHERE tail = $1 AND active = TRUE",
        )
        .bind(tail)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| StartFlightError::NoTail)?;
        let Some(aircraft_id) = aircraft_id else {
            return Err(StartFlightError::NoTail);
        };
        let type_code: String = sqlx::query_scalar(
            "SELECT t.code FROM aircraft a JOIN aircraft_types t ON t.id = a.type_id WHERE a.id = $1",
        )
        .bind(&aircraft_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| StartFlightError::NoTail)?;
        let checkout = Self::checkout_ok(&self.pool, pilot_id, &aircraft_id, now)
            .await
            .unwrap_or(false);
        if !checkout {
            return Err(StartFlightError::NotCleared {
                tail: tail.to_string(),
                type_code,
            });
        }
        let open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM flights WHERE pilot_id = $1 AND status = 'active'",
        )
        .bind(pilot_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        if open > 0 {
            return Err(StartFlightError::AlreadyActive);
        }
        let club_id: String = sqlx::query_scalar("SELECT club_id FROM aircraft WHERE id = $1")
            .bind(&aircraft_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
        if self.device_pilot_id(device_id).await.as_deref() != Some(pilot_id) {
            return Err(StartFlightError::DeviceMismatch);
        }
        if !self
            .consume_checkin(pilot_id, device_id, &club_id, now)
            .await
            .map_err(|_| StartFlightError::NoCheckin)?
        {
            return Err(StartFlightError::NoCheckin);
        }
        let flight_id = new_id();
        sqlx::query(
            "INSERT INTO flights (id, pilot_id, aircraft_id, status, started_at, visibility, show_pilot_name_public)
             VALUES ($1, $2, $3, 'active', $4, $5, $6)",
        )
        .bind(&flight_id)
        .bind(pilot_id)
        .bind(&aircraft_id)
        .bind(now)
        .bind(visibility.as_str())
        .bind(show_pilot_name_public)
        .execute(&self.pool)
        .await
        .map_err(|_| StartFlightError::NoTail)?;
        sqlx::query("INSERT INTO flight_devices (flight_id, device_id) VALUES ($1, $2)")
            .bind(&flight_id)
            .bind(device_id)
            .execute(&self.pool)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
        Ok(flight_id)
    }

'''
DB.write_text(text[:start] + block + text[end:])
print("patched start_flight")