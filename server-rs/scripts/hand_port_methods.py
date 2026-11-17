#!/usr/bin/env python3
"""Replace broken sqlite remnants in db.rs with hand-written async sqlx methods."""
from pathlib import Path
import replace_method as rm

ROOT = Path(__file__).resolve().parents[1]
DB = ROOT / "src/club/db.rs"

HAND = r"""
    pub async fn get_flight(&self, flight_id: &str) -> Option<FlightRow> {
        let row = sqlx::query(
            "SELECT f.id, f.pilot_id, f.aircraft_id, f.status, f.started_at, f.ended_at, f.last_fix_at,
                        f.visibility, f.show_pilot_name_public, a.tail, p.name
                 FROM flights f
                 JOIN aircraft a ON a.id = f.aircraft_id
                 JOIN pilots p ON p.id = f.pilot_id
                 WHERE f.id = $1",
        )
        .bind(flight_id)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        Some(FlightRow {
            id: row.try_get(0).ok()?,
            pilot_id: row.try_get(1).ok()?,
            aircraft_id: row.try_get(2).ok()?,
            status: row.try_get(3).ok()?,
            started_at: row.try_get(4).ok()?,
            ended_at: row.try_get(5).ok()?,
            last_fix_at: row.try_get(6).ok()?,
            visibility: row.try_get(7).ok()?,
            show_pilot_name_public: row.try_get::<_, i64>(8).ok()? != 0,
            tail: row.try_get(9).ok()?,
            pilot_name: row.try_get(10).ok()?,
        })
    }

    pub async fn flight_visible_to(&self, viewer: &Viewer, flight_id: &str) -> Option<FlightRow> {
        let flight = self.get_flight(flight_id).await?;
        let vis = FlightVisibility::parse(&flight.visibility).ok()?;
        if can_view_flight(viewer, &flight.pilot_id, vis) {
            Some(flight)
        } else {
            None
        }
    }

    pub async fn fixes_for_flight(&self, flight_id: &str) -> Vec<LiveFix> {
        let rows = sqlx::query(
            "SELECT t_ms, lat, lon, alt, speed, course, accuracy, alt_ref
             FROM fixes WHERE flight_id = $1 ORDER BY t_ms ASC",
        )
        .bind(flight_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(LiveFix {
                    t: r.try_get(0).ok()?,
                    lat: r.try_get(1).ok()?,
                    lon: r.try_get(2).ok()?,
                    alt: r.try_get(3).ok()?,
                    alt_ref: r.try_get(4).ok()?,
                    speed: r.try_get(5).ok()?,
                    course: r.try_get(6).ok()?,
                    accuracy: r.try_get(7).ok()?,
                })
            })
            .collect()
    }

    async fn latest_device_fixes(&self) -> Vec<DeviceFix> {
        let rows = sqlx::query(
            "SELECT f.id, a.tail, f.pilot_id, p.name, f.visibility,
                    fx.t_ms, fx.lat, fx.lon, fx.alt, fx.speed, fx.course, fx.accuracy
             FROM flights f
             JOIN aircraft a ON a.id = f.aircraft_id
             JOIN pilots p ON p.id = f.pilot_id
             JOIN fixes fx ON fx.flight_id = f.id
             WHERE f.status = 'active'
               AND fx.t_ms = (
                 SELECT MAX(fx2.t_ms) FROM fixes fx2 WHERE fx2.flight_id = f.id
               )",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(DeviceFix {
                    flight_id: r.try_get(0).ok()?,
                    tail: r.try_get(1).ok()?,
                    pilot_id: r.try_get(2).ok()?,
                    pilot_name: r.try_get(3).ok()?,
                    visibility: r.try_get(4).ok()?,
                    fix: LiveFix {
                        t: r.try_get(5).ok()?,
                        lat: r.try_get(6).ok()?,
                        lon: r.try_get(7).ok()?,
                        alt: r.try_get(8).ok()?,
                        alt_ref: None,
                        speed: r.try_get(9).ok()?,
                        course: r.try_get(10).ok()?,
                        accuracy: r.try_get(11).ok()?,
                    },
                })
            })
            .collect()
    }

    pub async fn club_map_for_viewer(&self, viewer: &Viewer) -> Vec<TailLivePosition> {
        let rows = self.latest_device_fixes().await;
        let filtered: Vec<DeviceFix> = rows
            .into_iter()
            .filter(|r| {
                let vis =
                    FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club);
                includes_in_club_map(viewer, &r.pilot_id, vis)
                    && can_view_flight(viewer, &r.pilot_id, vis)
            })
            .collect();
        let mut positions = dedupe_by_tail(filtered);
        for pos in &mut positions {
            if let Some(f) = self.get_flight(&pos.flight_id).await {
                pos.pilot_name = Some(f.pilot_name);
            }
        }
        positions
    }

    pub async fn public_map(&self) -> Vec<TailLivePosition> {
        let rows: Vec<DeviceFix> = self
            .latest_device_fixes()
            .await
            .into_iter()
            .filter(|r| {
                let vis =
                    FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club);
                includes_in_public_map(vis)
            })
            .collect();
        let mut positions = dedupe_by_tail(rows);
        for pos in &mut positions {
            if let Some(f) = self.get_flight(&pos.flight_id).await {
                if show_pilot_name(&Viewer::Anonymous, &f.pilot_id, f.show_pilot_name_public) {
                    pos.pilot_name = Some(f.pilot_name);
                } else {
                    pos.pilot_name = None;
                }
            }
        }
        positions
    }

    pub async fn admin_map(&self, admin_id: &str) -> Vec<TailLivePosition> {
        let viewer = Viewer::Pilot {
            id: admin_id.to_string(),
            status: PilotStatus::Approved,
            role: PilotRole::Admin,
        };
        let all = self.club_map_for_viewer(&viewer).await;
        let tails = self.admin_map_tails(admin_id).await;
        filter_admin_tails(all, &tails)
    }

    pub async fn admin_map_tails(&self, admin_id: &str) -> Vec<String> {
        let json: String = sqlx::query_scalar(
            "SELECT tails_json FROM admin_map_prefs WHERE pilot_id = $1",
        )
        .bind(admin_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| "[]".to_string());
        serde_json::from_str(&json).unwrap_or_default()
    }

    pub async fn device_pair_token_for_pilot(
        &self,
        device_id: &str,
        pilot_id: &str,
    ) -> Option<String> {
        sqlx::query_scalar(
            "SELECT dpt.token FROM device_pair_tokens dpt
             JOIN devices d ON d.id = dpt.device_id
             WHERE dpt.device_id = $1 AND d.pilot_id = $2",
        )
        .bind(device_id)
        .bind(pilot_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }

    pub async fn list_my_devices(&self, pilot_id: &str) -> Vec<DeviceInfo> {
        let rows = sqlx::query(
            "SELECT id, label, created_at FROM devices WHERE pilot_id = $1",
        )
        .bind(pilot_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(DeviceInfo {
                    id: r.try_get(0).ok()?,
                    label: r.try_get(1).ok()?,
                    created_at: r.try_get(2).ok()?,
                })
            })
            .collect()
    }

    pub async fn list_my_flights(&self, pilot_id: &str) -> Vec<FlightSummary> {
        let rows = sqlx::query(
            "SELECT f.id, a.tail, f.status, f.started_at, f.ended_at, f.visibility, f.show_pilot_name_public
             FROM flights f JOIN aircraft a ON a.id = f.aircraft_id
             WHERE f.pilot_id = $1 ORDER BY f.started_at DESC",
        )
        .bind(pilot_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(FlightSummary {
                    id: r.try_get(0).ok()?,
                    tail: r.try_get(1).ok()?,
                    status: r.try_get(2).ok()?,
                    started_at: r.try_get(3).ok()?,
                    ended_at: r.try_get(4).ok()?,
                    visibility: r.try_get(5).ok()?,
                    show_pilot_name_public: r.try_get::<_, i64>(6).ok()? != 0,
                })
            })
            .collect()
    }

    pub async fn delete_my_flight(&self, pilot_id: &str, flight_id: &str) -> Result<(), DeleteError> {
        let owner: String = match sqlx::query_scalar("SELECT pilot_id FROM flights WHERE id = $1")
            .bind(flight_id)
            .fetch_optional(&self.pool)
            .await
        {
            Ok(Some(o)) => o,
            Ok(None) => return Err(DeleteError::NotFound),
            Err(e) => return Err(DeleteError::Db(e)),
        };
        if owner != pilot_id {
            return Err(DeleteError::Forbidden);
        }
        sqlx::query("DELETE FROM flights WHERE id = $1")
            .bind(flight_id)
            .execute(&self.pool)
            .await
            .map_err(DeleteError::Db)?;
        Ok(())
    }

    pub async fn upsert_type(&self, club_id: &str, code: &str) -> Result<String, DbError> {
        if let Some(id) = sqlx::query_scalar::<_, String>(
            "SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2",
        )
        .bind(club_id)
        .bind(code)
        .fetch_optional(&self.pool)
        .await?
        {
            return Ok(id);
        }
        let id = new_id();
        sqlx::query(
            "INSERT INTO aircraft_types (id, club_id, code) VALUES ($1, $2, $3)",
        )
        .bind(&id)
        .bind(club_id)
        .bind(code)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn upsert_aircraft(
        &self,
        club_id: &str,
        tail: &str,
        type_code: &str,
        active: bool,
    ) -> Result<String, DbError> {
        let type_id: String = sqlx::query_scalar(
            "SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2",
        )
        .bind(club_id)
        .bind(type_code)
        .fetch_one(&self.pool)
        .await?;
        if let Some(id) = sqlx::query_scalar::<_, String>(
            "SELECT id FROM aircraft WHERE club_id = $1 AND tail = $2",
        )
        .bind(club_id)
        .bind(tail)
        .fetch_optional(&self.pool)
        .await?
        {
            sqlx::query("UPDATE aircraft SET type_id = $1, active = $2 WHERE id = $3")
                .bind(&type_id)
                .bind(i64::from(active))
                .bind(&id)
                .execute(&self.pool)
                .await?;
            return Ok(id);
        }
        let id = new_id();
        sqlx::query(
            "INSERT INTO aircraft (id, club_id, tail, type_id, active) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&id)
        .bind(club_id)
        .bind(tail)
        .bind(&type_id)
        .bind(i64::from(active))
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn list_pending_pilots(&self) -> Vec<PilotRow> {
        let rows = sqlx::query(
            "SELECT id, club_id, name, email, license_no, status, role
             FROM pilots WHERE status = 'pending'",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(PilotRow {
                    id: r.try_get(0).ok()?,
                    club_id: r.try_get(1).ok()?,
                    name: r.try_get(2).ok()?,
                    email: r.try_get(3).ok()?,
                    license_no: r.try_get(4).ok()?,
                    status: r.try_get(5).ok()?,
                    role: r.try_get(6).ok()?,
                })
            })
            .collect()
    }

    pub async fn list_types(&self, club_id: &str) -> Vec<TypeRow> {
        let rows = sqlx::query(
            "SELECT id, code FROM aircraft_types WHERE club_id = $1 ORDER BY code",
        )
        .bind(club_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(TypeRow {
                    id: r.try_get(0).ok()?,
                    code: r.try_get(1).ok()?,
                })
            })
            .collect()
    }

    pub async fn list_aircraft(&self, club_id: &str) -> Vec<AircraftRow> {
        let rows = sqlx::query(
            "SELECT a.id, a.tail, t.code, a.active
             FROM aircraft a JOIN aircraft_types t ON t.id = a.type_id
             WHERE a.club_id = $1 ORDER BY a.tail",
        )
        .bind(club_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(AircraftRow {
                    id: r.try_get(0).ok()?,
                    tail: r.try_get(1).ok()?,
                    type_code: r.try_get(2).ok()?,
                    active: r.try_get::<_, i64>(3).ok()? != 0,
                })
            })
            .collect()
    }

    pub async fn list_all_pilots(&self) -> Vec<PilotRow> {
        let rows = sqlx::query(
            "SELECT id, club_id, name, email, license_no, status, role
             FROM pilots ORDER BY email",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(PilotRow {
                    id: r.try_get(0).ok()?,
                    club_id: r.try_get(1).ok()?,
                    name: r.try_get(2).ok()?,
                    email: r.try_get(3).ok()?,
                    license_no: r.try_get(4).ok()?,
                    status: r.try_get(5).ok()?,
                    role: r.try_get(6).ok()?,
                })
            })
            .collect()
    }

    pub async fn list_checkouts(&self) -> Vec<CheckoutRow> {
        let rows = sqlx::query(
            "SELECT c.id, c.pilot_id, p.name, p.email, t.code, a.tail, c.expires_at
             FROM checkouts c
             JOIN pilots p ON p.id = c.pilot_id
             JOIN aircraft_types t ON t.id = c.type_id
             LEFT JOIN aircraft a ON a.id = c.aircraft_id
             ORDER BY p.email, t.code",
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(CheckoutRow {
                    id: r.try_get(0).ok()?,
                    pilot_id: r.try_get(1).ok()?,
                    pilot_name: r.try_get(2).ok()?,
                    pilot_email: r.try_get(3).ok()?,
                    type_code: r.try_get(4).ok()?,
                    tail: r.try_get(5).ok()?,
                    expires_at: r.try_get(6).ok()?,
                })
            })
            .collect()
    }

    pub async fn pilot_club_id(&self, pilot_id: &str) -> Result<String, DbError> {
        sqlx::query_scalar("SELECT club_id FROM pilots WHERE id = $1")
            .bind(pilot_id)
            .fetch_one(&self.pool)
            .await
    }

    pub async fn club_checkin_settings(&self, club_id: &str) -> Result<(String, i64), DbError> {
        let row = sqlx::query(
            "SELECT checkin_hmac_secret, checkin_valid_minutes FROM clubs WHERE id = $1",
        )
        .bind(club_id)
        .fetch_one(&self.pool)
        .await?;
        let secret: String = row.try_get(0)?;
        let minutes: i64 = row.try_get(1)?;
        let secret = if secret.is_empty() {
            self.ensure_checkin_secret(club_id).await?
        } else {
            secret
        };
        Ok((secret, minutes.max(1)))
    }

    pub async fn ensure_checkin_secret(&self, club_id: &str) -> Result<String, DbError> {
        if let Ok(env) = std::env::var("FLIGHT_CLUB_CHECKIN_SECRET") {
            if !env.is_empty() {
                sqlx::query("UPDATE clubs SET checkin_hmac_secret = $1 WHERE id = $2")
                    .bind(&env)
                    .bind(club_id)
                    .execute(&self.pool)
                    .await?;
                return Ok(env);
            }
        }
        let secret = new_device_token();
        sqlx::query("UPDATE clubs SET checkin_hmac_secret = $1 WHERE id = $2")
            .bind(&secret)
            .bind(club_id)
            .execute(&self.pool)
            .await?;
        Ok(secret)
    }

    async fn insert_checkin_row(
        &self,
        club_id: &str,
        pilot_id: Option<&str>,
        device_id: Option<&str>,
        time_step: i64,
        token_digest: &str,
        now: i64,
        expires_at: Option<i64>,
        outcome: &str,
        fail_reason: Option<&str>,
    ) -> Result<String, DbError> {
        let id = new_id();
        sqlx::query(
            "INSERT INTO checkins (id, club_id, pilot_id, device_id, time_step, token_digest, created_at, expires_at, outcome, fail_reason)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(&id)
        .bind(club_id)
        .bind(pilot_id)
        .bind(device_id)
        .bind(time_step)
        .bind(token_digest)
        .bind(now)
        .bind(expires_at)
        .bind(outcome)
        .bind(fail_reason)
        .execute(&self.pool)
        .await?;
        Ok(id)
    }

    pub async fn mint_checkin_qr_token(&self, club_id: &str, now: i64) -> Result<String, DbError> {
        let (secret, _) = self.club_checkin_settings(club_id).await?;
        let step = checkin::current_step(now);
        Ok(checkin::mint_checkin_token(&secret, club_id, step))
    }

    async fn checkin_replay(
        &self,
        pilot_id: &str,
        device_id: Option<&str>,
        token_digest: &str,
    ) -> Result<bool, DbError> {
        let count: i64 = if let Some(dev) = device_id {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM checkins WHERE outcome = 'ok' AND pilot_id = $1 AND device_id = $2 AND token_digest = $3",
            )
            .bind(pilot_id)
            .bind(dev)
            .bind(token_digest)
            .fetch_one(&self.pool)
            .await?
        } else {
            sqlx::query_scalar(
                "SELECT COUNT(*) FROM checkins WHERE outcome = 'ok' AND pilot_id = $1 AND device_id IS NULL AND token_digest = $2",
            )
            .bind(pilot_id)
            .bind(token_digest)
            .fetch_one(&self.pool)
            .await?
        };
        Ok(count > 0)
    }

    pub async fn submit_checkin(
        &self,
        pilot_id: &str,
        device_id: Option<&str>,
        token: &str,
        now: i64,
    ) -> Result<CheckinRecord, CheckinReject> {
        let pilot_club = self
            .pilot_club_id(pilot_id)
            .await
            .map_err(|_| CheckinReject::WrongClub)?;
        let (secret, valid_min) = self
            .club_checkin_settings(&pilot_club)
            .await
            .map_err(|_| CheckinReject::ForgedHmac)?;
        let digest = checkin::token_digest(token);
        let record_fail = |reason: &str| async {
            let _ = self
                .insert_checkin_row(
                    &pilot_club,
                    Some(pilot_id),
                    device_id,
                    0,
                    &digest,
                    now,
                    None,
                    "fail",
                    Some(reason),
                )
                .await;
        };
        let (club_id, step) = match checkin::verify_checkin_token(&secret, token, now) {
            Ok(v) => v,
            Err(CheckinTokenError::Format) => {
                record_fail("format").await;
                return Err(CheckinReject::Format);
            }
            Err(CheckinTokenError::Forged) => {
                record_fail("forged_hmac").await;
                return Err(CheckinReject::ForgedHmac);
            }
            Err(CheckinTokenError::OldStep) => {
                record_fail("old_step").await;
                return Err(CheckinReject::OldStep);
            }
        };
        if club_id != pilot_club {
            record_fail("wrong_club").await;
            return Err(CheckinReject::WrongClub);
        }
        if self
            .checkin_replay(pilot_id, device_id, &digest)
            .await
            .unwrap_or(false)
        {
            record_fail("replay").await;
            return Err(CheckinReject::Replay);
        }
        let expires = now + valid_min * 60_000;
        let id = self
            .insert_checkin_row(
                &pilot_club,
                Some(pilot_id),
                device_id,
                step,
                &digest,
                now,
                Some(expires),
                "ok",
                None,
            )
            .await
            .map_err(|_| CheckinReject::ForgedHmac)?;
        Ok(CheckinRecord {
            id,
            created_at: now,
            expires_at: expires,
        })
    }

    pub async fn consume_checkin(
        &self,
        pilot_id: &str,
        device_id: &str,
        club_id: &str,
        now: i64,
    ) -> Result<bool, DbError> {
        let checkin_id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM checkins
             WHERE outcome = 'ok' AND pilot_id = $1 AND club_id = $2
               AND consumed_at IS NULL AND expires_at IS NOT NULL AND expires_at > $3
               AND (device_id IS NULL OR device_id = $4)
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(pilot_id)
        .bind(club_id)
        .bind(now)
        .bind(device_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(id) = checkin_id else {
            return Ok(false);
        };
        sqlx::query("UPDATE checkins SET consumed_at = $1 WHERE id = $2 AND consumed_at IS NULL")
            .bind(now)
            .bind(&id)
            .execute(&self.pool)
            .await?;
        Ok(true)
    }

    pub async fn issue_open_checkin(
        &self,
        pilot_id: &str,
        device_id: &str,
        club_id: &str,
        now: i64,
    ) -> Result<(), DbError> {
        let (_, valid_min) = self.club_checkin_settings(club_id).await?;
        let expires = now + valid_min * 60_000;
        let digest = checkin::token_digest(&format!("internal:{pilot_id}:{now}"));
        let _ = self
            .insert_checkin_row(
                club_id,
                Some(pilot_id),
                Some(device_id),
                checkin::current_step(now),
                &digest,
                now,
                Some(expires),
                "ok",
                None,
            )
            .await?;
        Ok(())
    }

    pub async fn create_display_session(
        &self,
        admin_id: &str,
        label: &str,
    ) -> Result<(String, String), DbError> {
        let club_id = self.pilot_club_id(admin_id).await?;
        let token = new_device_token();
        let hash = digest_token(&token);
        let id = new_id();
        let now = now_ms();
        sqlx::query(
            "INSERT INTO display_sessions (id, club_id, created_by_pilot_id, token_hash, label, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&id)
        .bind(&club_id)
        .bind(admin_id)
        .bind(&hash)
        .bind(label)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok((id, token))
    }

    pub async fn revoke_display_session(
        &self,
        admin_id: &str,
        session_id: &str,
    ) -> Result<bool, DbError> {
        let club_id = self.pilot_club_id(admin_id).await?;
        let updated = sqlx::query(
            "UPDATE display_sessions SET revoked_at = $1
             WHERE id = $2 AND club_id = $3 AND created_by_pilot_id = $4 AND revoked_at IS NULL",
        )
        .bind(now_ms())
        .bind(session_id)
        .bind(&club_id)
        .bind(admin_id)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() > 0)
    }

    pub async fn resolve_display_session(&self, token: &str) -> Option<(String, String, String)> {
        let hash = digest_token(token);
        let row = sqlx::query(
            "SELECT id, club_id, created_by_pilot_id FROM display_sessions
             WHERE token_hash = $1 AND revoked_at IS NULL",
        )
        .bind(hash)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        Some((
            row.try_get(0).ok()?,
            row.try_get(1).ok()?,
            row.try_get(2).ok()?,
        ))
    }

    pub async fn list_display_sessions(&self, admin_id: &str) -> Vec<DisplaySessionRow> {
        let rows = sqlx::query(
            "SELECT id, label, created_at, revoked_at FROM display_sessions
             WHERE created_by_pilot_id = $1 ORDER BY created_at DESC",
        )
        .bind(admin_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                let revoked_at: Option<i64> = r.try_get(3).ok()?;
                Some(DisplaySessionRow {
                    id: r.try_get(0).ok()?,
                    label: r.try_get(1).ok()?,
                    created_at: r.try_get(2).ok()?,
                    revoked: revoked_at.is_some(),
                })
            })
            .collect()
    }

    pub async fn display_map(&self, admin_creator_id: &str) -> Vec<TailLivePosition> {
        let rows: Vec<DeviceFix> = self
            .latest_device_fixes()
            .await
            .into_iter()
            .filter(|r| {
                let vis =
                    FlightVisibility::parse(&r.visibility).unwrap_or(FlightVisibility::Club);
                matches!(vis, FlightVisibility::Club | FlightVisibility::Public)
            })
            .collect();
        let mut positions = dedupe_by_tail(rows);
        for pos in &mut positions {
            if let Some(f) = self.get_flight(&pos.flight_id).await {
                if show_pilot_name(&Viewer::Anonymous, &f.pilot_id, f.show_pilot_name_public) {
                    pos.pilot_name = Some(f.pilot_name);
                } else {
                    pos.pilot_name = None;
                }
            }
        }
        let prefs = self.admin_map_tails(admin_creator_id).await;
        filter_admin_tails(positions, &prefs)
    }

    pub async fn seed_demo_if_empty(&self) -> Result<(), DbError> {
        let club_id = self.default_club_id().await?;
        let exists: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM pilots WHERE email = 'admin@demo.test'")
                .fetch_one(&self.pool)
                .await?;
        if exists > 0 {
            return Ok(());
        }
        let admin_id = new_id();
        let pilot_id = new_id();
        let pending_id = new_id();
        let hash = auth::hash_password("password12")
            .map_err(|_| DbError::InvalidParameterName("hash".into()))?;
        let t = now_ms();
        sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at) VALUES
             ($1, $4, 'Demo Admin', 'admin@demo.test', 'ADM-1', 'approved', 'admin', $5, $6),
             ($2, $4, 'Demo Pilot', 'pilot@demo.test', 'P-1', 'approved', 'pilot', $5, $6),
             ($3, $4, 'Demo Pending', 'pending@demo.test', 'P-2', 'pending', 'pilot', $5, $6)",
        )
        .bind(&admin_id)
        .bind(&pilot_id)
        .bind(&pending_id)
        .bind(&club_id)
        .bind(&hash)
        .bind(t)
        .execute(&self.pool)
        .await?;
        for code in ["DA20-C1", "C152"] {
            self.upsert_type(&club_id, code).await?;
        }
        for tail in ["N101", "N202", "N303"] {
            self.upsert_aircraft(&club_id, tail, "C152", true).await?;
        }
        self.add_checkout(&pilot_id, "C152", None, None).await?;
        self.add_checkout(&admin_id, "C152", None, None).await?;
        let (dev_pilot, _) = self.create_device(&pilot_id, "Pilot phone").await?;
        let (dev_admin, _) = self.create_device(&admin_id, "Admin tablet").await?;
        self.issue_open_checkin(&pilot_id, &dev_pilot, &club_id, t)
            .await?;
        self.issue_open_checkin(&admin_id, &dev_admin, &club_id, t)
            .await?;
        let seed_flight_err = |e: StartFlightError| {
            DbError::InvalidParameterName(format!("seed_demo flight: {e:?}"))
        };
        let pub_id = self
            .start_flight(
                &pilot_id,
                &dev_pilot,
                "N101",
                FlightVisibility::Public,
                true,
                t,
            )
            .await
            .map_err(seed_flight_err)?;
        let club_f = self
            .start_flight(
                &admin_id,
                &dev_admin,
                "N202",
                FlightVisibility::Club,
                false,
                t,
            )
            .await
            .map_err(seed_flight_err)?;
        let fix = |offset: i64, lat: f64| LiveFix {
            t: t + offset,
            lat,
            lon: -70.6,
            alt: Some(900.0),
            alt_ref: None,
            speed: Some(28.0),
            course: Some(120.0),
            accuracy: Some(6.0),
        };
        self.append_fix(&pub_id, &dev_pilot, &fix(100, -33.40))
            .await?;
        self.append_fix(&pub_id, &dev_pilot, &fix(200, -33.39))
            .await?;
        self.append_fix(&club_f, &dev_admin, &fix(150, -33.38))
            .await?;
        self.append_fix(&club_f, &dev_admin, &fix(250, -33.37))
            .await?;
        Ok(())
    }
"""


def splice_impl(db: str, new_tail: str) -> str:
    start_marker = "\n    pub fn get_flight"
    alt = "\n    pub async fn get_flight"
    idx = db.find(start_marker)
    if idx < 0:
        idx = db.find(alt)
    if idx < 0:
        raise SystemExit("get_flight not found")
    end_marker = "\n}\n\npub fn now_ms()"
    end = db.find(end_marker, idx)
    if end < 0:
        raise SystemExit("impl end not found")
    return db[:idx] + "\n" + new_tail.strip() + "\n" + db[end:]


def main() -> None:
    db = DB.read_text()
    # fix auto_end_idle before splice (it's before get_flight)
    db = db.replace(
        "            Ok(updated)\n        \n    }\n\n    pub fn get_flight",
        "            Ok(updated.rows_affected() as usize)\n    }\n\n    pub fn get_flight",
    )
    db = splice_impl(db, HAND)
  # fix viewer helpers
    db = db.replace(
        "        let viewer = self.viewer_from_session(Some(session_id), now);",
        "        let viewer = self.viewer_from_session(Some(session_id), now).await;",
    )
    db = db.replace(
        "pub async fn viewer_from_session",
        "    pub async fn viewer_from_session",
    )
    db = db.replace(
        "pub async fn approve_pilot",
        "    pub async fn approve_pilot",
    )
    DB.write_text(db)
    left = db.count("conn.")
    params = db.count("params!")
    with_conn = db.count("with_conn")
    print("done conn", left, "params", params, "with_conn", with_conn)


if __name__ == "__main__":
    main()