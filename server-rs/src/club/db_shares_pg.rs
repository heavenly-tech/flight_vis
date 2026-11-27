use crate::club::{
    auth::{digest_token, new_id, new_share_token},
    db::{
        now_ms, ClubDb, FlightRow, FlightShareListItem, FlightShareRow, ResolvedShare,
        ShareCreateError, ShareRevokeError, SHARE_LIFETIME_AFTER_END_MS,
    },
    map::{TAIL_SOURCE_CLUB, TAIL_SOURCE_PILOT},
};
use sqlx::Row;
use subtle::ConstantTimeEq;

#[allow(clippy::too_many_arguments)]
fn row_from_query(
    id: String,
    flight_id: String,
    club_id: Option<String>,
    created_by: String,
    token_hash: String,
    created_at: i64,
    expires_at: Option<i64>,
    revoked_at: Option<i64>,
) -> FlightShareRow {
    FlightShareRow {
        id,
        flight_id,
        club_id,
        created_by,
        token_hash,
        created_at,
        expires_at,
        revoked_at,
    }
}

fn share_active_at(share: &FlightShareRow, flight: &FlightRow, now: i64) -> bool {
    if share.revoked_at.is_some() {
        return false;
    }
    if let Some(exp) = share.expires_at {
        if now > exp {
            return false;
        }
    }
    if let Some(ended) = flight.ended_at {
        let deadline = ended + SHARE_LIFETIME_AFTER_END_MS;
        if share.expires_at.is_none() && now > deadline {
            return false;
        }
    }
    true
}

pub(crate) fn token_hash_matches(stored: &str, candidate_digest: &str) -> bool {
    if stored.len() != candidate_digest.len() {
        return false;
    }
    stored.as_bytes().ct_eq(candidate_digest.as_bytes()).into()
}

impl ClubDb {
    pub async fn create_flight_share(
        &self,
        pilot_id: &str,
        flight_id: &str,
    ) -> Result<(FlightShareRow, String), ShareCreateError> {
        let flight = self
            .get_flight(flight_id)
            .await
            .ok_or(ShareCreateError::NotFound)?;
        if flight.pilot_id != pilot_id {
            return Err(ShareCreateError::NotOwner);
        }
        let club_id: Option<String> = if flight.aircraft_id.is_some() {
            let cid = self
                .flight_aircraft_club_id(flight_id)
                .await
                .or_else(|| flight.club_id.clone());
            Some(cid.ok_or(ShareCreateError::NotFound)?)
        } else {
            None
        };
        let token = new_share_token();
        let token_hash = digest_token(&token);
        let id = new_id();
        let created_at = now_ms();
        const SHARE_MAX_MS: i64 = 30 * 24 * 3600 * 1000;
        let expires_at = created_at + SHARE_MAX_MS;
        let row = FlightShareRow {
            id: id.clone(),
            flight_id: flight_id.to_string(),
            club_id,
            created_by: pilot_id.to_string(),
            token_hash: token_hash.clone(),
            created_at,
            expires_at: Some(expires_at),
            revoked_at: None,
        };
        sqlx::query(
            "INSERT INTO flight_shares (id, flight_id, club_id, created_by, token_hash, created_at, expires_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(&row.id)
        .bind(&row.flight_id)
        .bind(row.club_id.as_deref())
        .bind(&row.created_by)
        .bind(&row.token_hash)
        .bind(row.created_at)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .map_err(ShareCreateError::Db)?;
        Ok((row, token))
    }

    pub async fn list_flight_shares(
        &self,
        pilot_id: &str,
        flight_id: &str,
    ) -> Result<Vec<FlightShareListItem>, ShareCreateError> {
        let flight = self
            .get_flight(flight_id)
            .await
            .ok_or(ShareCreateError::NotFound)?;
        if flight.pilot_id != pilot_id {
            return Err(ShareCreateError::NotOwner);
        }
        let rows = sqlx::query(
            "SELECT id, created_at, expires_at, revoked_at FROM flight_shares
             WHERE flight_id = $1 ORDER BY created_at DESC",
        )
        .bind(flight_id)
        .fetch_all(&self.pool)
        .await
        .map_err(ShareCreateError::Db)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let revoked_at: Option<i64> = r.try_get(3).ok()?;
                Some(FlightShareListItem {
                    id: r.try_get(0).ok()?,
                    created_at: r.try_get(1).ok()?,
                    expires_at: r.try_get(2).ok()?,
                    revoked: revoked_at.is_some(),
                })
            })
            .collect())
    }

    pub async fn revoke_flight_share(
        &self,
        pilot_id: &str,
        flight_id: &str,
        share_id: &str,
    ) -> Result<(), ShareRevokeError> {
        let flight = self
            .get_flight(flight_id)
            .await
            .ok_or(ShareRevokeError::NotFound)?;
        if flight.pilot_id != pilot_id {
            return Err(ShareRevokeError::NotOwner);
        }
        let now = now_ms();
        let changed = sqlx::query(
            "UPDATE flight_shares SET revoked_at = $1
             WHERE id = $2 AND flight_id = $3 AND revoked_at IS NULL",
        )
        .bind(now)
        .bind(share_id)
        .bind(flight_id)
        .execute(&self.pool)
        .await
        .map_err(ShareRevokeError::Db)?;
        if changed.rows_affected() == 0 {
            return Err(ShareRevokeError::NotFound);
        }
        Ok(())
    }

    async fn fetch_share_by_hash(&self, token_hash: &str) -> Option<FlightShareRow> {
        let row = sqlx::query(
            "SELECT id, flight_id, club_id, created_by, token_hash, created_at, expires_at, revoked_at
             FROM flight_shares WHERE token_hash = $1",
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        Some(row_from_query(
            row.try_get(0).ok()?,
            row.try_get(1).ok()?,
            row.try_get(2).ok().flatten(),
            row.try_get(3).ok()?,
            row.try_get(4).ok()?,
            row.try_get(5).ok()?,
            row.try_get(6).ok()?,
            row.try_get(7).ok()?,
        ))
    }

    pub async fn resolve_share_token(&self, token: &str) -> Option<ResolvedShare> {
        if token.is_empty() || token.len() > 128 {
            return None;
        }
        let digest = digest_token(token);
        let share = self.fetch_share_by_hash(&digest).await?;
        if !token_hash_matches(&share.token_hash, &digest) {
            return None;
        }
        let flight = self.get_flight(&share.flight_id).await?;
        if !share_active_at(&share, &flight, now_ms()) {
            return None;
        }
        let tail_source = if flight.club_id.is_some() && flight.aircraft_id.is_some() {
            TAIL_SOURCE_CLUB
        } else {
            TAIL_SOURCE_PILOT
        };
        Some(ResolvedShare {
            flight_id: flight.id,
            club_id: share.club_id,
            pilot_id: flight.pilot_id,
            tail: flight.tail,
            tail_source,
            status: flight.status.clone(),
        })
    }

    /// Map a share URL token to a flight id when active (tests + diagnostics).
    pub async fn share_flight_id(&self, token: &str) -> Option<String> {
        self.resolve_share_token(token).await.map(|r| r.flight_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn share_flight_id_tracks_resolve() {
        let db = ClubDb::open_test().await.expect("db");
        assert_eq!(db.share_flight_id("not-a-real-token").await, None);
    }
}
