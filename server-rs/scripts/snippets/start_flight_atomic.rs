    pub async fn start_flight_atomic(
        &self,
        pilot_id: &str,
        device_id: &str,
        tail: &str,
        visibility: FlightVisibility,
        show_pilot_name_public: bool,
        now: i64,
    ) -> Result<String, StartFlightError> {
        let mut tx = self.pool.begin().await.map_err(|_| StartFlightError::NoTail)?;
        let result: Result<String, StartFlightError> = try {
            let dev_owner: Option<String> = sqlx::query_scalar(
                "SELECT pilot_id FROM devices WHERE id = $1",
            )
            .bind(device_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| StartFlightError::DeviceMismatch)?;
            if dev_owner.as_deref() != Some(pilot_id) {
                Err(StartFlightError::DeviceMismatch)?;
            }
            let checkin_club: String = sqlx::query_scalar(
                "SELECT club_id FROM checkins
                 WHERE outcome = 'ok' AND pilot_id = $1
                   AND expires_at IS NOT NULL AND expires_at > $2
                   AND (device_id IS NULL OR device_id = $3)
                 ORDER BY expires_at DESC LIMIT 1",
            )
            .bind(pilot_id)
            .bind(now)
            .bind(device_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoCheckin)?;
            let aircraft_id: Option<String> = sqlx::query_scalar(
                "SELECT id FROM aircraft WHERE tail = $1 AND club_id = $2 AND active = true",
            )
            .bind(tail)
            .bind(&checkin_club)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
            let aircraft_id = aircraft_id.ok_or(StartFlightError::NoTail)?;
            let type_code: String = sqlx::query_scalar(
                "SELECT t.code FROM aircraft a
                 JOIN aircraft_types t ON t.id = a.type_id
                 WHERE a.id = $1",
            )
            .bind(&aircraft_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
            if !Self::checkout_ok(&self.pool, pilot_id, &aircraft_id, now)
                .await
                .map_err(|_| StartFlightError::NoTail)?
            {
                Err(StartFlightError::NotCleared {
                    tail: tail.to_string(),
                    type_code,
                })?;
            }
            let pilot_active: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM flights WHERE pilot_id = $1 AND status = 'active'",
            )
            .bind(pilot_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
            if pilot_active > 0 {
                Err(StartFlightError::AlreadyActive)?;
            }
            let device_active: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM flights f
                 JOIN flight_devices fd ON fd.flight_id = f.id
                 WHERE fd.device_id = $1 AND f.status = 'active'",
            )
            .bind(device_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
            if device_active > 0 {
                Err(StartFlightError::AlreadyActive)?;
            }
            let club_id = checkin_club;
            let checkins: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM checkins
                 WHERE outcome = 'ok' AND pilot_id = $1 AND club_id = $2
                   AND expires_at IS NOT NULL AND expires_at > $3
                   AND (device_id IS NULL OR device_id = $4)",
            )
            .bind(pilot_id)
            .bind(&club_id)
            .bind(now)
            .bind(device_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoCheckin)?;
            if checkins == 0 {
                Err(StartFlightError::NoCheckin)?;
            }
            let flight_id = new_id();
            sqlx::query(
                "INSERT INTO flights (id, pilot_id, aircraft_id, club_id, status, started_at, visibility, show_pilot_name_public)
                 VALUES ($1, $2, $3, $4, 'active', $5, $6, $7)",
            )
            .bind(&flight_id)
            .bind(pilot_id)
            .bind(&aircraft_id)
            .bind(&club_id)
            .bind(now)
            .bind(visibility.as_str())
            .bind(show_pilot_name_public)
            .execute(&mut *tx)
            .await
            .map_err(|_| StartFlightError::NoTail)?;
            sqlx::query("INSERT INTO flight_devices (flight_id, device_id) VALUES ($1, $2)")
                .bind(&flight_id)
                .bind(device_id)
                .execute(&mut *tx)
                .await
                .map_err(|_| StartFlightError::NoTail)?;
            flight_id
        };
        match &result {
            Ok(_) => tx.commit().await.map_err(|_| StartFlightError::NoTail)?,
            Err(_) => {
                let _ = tx.rollback().await;
            }
        }
        result
    }