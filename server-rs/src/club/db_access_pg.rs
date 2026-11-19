//! Access requests and audit — async Postgres (`sqlx`).

use crate::club::access_request::{
    normalize_tail, AccessRequestError, AccessRequestView, AdminAccessRequestView,
};
use crate::club::auth::new_id;
use crate::club::db::{
    now_ms, AdminPendingCounts, ApprovePilotCheckout, CheckoutError, ClubDb, DbError,
};
use sqlx::Row;

impl ClubDb {
    pub async fn record_audit(
        &self,
        club_id: &str,
        actor_pilot_id: Option<&str>,
        action: &str,
        entity_type: &str,
        entity_id: &str,
        detail: Option<&str>,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(new_id())
        .bind(club_id)
        .bind(actor_pilot_id)
        .bind(action)
        .bind(entity_type)
        .bind(entity_id)
        .bind(detail)
        .bind(now_ms())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn pilot_has_open_checkin(&self, pilot_id: &str, now: i64) -> bool {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM checkins
             WHERE outcome = 'ok' AND pilot_id = $1
               AND consumed_at IS NULL AND expires_at IS NOT NULL AND expires_at > $2",
        )
        .bind(pilot_id)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    pub async fn count_pending_access_requests(&self, club_id: &str) -> usize {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM access_requests WHERE club_id = $1 AND status = 'pending'",
        )
        .bind(club_id)
        .fetch_one(&self.pool)
        .await
        .map(|n| n as usize)
        .unwrap_or(0)
    }

    pub async fn create_access_request(
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
        if !self.pilot_has_open_checkin(pilot_id, now).await {
            return Err(AccessRequestError::NoCheckin);
        }
        let club_id: String = sqlx::query_scalar("SELECT club_id FROM pilots WHERE id = $1")
            .bind(pilot_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|_| AccessRequestError::Db)?;
        if let Some(tid) = type_id {
            let ok = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM aircraft_types WHERE club_id = $1 AND id = $2",
            )
            .bind(&club_id)
            .bind(tid)
            .fetch_one(&self.pool)
            .await
            .map(|n| n > 0)
            .unwrap_or(false);
            if !ok {
                return Err(AccessRequestError::MissingType);
            }
        }
        let dup: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM access_requests
             WHERE pilot_id = $1 AND requested_tail = $2 AND status = 'pending'",
        )
        .bind(pilot_id)
        .bind(&tail)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        if dup > 0 {
            return Err(AccessRequestError::DuplicatePending);
        }
        let id = new_id();
        let new_type = new_type_name.map(str::trim).filter(|s| !s.is_empty());
        sqlx::query(
            "INSERT INTO access_requests (id, club_id, pilot_id, requested_tail, type_id, new_type_name, status, created_at)
             VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7)",
        )
        .bind(&id)
        .bind(&club_id)
        .bind(pilot_id)
        .bind(&tail)
        .bind(type_id)
        .bind(new_type)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|_| AccessRequestError::Db)?;
        let _ = self
            .record_audit(
                &club_id,
                Some(pilot_id),
                "access_request.create",
                "access_request",
                &id,
                Some(&tail),
            )
            .await;
        Ok(id)
    }

    pub async fn list_pilot_access_requests(&self, pilot_id: &str) -> Vec<AccessRequestView> {
        let rows = sqlx::query(
            "SELECT ar.id, ar.pilot_id, ar.requested_tail, t.code, ar.new_type_name,
                    ar.status, ar.reject_reason, ar.created_at, ar.decided_at
             FROM access_requests ar
             LEFT JOIN aircraft_types t ON t.id = ar.type_id
             WHERE ar.pilot_id = $1
             ORDER BY ar.created_at DESC",
        )
        .bind(pilot_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .map(|r| AccessRequestView {
                id: r.get(0),
                requested_tail: r.get(2),
                type_code: r.get(3),
                new_type_name: r.get(4),
                status: r.get(5),
                reject_reason: r.get(6),
                created_at: r.get(7),
                decided_at: r.get(8),
            })
            .collect()
    }

    pub async fn list_admin_access_requests(
        &self,
        club_id: &str,
        status: Option<&str>,
    ) -> Vec<AdminAccessRequestView> {
        let rows = if let Some(st) = status {
            sqlx::query(
                "SELECT ar.id, ar.pilot_id, ar.requested_tail, t.code, ar.new_type_name,
                        ar.status, ar.reject_reason, ar.created_at, ar.decided_at,
                        ar.pilot_id, p.name, p.email
                 FROM access_requests ar
                 JOIN pilots p ON p.id = ar.pilot_id
                 LEFT JOIN aircraft_types t ON t.id = ar.type_id
                 WHERE ar.club_id = $1 AND ar.status = $2
                 ORDER BY ar.created_at DESC",
            )
            .bind(club_id)
            .bind(st)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query(
                "SELECT ar.id, ar.pilot_id, ar.requested_tail, t.code, ar.new_type_name,
                        ar.status, ar.reject_reason, ar.created_at, ar.decided_at,
                        ar.pilot_id, p.name, p.email
                 FROM access_requests ar
                 JOIN pilots p ON p.id = ar.pilot_id
                 LEFT JOIN aircraft_types t ON t.id = ar.type_id
                 WHERE ar.club_id = $1
                 ORDER BY ar.created_at DESC",
            )
            .bind(club_id)
            .fetch_all(&self.pool)
            .await
        }
        .unwrap_or_default();
        rows.into_iter()
            .map(|r| AdminAccessRequestView {
                base: AccessRequestView {
                    id: r.get(0),
                    requested_tail: r.get(2),
                    type_code: r.get(3),
                    new_type_name: r.get(4),
                    status: r.get(5),
                    reject_reason: r.get(6),
                    created_at: r.get(7),
                    decided_at: r.get(8),
                },
                pilot_id: r.get(9),
                pilot_name: r.get(10),
                pilot_email: r.get(11),
            })
            .collect()
    }

    pub async fn approve_access_request(
        &self,
        admin_id: &str,
        request_id: &str,
        expires_at: Option<i64>,
        now: i64,
    ) -> Result<(), AccessRequestError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AccessRequestError::Db)?;
        let row = sqlx::query(
            "SELECT ar.club_id, ar.pilot_id, ar.requested_tail, ar.type_id, ar.new_type_name, ar.status
             FROM access_requests ar WHERE ar.id = $1",
        )
        .bind(request_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AccessRequestError::Db)?;
        let Some(row) = row else {
            return Err(AccessRequestError::NotFound);
        };
        let club_id: String = row.get(0);
        let pilot_id: String = row.get(1);
        let tail: String = row.get(2);
        let type_id: Option<String> = row.get(3);
        let new_type_name: Option<String> = row.get(4);
        let status: String = row.get(5);
        if self.require_club_admin(admin_id, &club_id).await.is_err() {
            tx.rollback().await.ok();
            return Err(AccessRequestError::Forbidden);
        }
        if status != "pending" {
            return Err(AccessRequestError::NotPending);
        }
        let mut resolved_type_id = type_id;
        if resolved_type_id.is_none() {
            let name = new_type_name.as_deref().unwrap_or("").trim();
            if name.is_empty() {
                return Err(AccessRequestError::MissingType);
            }
            resolved_type_id = if let Some(existing) =
                sqlx::query_scalar("SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2")
                    .bind(&club_id)
                    .bind(name)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(|_| AccessRequestError::Db)?
            {
                Some(existing)
            } else {
                let tid = new_id();
                sqlx::query("INSERT INTO aircraft_types (id, club_id, code) VALUES ($1, $2, $3)")
                    .bind(&tid)
                    .bind(&club_id)
                    .bind(name)
                    .execute(&mut *tx)
                    .await
                    .map_err(|_| AccessRequestError::Db)?;
                sqlx::query(
                    "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                     VALUES ($1, $2, $3, 'create', 'aircraft_type', $4, $5, $6)",
                )
                .bind(new_id())
                .bind(&club_id)
                .bind(admin_id)
                .bind(&tid)
                .bind(name)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|_| AccessRequestError::Db)?;
                Some(tid)
            };
        }
        let type_id = resolved_type_id.ok_or(AccessRequestError::MissingType)?;
        let aircraft_id = if let Some(row) =
            sqlx::query("SELECT id, type_id FROM aircraft WHERE club_id = $1 AND tail = $2")
                .bind(&club_id)
                .bind(&tail)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| AccessRequestError::Db)?
        {
            let aid: String = row.get(0);
            let existing_type: String = row.get(1);
            if existing_type != type_id {
                return Err(AccessRequestError::TypeMismatch);
            }
            aid
        } else {
            let aid = new_id();
            sqlx::query(
                "INSERT INTO aircraft (id, club_id, tail, type_id, active) VALUES ($1, $2, $3, $4, true)",
            )
            .bind(&aid)
            .bind(&club_id)
            .bind(&tail)
            .bind(&type_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| AccessRequestError::Db)?;
            sqlx::query(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                 VALUES ($1, $2, $3, 'create', 'aircraft', $4, $5, $6)",
            )
            .bind(new_id())
            .bind(&club_id)
            .bind(admin_id)
            .bind(&aid)
            .bind(&tail)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| AccessRequestError::Db)?;
            aid
        };
        let checkout_id = new_id();
        sqlx::query(
            "INSERT INTO checkouts (id, pilot_id, type_id, aircraft_id, expires_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(&checkout_id)
        .bind(&pilot_id)
        .bind(&type_id)
        .bind(&aircraft_id)
        .bind(expires_at)
        .execute(&mut *tx)
        .await
        .map_err(|_| AccessRequestError::Db)?;
        sqlx::query(
            "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
             VALUES ($1, $2, $3, 'create', 'checkout', $4, $5, $6)",
        )
        .bind(new_id())
        .bind(&club_id)
        .bind(admin_id)
        .bind(&checkout_id)
        .bind(&tail)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|_| AccessRequestError::Db)?;
        sqlx::query(
            "UPDATE access_requests SET status = 'approved', decided_at = $1, decided_by_pilot_id = $2
             WHERE id = $3",
        )
        .bind(now)
        .bind(admin_id)
        .bind(request_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| AccessRequestError::Db)?;
        sqlx::query(
            "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
             VALUES ($1, $2, $3, 'approve', 'access_request', $4, $5, $6)",
        )
        .bind(new_id())
        .bind(&club_id)
        .bind(admin_id)
        .bind(request_id)
        .bind(&tail)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|_| AccessRequestError::Db)?;
        tx.commit().await.map_err(|_| AccessRequestError::Db)?;
        Ok(())
    }

    pub async fn reject_access_request(
        &self,
        admin_id: &str,
        request_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<(), AccessRequestError> {
        let row = sqlx::query("SELECT club_id, status FROM access_requests WHERE id = $1")
            .bind(request_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| AccessRequestError::Db)?;
        let Some(row) = row else {
            return Err(AccessRequestError::NotFound);
        };
        let club_id: String = row.get(0);
        let status: String = row.get(1);
        if status != "pending" {
            return Err(AccessRequestError::NotPending);
        }
        if self.require_club_admin(admin_id, &club_id).await.is_err() {
            return Err(AccessRequestError::Forbidden);
        }
        sqlx::query(
            "UPDATE access_requests SET status = 'rejected', reject_reason = $1,
                decided_at = $2, decided_by_pilot_id = $3 WHERE id = $4",
        )
        .bind(reason)
        .bind(now)
        .bind(admin_id)
        .bind(request_id)
        .execute(&self.pool)
        .await
        .map_err(|_| AccessRequestError::Db)?;
        sqlx::query(
            "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
             VALUES ($1, $2, $3, 'reject', 'access_request', $4, $5, $6)",
        )
        .bind(new_id())
        .bind(&club_id)
        .bind(admin_id)
        .bind(request_id)
        .bind(reason)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|_| AccessRequestError::Db)?;
        Ok(())
    }

    pub async fn approve_pilot_with_checkout(
        &self,
        admin_id: &str,
        pilot_id: &str,
        checkout: Option<ApprovePilotCheckout>,
    ) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query("SELECT club_id, status, role FROM pilots WHERE id = $1")
            .bind(pilot_id)
            .fetch_one(&mut *tx)
            .await?;
        let club_id: String = row.get(0);
        let status: String = row.get(1);
        let role: String = row.get(2);
        if status != "pending" {
            tx.commit().await?;
            return Ok(());
        }
        let verified: Option<i64> =
            sqlx::query_scalar("SELECT email_verified_at FROM pilots WHERE id = $1")
                .bind(pilot_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| DbError::protocol("email not verified"))?
                .flatten();
        if verified.is_none() {
            tx.rollback().await.ok();
            return Err(DbError::protocol("email not verified"));
        }
        if self.require_club_admin(admin_id, &club_id).await.is_err() {
            tx.rollback().await.ok();
            return Err(DbError::protocol("forbidden"));
        }
        let now = now_ms();
        sqlx::query("UPDATE pilots SET status = 'approved' WHERE id = $1")
            .bind(pilot_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
             VALUES ($1, $2, $3, $4, 'active', $5)
             ON CONFLICT (pilot_id, club_id) DO UPDATE SET
               role = excluded.role,
               status = excluded.status",
        )
        .bind(new_id())
        .bind(pilot_id)
        .bind(&club_id)
        .bind(&role)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
             VALUES ($1, $2, $3, 'approve', 'pilot', $4, NULL, $5)",
        )
        .bind(new_id())
        .bind(&club_id)
        .bind(admin_id)
        .bind(pilot_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if let Some(co) = checkout {
            let type_id: String = sqlx::query_scalar(
                "SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2",
            )
            .bind(&club_id)
            .bind(&co.type_code)
            .fetch_one(&mut *tx)
            .await?;
            let aircraft_id: Option<String> = if let Some(ref tail) = co.tail {
                sqlx::query_scalar("SELECT id FROM aircraft WHERE club_id = $1 AND tail = $2")
                    .bind(&club_id)
                    .bind(tail)
                    .fetch_optional(&mut *tx)
                    .await?
            } else {
                None
            };
            let checkout_id = new_id();
            sqlx::query(
                "INSERT INTO checkouts (id, pilot_id, type_id, aircraft_id, expires_at)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(&checkout_id)
            .bind(pilot_id)
            .bind(&type_id)
            .bind(aircraft_id)
            .bind(co.expires_at)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, detail, created_at)
                 VALUES ($1, $2, $3, 'create', 'checkout', $4, $5, $6)",
            )
            .bind(new_id())
            .bind(&club_id)
            .bind(admin_id)
            .bind(&checkout_id)
            .bind(&co.type_code)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn add_checkout_typed(
        &self,
        pilot_id: &str,
        type_code: &str,
        tail: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<(), CheckoutError> {
        let club_id: String = sqlx::query_scalar("SELECT club_id FROM pilots WHERE id = $1")
            .bind(pilot_id)
            .fetch_one(&self.pool)
            .await
            .map_err(CheckoutError::Db)?;
        let (type_id, aircraft_id) = if let Some(tail) = tail {
            let row = sqlx::query(
                "SELECT a.id, a.type_id FROM aircraft a
                 WHERE a.club_id = $1 AND a.tail = $2",
            )
            .bind(&club_id)
            .bind(tail)
            .fetch_optional(&self.pool)
            .await
            .map_err(CheckoutError::Db)?;
            let Some(row) = row else {
                return Err(CheckoutError::UnknownTail);
            };
            let ac_id: String = row.try_get(0)?;
            let type_id: String = row.try_get(1)?;
            (type_id, Some(ac_id))
        } else {
            let type_id: String = sqlx::query_scalar(
                "SELECT id FROM aircraft_types WHERE club_id = $1 AND code = $2",
            )
            .bind(&club_id)
            .bind(type_code)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => CheckoutError::UnknownType,
                other => CheckoutError::Db(other),
            })?;
            (type_id, None)
        };
        sqlx::query(
            "INSERT INTO checkouts (id, pilot_id, type_id, aircraft_id, expires_at)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(new_id())
        .bind(pilot_id)
        .bind(&type_id)
        .bind(aircraft_id)
        .bind(expires_at)
        .execute(&self.pool)
        .await
        .map_err(CheckoutError::Db)?;
        Ok(())
    }

    pub async fn admin_pending_counts(&self, club_id: &str) -> AdminPendingCounts {
        AdminPendingCounts {
            pilots: self.list_pending_pilots(club_id).await.len(),
            access_requests: self.count_pending_access_requests(club_id).await,
        }
    }
}
