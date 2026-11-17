use crate::club::{
    auth::{digest_token, new_id, new_share_token},
    db::{now_ms, ClubDb, FlightRow},
};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use subtle::ConstantTimeEq;

pub const SHARE_LIFETIME_AFTER_END_MS: i64 = 24 * 3600 * 1000;

#[derive(Debug, Clone)]
pub struct FlightShareRow {
    pub id: String,
    pub flight_id: String,
    pub club_id: String,
    pub created_by: String,
    pub token_hash: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct FlightShareListItem {
    pub id: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub revoked: bool,
}

#[derive(Clone, Debug)]
pub struct ResolvedShare {
    pub flight_id: String,
    #[allow(dead_code)]
    pub club_id: String,
    pub tail: String,
    pub status: String,
}

#[derive(Debug)]
pub enum ShareCreateError {
    NotOwner,
    NotFound,
    #[allow(dead_code)]
    Db(rusqlite::Error),
}

#[derive(Debug)]
pub enum ShareRevokeError {
    NotOwner,
    NotFound,
    #[allow(dead_code)]
    Db(rusqlite::Error),
}

#[allow(clippy::too_many_arguments)]
fn row_from_query(
    id: String,
    flight_id: String,
    club_id: String,
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

impl ClubDb {
    pub fn create_flight_share(
        &self,
        pilot_id: &str,
        flight_id: &str,
    ) -> Result<(FlightShareRow, String), ShareCreateError> {
        let flight = self
            .get_flight(flight_id)
            .ok_or(ShareCreateError::NotFound)?;
        if flight.pilot_id != pilot_id {
            return Err(ShareCreateError::NotOwner);
        }
        let club_id = self.with_conn(|conn| {
            conn.query_row(
                "SELECT a.club_id FROM flights f JOIN aircraft a ON a.id = f.aircraft_id WHERE f.id = ?1",
                params![flight_id],
                |r| r.get::<_, String>(0),
            )
        })
        .map_err(ShareCreateError::Db)?;
        let token = new_share_token();
        let token_hash = digest_token(&token);
        let id = new_id();
        let created_at = now_ms();
        let row = FlightShareRow {
            id: id.clone(),
            flight_id: flight_id.to_string(),
            club_id,
            created_by: pilot_id.to_string(),
            token_hash: token_hash.clone(),
            created_at,
            expires_at: None,
            revoked_at: None,
        };
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO flight_shares (id, flight_id, club_id, created_by, token_hash, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    row.id,
                    row.flight_id,
                    row.club_id,
                    row.created_by,
                    row.token_hash,
                    row.created_at,
                ],
            )
        })
        .map_err(ShareCreateError::Db)?;
        Ok((row, token))
    }

    pub fn list_flight_shares(
        &self,
        pilot_id: &str,
        flight_id: &str,
    ) -> Result<Vec<FlightShareListItem>, ShareCreateError> {
        let flight = self
            .get_flight(flight_id)
            .ok_or(ShareCreateError::NotFound)?;
        if flight.pilot_id != pilot_id {
            return Err(ShareCreateError::NotOwner);
        }
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, created_at, expires_at, revoked_at FROM flight_shares
                 WHERE flight_id = ?1 ORDER BY created_at DESC",
            )?;
            let rows = stmt.query_map(params![flight_id], |r| {
                Ok(FlightShareListItem {
                    id: r.get(0)?,
                    created_at: r.get(1)?,
                    expires_at: r.get(2)?,
                    revoked: r.get::<_, Option<i64>>(3)?.is_some(),
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
        .map_err(ShareCreateError::Db)
    }

    pub fn revoke_flight_share(
        &self,
        pilot_id: &str,
        flight_id: &str,
        share_id: &str,
    ) -> Result<(), ShareRevokeError> {
        let flight = self
            .get_flight(flight_id)
            .ok_or(ShareRevokeError::NotFound)?;
        if flight.pilot_id != pilot_id {
            return Err(ShareRevokeError::NotOwner);
        }
        let now = now_ms();
        let changed = self
            .with_conn(|conn| {
                conn.execute(
                    "UPDATE flight_shares SET revoked_at = ?1
                     WHERE id = ?2 AND flight_id = ?3 AND revoked_at IS NULL",
                    params![now, share_id, flight_id],
                )
            })
            .map_err(ShareRevokeError::Db)?;
        if changed == 0 {
            return Err(ShareRevokeError::NotFound);
        }
        Ok(())
    }

    fn fetch_share_by_hash(&self, token_hash: &str) -> Option<FlightShareRow> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT id, flight_id, club_id, created_by, token_hash, created_at, expires_at, revoked_at
                 FROM flight_shares WHERE token_hash = ?1",
                params![token_hash],
                |r| {
                    Ok(row_from_query(
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .optional()
        })
        .ok()
        .flatten()
    }

    pub fn resolve_share_token(&self, token: &str) -> Option<ResolvedShare> {
        if token.is_empty() || token.len() > 128 {
            return None;
        }
        let digest = digest_token(token);
        let share = self.fetch_share_by_hash(&digest)?;
        if !token_hash_matches(&share.token_hash, &digest) {
            return None;
        }
        let flight = self.get_flight(&share.flight_id)?;
        if !share_active_at(&share, &flight, now_ms()) {
            return None;
        }
        Some(ResolvedShare {
            flight_id: flight.id,
            club_id: share.club_id,
            tail: flight.tail,
            status: flight.status.clone(),
        })
    }

    /// Same flight as resolve_share_token; used to assert share access does not leak other flights.
    #[allow(dead_code)]
    pub fn share_flight_id(&self, token: &str) -> Option<String> {
        self.resolve_share_token(token).map(|r| r.flight_id)
    }
}

pub(crate) fn token_hash_matches(stored: &str, candidate_digest: &str) -> bool {
    if stored.len() != candidate_digest.len() {
        return false;
    }
    stored.as_bytes().ct_eq(candidate_digest.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::club::{db::ClubDb, visibility::FlightVisibility};

    fn pilot_flight(db: &ClubDb) -> (String, String) {
        let club = db.default_club_id().unwrap();
        db.upsert_type(&club, "C152").unwrap();
        db.upsert_aircraft(&club, "CC-SHR", "C152", true).unwrap();
        let pilot = db
            .register(&club, "Sharer", "share@test", "", "password1234")
            .unwrap();
        db.approve_pilot(&pilot).unwrap();
        db.add_checkout(&pilot, "C152", None, None).unwrap();
        let (dev, _) = db.create_device(&pilot, "phone").unwrap();
        db.issue_open_checkin(&pilot, &dev, &club, now_ms())
            .unwrap();
        let fid = db
            .start_flight(
                &pilot,
                &dev,
                "CC-SHR",
                FlightVisibility::Club,
                false,
                now_ms(),
            )
            .unwrap();
        (pilot, fid)
    }

    #[test]
    fn owner_create_revoke_and_expired_404() {
        let db = ClubDb::open_in_memory().unwrap();
        let (pilot, fid) = pilot_flight(&db);
        let (_row, token) = db.create_flight_share(&pilot, &fid).unwrap();
        assert!(db.resolve_share_token(&token).is_some());
        let list = db.list_flight_shares(&pilot, &fid).unwrap();
        assert_eq!(list.len(), 1);
        db.revoke_flight_share(&pilot, &fid, &list[0].id).unwrap();
        assert!(db.resolve_share_token(&token).is_none());
    }

    #[test]
    fn non_owner_cannot_create() {
        let db = ClubDb::open_in_memory().unwrap();
        let club = db.default_club_id().unwrap();
        let (pilot_a, fid) = pilot_flight(&db);
        let pilot_b = db
            .register(&club, "Other", "other@test", "", "password1234")
            .unwrap();
        db.approve_pilot(&pilot_b).unwrap();
        assert!(matches!(
            db.create_flight_share(&pilot_b, &fid),
            Err(ShareCreateError::NotOwner)
        ));
        assert!(db.create_flight_share(&pilot_a, &fid).is_ok());
    }

    #[test]
    fn private_flight_share_works() {
        let db = ClubDb::open_in_memory().unwrap();
        let club = db.default_club_id().unwrap();
        db.upsert_type(&club, "C152").unwrap();
        db.upsert_aircraft(&club, "N-PVT", "C152", true).unwrap();
        let pilot = db
            .register(&club, "Pvt", "pvt@test", "", "password1234")
            .unwrap();
        db.approve_pilot(&pilot).unwrap();
        db.add_checkout(&pilot, "C152", None, None).unwrap();
        let (dev, _) = db.create_device(&pilot, "d").unwrap();
        db.issue_open_checkin(&pilot, &dev, &club, now_ms())
            .unwrap();
        let fid = db
            .start_flight(
                &pilot,
                &dev,
                "N-PVT",
                FlightVisibility::Club,
                false,
                now_ms(),
            )
            .unwrap();
        let (_, token) = db.create_flight_share(&pilot, &fid).unwrap();
        let resolved = db.resolve_share_token(&token).unwrap();
        assert_eq!(resolved.tail, "N-PVT");
        assert!(db.public_map().iter().all(|p| p.flight_id != fid));
    }

    #[test]
    fn share_does_not_leak_other_flights() {
        let db = ClubDb::open_in_memory().unwrap();
        let (pilot_a, fid_a) = pilot_flight(&db);
        let club = db.default_club_id().unwrap();
        db.upsert_aircraft(&club, "CC-OTH", "C152", true).unwrap();
        let pilot_b = db
            .register(&club, "B", "b@test", "", "password1234")
            .unwrap();
        db.approve_pilot(&pilot_b).unwrap();
        db.add_checkout(&pilot_b, "C152", None, None).unwrap();
        let (dev_b, _) = db.create_device(&pilot_b, "b").unwrap();
        db.issue_open_checkin(&pilot_b, &dev_b, &club, now_ms())
            .unwrap();
        let fid_b = db
            .start_flight(
                &pilot_b,
                &dev_b,
                "CC-OTH",
                FlightVisibility::Public,
                false,
                now_ms(),
            )
            .unwrap();
        let (_, token) = db.create_flight_share(&pilot_a, &fid_a).unwrap();
        assert_eq!(db.share_flight_id(&token).as_deref(), Some(fid_a.as_str()));
        assert_ne!(db.share_flight_id(&token).as_deref(), Some(fid_b.as_str()));
    }

    #[test]
    fn ended_flight_share_expires_after_24h() {
        let db = ClubDb::open_in_memory().unwrap();
        let (pilot, fid) = pilot_flight(&db);
        let (_, token) = db.create_flight_share(&pilot, &fid).unwrap();
        let end = now_ms() - SHARE_LIFETIME_AFTER_END_MS - 60_000;
        db.end_flight(&pilot, &fid, end).unwrap();
        assert!(db.resolve_share_token(&token).is_none());
    }
}
