    pub async fn start_flight(
        &self,
        pilot_id: &str,
        device_id: &str,
        tail: &str,
        visibility: FlightVisibility,
        show_pilot_name_public: bool,
        now: i64,
    ) -> Result<String, StartFlightError> {
        self.start_flight_atomic(
            pilot_id,
            device_id,
            tail,
            visibility,
            show_pilot_name_public,
            now,
        )
        .await
    }

    pub async fn patch_flight(
        &self,
        pilot_id: &str,
        flight_id: &str,
        visibility: Option<FlightVisibility>,
        show_pilot_name_public: Option<bool>,
    ) -> Result<(), PatchError> {
        let owner = sqlx::query_scalar::<_, String>("SELECT pilot_id FROM flights WHERE id = $1")
            .bind(flight_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| PatchError::NotFound)?;
        let Some(owner) = owner else {
            return Err(PatchError::NotFound);
        };
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

    pub async fn end_flight(&self, pilot_id: &str, flight_id: &str, now: i64) -> Result<(), EndError> {
        let row = sqlx::query("SELECT pilot_id, status FROM flights WHERE id = $1")
            .bind(flight_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| EndError::NotFound)?;
        let Some(row) = row else {
            return Err(EndError::NotFound);
        };
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

    pub async fn device_pilot_id(&self, device_id: &str) -> Option<String> {
        sqlx::query_scalar("SELECT pilot_id FROM devices WHERE id = $1")
            .bind(device_id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    pub async fn resolve_device_token(&self, token: &str) -> Option<String> {
        let hash = digest_token(token);
        sqlx::query_scalar("SELECT id FROM devices WHERE token_hash = $1")
            .bind(hash)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
    }

    pub async fn open_flight_for_device(&self, device_id: &str) -> Option<String> {
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

    pub async fn append_fix(
        &self,
        flight_id: &str,
        device_id: &str,
        fix: &LiveFix,
    ) -> Result<(), DbError> {
        self.append_fixes(flight_id, device_id, std::slice::from_ref(fix))
            .await
            .map(|_| ())
    }

    pub async fn append_fixes(
        &self,
        flight_id: &str,
        device_id: &str,
        fixes: &[LiveFix],
    ) -> Result<usize, DbError> {
        if fixes.is_empty() {
            return Ok(0);
        }
        let mut tx = self.pool.begin().await?;
        let mut inserted = 0usize;
        let mut last_t = fixes[0].t;
        for fix in fixes {
            let changed = sqlx::query(
                "INSERT INTO fixes (flight_id, device_id, t_ms, received_at, lat, lon, alt, alt_ref, speed, course, accuracy)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                 ON CONFLICT (flight_id, device_id, t_ms) DO NOTHING",
            )
            .bind(flight_id)
            .bind(device_id)
            .bind(fix.t)
            .bind(now_ms())
            .bind(fix.lat)
            .bind(fix.lon)
            .bind(fix.alt)
            .bind(fix.alt_ref.as_deref())
            .bind(fix.speed)
            .bind(fix.course)
            .bind(fix.accuracy)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            inserted += usize::try_from(changed).unwrap_or(0);
            last_t = fix.t;
        }
        sqlx::query("UPDATE flights SET last_fix_at = $1 WHERE id = $2")
            .bind(last_t)
            .bind(flight_id)
            .execute(&mut *tx)
            .await?;
        if let Ok(pilot_id) = sqlx::query_scalar::<_, String>("SELECT pilot_id FROM flights WHERE id = $1")
            .bind(flight_id)
            .fetch_one(&mut *tx)
            .await
        {
            if let Some(last) = fixes.last() {
                let _ = sqlx::query(
                    "UPDATE pilots SET last_latitude = $1, last_longitude = $2, last_position_at = $3 WHERE id = $4",
                )
                .bind(last.lat)
                .bind(last.lon)
                .bind(last.t)
                .bind(pilot_id)
                .execute(&mut *tx)
                .await;
            }
        }
        tx.commit().await?;
        Ok(inserted)
    }