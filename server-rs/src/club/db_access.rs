use crate::club::access_request::{
    normalize_tail, AccessRequestError, AccessRequestView, AdminAccessRequestView,
};
use crate::club::auth::new_id;
use crate::club::db::{now_ms, CheckoutError, ClubDb};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct ApprovePilotCheckout {
    #[serde(rename = "type")]
    pub type_code: String,
    pub tail: Option<String>,
    pub expires_at: Option<i64>,
}

#[derive(Serialize)]
pub struct AdminPendingCounts {
    pub pilots: usize,
    pub access_requests: usize,
}

impl ClubDb {
    pub fn record_audit(
        &self,
        club_id: &str,
        actor_pilot_id: Option<&str>,
        action: &str,
        entity_type: &str,
        entity_id: &str,
        detail: Option<&str>,
    ) -> Result<(), rusqlite::Error> {
        let now = now_ms();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    new_id(),
                    club_id,
                    actor_pilot_id,
                    action,
                    entity_type,
                    entity_id,
                    detail,
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub fn pilot_has_open_checkin(&self, pilot_id: &str, now: i64) -> bool {
        self.with_conn(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM checkins
                 WHERE outcome = 'ok' AND pilot_id = ?1
                   AND consumed_at IS NULL AND expires_at IS NOT NULL AND expires_at > ?2",
                params![pilot_id, now],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        })
        .unwrap_or(false)
    }

    pub fn count_pending_access_requests(&self, club_id: &str) -> usize {
        self.with_conn(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM access_requests WHERE club_id = ?1 AND status = 'pending'",
                params![club_id],
                |r| r.get(0),
            )?;
            Ok(n as usize)
        })
        .unwrap_or(0)
    }

    pub fn create_access_request(
        &self,
        pilot_id: &str,
        tail: &str,
        type_id: Option<&str>,
        new_type_name: Option<&str>,
        now: i64,
    ) -> Result<String, AccessRequestError> {
        let tail = normalize_tail(tail).map_err(|_| AccessRequestError::InvalidTail)?;
        if type_id.is_none() && new_type_name.map(str::trim).unwrap_or("").is_empty() {
            return Err(AccessRequestError::MissingType);
        }
        if !self.pilot_has_open_checkin(pilot_id, now) {
            return Err(AccessRequestError::NoCheckin);
        }
        let club_id: String = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT club_id FROM pilots WHERE id = ?1",
                    params![pilot_id],
                    |r| r.get(0),
                )
            })
            .map_err(|_| AccessRequestError::Db)?;
        if let Some(tid) = type_id {
            let ok = self
                .with_conn(|conn| {
                    let n: i64 = conn.query_row(
                        "SELECT COUNT(*) FROM aircraft_types WHERE club_id = ?1 AND id = ?2",
                        params![club_id, tid],
                        |r| r.get(0),
                    )?;
                    Ok(n > 0)
                })
                .unwrap_or(false);
            if !ok {
                return Err(AccessRequestError::MissingType);
            }
        }
        let dup: i64 = self
            .with_conn(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM access_requests
                     WHERE pilot_id = ?1 AND requested_tail = ?2 AND status = 'pending'",
                    params![pilot_id, tail],
                    |r| r.get(0),
                )
            })
            .unwrap_or(0);
        if dup > 0 {
            return Err(AccessRequestError::DuplicatePending);
        }
        let id = new_id();
        let new_type = new_type_name.map(str::trim).filter(|s| !s.is_empty());
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO access_requests (id, club_id, pilot_id, requested_tail, type_id, new_type_name, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
                params![id, club_id, pilot_id, tail, type_id, new_type, now],
            )?;
            Ok(())
        })
        .map_err(|_| AccessRequestError::Db)?;
        let _ = self.record_audit(
            &club_id,
            Some(pilot_id),
            "access_request.create",
            "access_request",
            &id,
            Some(&tail),
        );
        Ok(id)
    }

    pub fn list_pilot_access_requests(&self, pilot_id: &str) -> Vec<AccessRequestView> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT ar.id, ar.pilot_id, ar.requested_tail, t.code, ar.new_type_name,
                        ar.status, ar.reject_reason, ar.created_at, ar.decided_at
                 FROM access_requests ar
                 LEFT JOIN aircraft_types t ON t.id = ar.type_id
                 WHERE ar.pilot_id = ?1
                 ORDER BY ar.created_at DESC",
            )?;
            let rows = stmt.query_map(params![pilot_id], |r| {
                Ok(AccessRequestView {
                    id: r.get(0)?,
                    requested_tail: r.get(2)?,
                    type_code: r.get(3)?,
                    new_type_name: r.get(4)?,
                    status: r.get(5)?,
                    reject_reason: r.get(6)?,
                    created_at: r.get(7)?,
                    decided_at: r.get(8)?,
                })
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    pub fn list_admin_access_requests(
        &self,
        club_id: &str,
        status: Option<&str>,
    ) -> Vec<AdminAccessRequestView> {
        self.with_conn(|conn| {
            let sql = if status.is_some() {
                "SELECT ar.id, ar.pilot_id, ar.requested_tail, t.code, ar.new_type_name,
                        ar.status, ar.reject_reason, ar.created_at, ar.decided_at,
                        ar.pilot_id, p.name, p.email
                 FROM access_requests ar
                 JOIN pilots p ON p.id = ar.pilot_id
                 LEFT JOIN aircraft_types t ON t.id = ar.type_id
                 WHERE ar.club_id = ?1 AND ar.status = ?2
                 ORDER BY ar.created_at DESC"
            } else {
                "SELECT ar.id, ar.pilot_id, ar.requested_tail, t.code, ar.new_type_name,
                        ar.status, ar.reject_reason, ar.created_at, ar.decided_at,
                        ar.pilot_id, p.name, p.email
                 FROM access_requests ar
                 JOIN pilots p ON p.id = ar.pilot_id
                 LEFT JOIN aircraft_types t ON t.id = ar.type_id
                 WHERE ar.club_id = ?1
                 ORDER BY ar.created_at DESC"
            };
            let mut stmt = conn.prepare(sql)?;
            let mut out = Vec::new();
            if let Some(st) = status {
                let rows = stmt.query_map(params![club_id, st], |r| {
                    Ok(AdminAccessRequestView {
                        base: AccessRequestView {
                            id: r.get(0)?,
                            requested_tail: r.get(2)?,
                            type_code: r.get(3)?,
                            new_type_name: r.get(4)?,
                            status: r.get(5)?,
                            reject_reason: r.get(6)?,
                            created_at: r.get(7)?,
                            decided_at: r.get(8)?,
                        },
                        pilot_id: r.get(9)?,
                        pilot_name: r.get(10)?,
                        pilot_email: r.get(11)?,
                    })
                })?;
                for row in rows {
                    out.push(row?);
                }
            } else {
                let rows = stmt.query_map(params![club_id], |r| {
                    Ok(AdminAccessRequestView {
                        base: AccessRequestView {
                            id: r.get(0)?,
                            requested_tail: r.get(2)?,
                            type_code: r.get(3)?,
                            new_type_name: r.get(4)?,
                            status: r.get(5)?,
                            reject_reason: r.get(6)?,
                            created_at: r.get(7)?,
                            decided_at: r.get(8)?,
                        },
                        pilot_id: r.get(9)?,
                        pilot_name: r.get(10)?,
                        pilot_email: r.get(11)?,
                    })
                })?;
                for row in rows {
                    out.push(row?);
                }
            }
            Ok(out)
        })
        .unwrap_or_default()
    }

    pub fn approve_access_request(
        &self,
        admin_id: &str,
        request_id: &str,
        expires_at: Option<i64>,
        now: i64,
    ) -> Result<(), AccessRequestError> {
        self.with_conn_any(|conn| -> Result<(), AccessRequestError> {
            let tx = conn.unchecked_transaction().map_err(|_| AccessRequestError::Db)?;
            let row = conn
                .query_row(
                    "SELECT ar.club_id, ar.pilot_id, ar.requested_tail, ar.type_id, ar.new_type_name, ar.status
                     FROM access_requests ar WHERE ar.id = ?1",
                    params![request_id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, Option<String>>(3)?,
                            r.get::<_, Option<String>>(4)?,
                            r.get::<_, String>(5)?,
                        ))
                    },
                )
                .optional()
                .map_err(|_| AccessRequestError::Db)?;
            let Some((club_id, pilot_id, tail, type_id, new_type_name, status)) = row else {
                return Err(AccessRequestError::NotFound);
            };
            if status != "pending" {
                return Err(AccessRequestError::NotPending);
            }
            let mut resolved_type_id = type_id;
            if resolved_type_id.is_none() {
                let name = new_type_name.as_deref().unwrap_or("").trim();
                if name.is_empty() {
                    return Err(AccessRequestError::MissingType);
                }
                resolved_type_id = if let Some(existing) = conn
                    .query_row(
                        "SELECT id FROM aircraft_types WHERE club_id = ?1 AND code = ?2",
                        params![club_id, name],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(|_| AccessRequestError::Db)?
                {
                    Some(existing)
                } else {
                    let tid = new_id();
                    conn.execute(
                        "INSERT INTO aircraft_types (id, club_id, code) VALUES (?1, ?2, ?3)",
                        params![tid, club_id, name],
                    )
                    .map_err(|_| AccessRequestError::Db)?;
                    conn.execute(
                        "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                         VALUES (?1, ?2, ?3, 'create', 'aircraft_type', ?4, ?5, ?6)",
                        params![new_id(), club_id, admin_id, tid, name, now],
                    )
                    .map_err(|_| AccessRequestError::Db)?;
                    Some(tid)
                };
            }
            let type_id = resolved_type_id.ok_or(AccessRequestError::MissingType)?;
            let aircraft_id = if let Some(aid) = conn
                .query_row(
                    "SELECT id, type_id FROM aircraft WHERE club_id = ?1 AND tail = ?2",
                    params![club_id, tail],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(|_| AccessRequestError::Db)?
            {
                if aid.1 != type_id {
                    return Err(AccessRequestError::TypeMismatch);
                }
                aid.0
            } else {
                let aid = new_id();
                conn.execute(
                    "INSERT INTO aircraft (id, club_id, tail, type_id, active) VALUES (?1, ?2, ?3, ?4, 1)",
                    params![aid, club_id, tail, type_id],
                )
                .map_err(|_| AccessRequestError::Db)?;
                conn.execute(
                    "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                     VALUES (?1, ?2, ?3, 'create', 'aircraft', ?4, ?5, ?6)",
                    params![new_id(), club_id, admin_id, aid, tail, now],
                )
                .map_err(|_| AccessRequestError::Db)?;
                aid
            };
            let checkout_id = new_id();
            conn.execute(
                "INSERT INTO checkouts (id, pilot_id, type_id, aircraft_id, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![checkout_id, pilot_id, type_id, aircraft_id, expires_at],
            )
            .map_err(|_| AccessRequestError::Db)?;
            conn.execute(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                 VALUES (?1, ?2, ?3, 'create', 'checkout', ?4, ?5, ?6)",
                params![new_id(), club_id, admin_id, checkout_id, tail, now],
            )
            .map_err(|_| AccessRequestError::Db)?;
            conn.execute(
                "UPDATE access_requests SET status = 'approved', decided_at = ?1, decided_by_pilot_id = ?2
                 WHERE id = ?3",
                params![now, admin_id, request_id],
            )
            .map_err(|_| AccessRequestError::Db)?;
            conn.execute(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                 VALUES (?1, ?2, ?3, 'approve', 'access_request', ?4, ?5, ?6)",
                params![new_id(), club_id, admin_id, request_id, tail, now],
            )
            .map_err(|_| AccessRequestError::Db)?;
            tx.commit().map_err(|_| AccessRequestError::Db)?;
            Ok(())
        })
    }

    pub fn reject_access_request(
        &self,
        admin_id: &str,
        request_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<(), AccessRequestError> {
        self.with_conn_any(|conn| -> Result<(), AccessRequestError> {
            let row = conn
                .query_row(
                    "SELECT club_id, status FROM access_requests WHERE id = ?1",
                    params![request_id],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(|_| AccessRequestError::Db)?;
            let Some((club_id, status)) = row else {
                return Err(AccessRequestError::NotFound);
            };
            if status != "pending" {
                return Err(AccessRequestError::NotPending);
            }
            conn.execute(
                "UPDATE access_requests SET status = 'rejected', reject_reason = ?1,
                    decided_at = ?2, decided_by_pilot_id = ?3 WHERE id = ?4",
                params![reason, now, admin_id, request_id],
            )
            .map_err(|_| AccessRequestError::Db)?;
            conn.execute(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                 VALUES (?1, ?2, ?3, 'reject', 'access_request', ?4, ?5, ?6)",
                params![new_id(), club_id, admin_id, request_id, reason, now],
            )
            .map_err(|_| AccessRequestError::Db)?;
            Ok(())
        })
    }

    pub fn approve_pilot_with_checkout(
        &self,
        admin_id: &str,
        pilot_id: &str,
        checkout: Option<ApprovePilotCheckout>,
    ) -> Result<(), rusqlite::Error> {
        self.with_conn(|conn| {
            let tx = conn.unchecked_transaction()?;
            let (club_id, status): (String, String) = conn.query_row(
                "SELECT club_id, status FROM pilots WHERE id = ?1",
                params![pilot_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if status != "pending" {
                tx.commit()?;
                return Ok(());
            }
            let now = now_ms();
            conn.execute(
                "UPDATE pilots SET status = 'approved' WHERE id = ?1",
                params![pilot_id],
            )?;
            conn.execute(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                 VALUES (?1, ?2, ?3, 'approve', 'pilot', ?4, NULL, ?5)",
                params![new_id(), club_id, admin_id, pilot_id, now],
            )?;
            if let Some(co) = checkout {
                let type_id: String = conn.query_row(
                    "SELECT id FROM aircraft_types WHERE club_id = ?1 AND code = ?2",
                    params![club_id, co.type_code],
                    |r| r.get(0),
                )?;
                let aircraft_id = if let Some(ref tail) = co.tail {
                    Some(conn.query_row(
                        "SELECT id FROM aircraft WHERE club_id = ?1 AND tail = ?2",
                        params![club_id, tail],
                        |r| r.get::<_, String>(0),
                    )?)
                } else {
                    None
                };
                let checkout_id = new_id();
                conn.execute(
                    "INSERT INTO checkouts (id, pilot_id, type_id, aircraft_id, expires_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        checkout_id,
                        pilot_id,
                        type_id,
                        aircraft_id,
                        co.expires_at
                    ],
                )?;
                conn.execute(
                    "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                     VALUES (?1, ?2, ?3, 'create', 'checkout', ?4, ?5, ?6)",
                    params![
                        new_id(),
                        club_id,
                        admin_id,
                        checkout_id,
                        co.type_code,
                        now
                    ],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn add_checkout_typed(
        &self,
        pilot_id: &str,
        type_code: &str,
        tail: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<(), CheckoutError> {
        self.with_conn_any(|conn| -> Result<(), CheckoutError> {
            let club_id: String = conn.query_row(
                "SELECT club_id FROM pilots WHERE id = ?1",
                params![pilot_id],
                |r| r.get(0),
            )?;
            let (type_id, aircraft_id) = if let Some(tail) = tail {
                let row = conn
                    .query_row(
                        "SELECT a.id, a.type_id FROM aircraft a
                         WHERE a.club_id = ?1 AND a.tail = ?2",
                        params![club_id, tail],
                        |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                    )
                    .optional()?;
                let Some((ac_id, ty_id)) = row else {
                    return Err(CheckoutError::UnknownTail);
                };
                (ty_id, Some(ac_id))
            } else {
                let type_id: String = conn
                    .query_row(
                        "SELECT id FROM aircraft_types WHERE club_id = ?1 AND code = ?2",
                        params![club_id, type_code],
                        |r| r.get(0),
                    )
                    .map_err(|_| CheckoutError::UnknownType)?;
                (type_id, None)
            };
            conn.execute(
                "INSERT INTO checkouts (id, pilot_id, type_id, aircraft_id, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![new_id(), pilot_id, type_id, aircraft_id, expires_at],
            )?;
            Ok(())
        })
    }

    pub fn admin_pending_counts(&self, club_id: &str) -> AdminPendingCounts {
        AdminPendingCounts {
            pilots: self.list_pending_pilots().len(),
            access_requests: self.count_pending_access_requests(club_id),
        }
    }
}
