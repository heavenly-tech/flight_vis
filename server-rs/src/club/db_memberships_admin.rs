//! Multi-admin club memberships: invites, role changes, last-admin guard.

use crate::club::{
    auth::{digest_token, new_id, new_share_token},
    db::{ClubDb, DbError},
    db_account::ACCOUNT_MAGIC_TTL_MS,
    membership_status::{membership_role_is_admin, MembershipStatus},
};
use sqlx::Row;

pub const ADMIN_INVITE_PURPOSE: &str = "admin_invite";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipAdminError {
    LastAdmin,
    NotFound,
    InvalidRole,
    ClubSuspended,
    ClubClosed,
    Db,
}

impl MembershipAdminError {
    pub fn message(self) -> &'static str {
        match self {
            Self::LastAdmin => "last_admin",
            Self::NotFound => "not_found",
            Self::InvalidRole => "invalid_role",
            Self::ClubSuspended => "club_suspended",
            Self::ClubClosed => "club_closed",
            Self::Db => "db_error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AdminMembershipListRow {
    pub id: String,
    pub pilot_id: String,
    pub name: String,
    pub email: String,
    pub role: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct AdminInviteAccept {
    pub account_id: String,
    pub membership_id: String,
    #[allow(dead_code)]
    pub club_id: String,
}

impl ClubDb {
    pub async fn list_club_memberships_admin(
        &self,
        club_id: &str,
    ) -> Vec<AdminMembershipListRow> {
        let rows = sqlx::query(
            "SELECT m.id, m.pilot_id, p.name, p.email, m.role, m.status
             FROM club_memberships m
             JOIN pilots p ON p.id = m.pilot_id
             WHERE m.club_id = $1
             ORDER BY m.role DESC, p.email",
        )
        .bind(club_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .filter_map(|r| {
                Some(AdminMembershipListRow {
                    id: r.try_get(0).ok()?,
                    pilot_id: r.try_get(1).ok()?,
                    name: r.try_get(2).ok()?,
                    email: r.try_get(3).ok()?,
                    role: r.try_get(4).ok()?,
                    status: r.try_get(5).ok()?,
                })
            })
            .collect()
    }

    pub async fn invite_admin_membership(
        &self,
        club_id: &str,
        email: &str,
        inviter_pilot_id: &str,
        now: i64,
    ) -> Result<String, DbError> {
        let raw = new_share_token();
        let token_hash = digest_token(&raw);
        let id = new_id();
        let expires = now + ACCOUNT_MAGIC_TTL_MS;
        let email = email.trim().to_ascii_lowercase();
        sqlx::query(
            "INSERT INTO magic_login_tokens (id, email, token_hash, created_at, expires_at, purpose, club_id, pilot_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(&id)
        .bind(&email)
        .bind(&token_hash)
        .bind(now)
        .bind(expires)
        .bind(ADMIN_INVITE_PURPOSE)
        .bind(club_id)
        .bind(inviter_pilot_id)
        .execute(&self.pool)
        .await?;
        Ok(raw)
    }

    pub async fn accept_admin_invite(
        &self,
        raw_token: &str,
        now: i64,
    ) -> Result<AdminInviteAccept, MembershipAdminError> {
        let token_hash = digest_token(raw_token.trim());
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| MembershipAdminError::Db)?;
        let row = sqlx::query(
            "SELECT id, email, expires_at, used_at, purpose, club_id
             FROM magic_login_tokens WHERE token_hash = $1 FOR UPDATE",
        )
        .bind(&token_hash)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| MembershipAdminError::Db)?;
        let Some(row) = row else {
            return Err(MembershipAdminError::NotFound);
        };
        let token_id: String = row.try_get(0).map_err(|_| MembershipAdminError::Db)?;
        let email: String = row.try_get(1).map_err(|_| MembershipAdminError::Db)?;
        let expires_at: i64 = row.try_get(2).map_err(|_| MembershipAdminError::Db)?;
        let used_at: Option<i64> = row.try_get(3).map_err(|_| MembershipAdminError::Db)?;
        let purpose: String = row.try_get(4).unwrap_or_default();
        let club_id: Option<String> = row.try_get(5).ok();
        if purpose != ADMIN_INVITE_PURPOSE {
            return Err(MembershipAdminError::NotFound);
        }
        let Some(club_id) = club_id else {
            return Err(MembershipAdminError::NotFound);
        };
        if used_at.is_some() {
            return Err(MembershipAdminError::NotFound);
        }
        if now > expires_at {
            return Err(MembershipAdminError::NotFound);
        }
        if let Some(row) = self.club_public_row(&club_id).await {
            if row.suspended_at.is_some() {
                return Err(MembershipAdminError::ClubSuspended);
            }
            if !ClubDb::is_public_verified(&row) {
                return Err(MembershipAdminError::ClubClosed);
            }
        } else {
            return Err(MembershipAdminError::ClubClosed);
        }
        let updated = sqlx::query(
            "UPDATE magic_login_tokens SET used_at = $1 WHERE id = $2 AND used_at IS NULL",
        )
        .bind(now)
        .bind(&token_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| MembershipAdminError::Db)?;
        if updated.rows_affected() == 0 {
            return Err(MembershipAdminError::NotFound);
        }
        sqlx::query("SELECT id FROM clubs WHERE id = $1 FOR UPDATE")
            .bind(&club_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| MembershipAdminError::Db)?;

        let account_id = if let Some(acct) = self.account_by_email(&email).await {
            acct.id
        } else {
            let aid = new_id();
            sqlx::query(
                "INSERT INTO accounts (id, email, created_at, email_verified_at)
                 VALUES ($1, $2, $3, $3)",
            )
            .bind(&aid)
            .bind(&email)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| MembershipAdminError::Db)?;
            aid
        };
        sqlx::query(
            "UPDATE accounts SET email_verified_at = coalesce(email_verified_at, $1) WHERE id = $2",
        )
        .bind(now)
        .bind(&account_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| MembershipAdminError::Db)?;

        let pilot_id: Option<String> = sqlx::query_scalar(
            "SELECT p.id FROM pilots p
             WHERE p.club_id = $1 AND (p.account_id = $2 OR lower(p.email) = lower($3))
             LIMIT 1",
        )
        .bind(&club_id)
        .bind(&account_id)
        .bind(&email)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| MembershipAdminError::Db)?;

        let pilot_id = if let Some(pid) = pilot_id {
            sqlx::query(
                "UPDATE pilots SET account_id = coalesce(account_id, $1), status = 'approved', role = 'admin'
                 WHERE id = $2",
            )
            .bind(&account_id)
            .bind(&pid)
            .execute(&mut *tx)
            .await
            .map_err(|_| MembershipAdminError::Db)?;
            pid
        } else {
            let pid = new_id();
            let display = email.split('@').next().unwrap_or("Admin");
            sqlx::query(
                "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
                 VALUES ($1, $2, $3, $4, $5, '', 'approved', 'admin', '', $6)",
            )
            .bind(&pid)
            .bind(&club_id)
            .bind(&account_id)
            .bind(display)
            .bind(&email)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| MembershipAdminError::Db)?;
            pid
        };

        let membership_id: String = sqlx::query_scalar(
            "SELECT id FROM club_memberships WHERE pilot_id = $1 AND club_id = $2",
        )
        .bind(&pilot_id)
        .bind(&club_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| MembershipAdminError::Db)?
        .unwrap_or_else(new_id);

        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM club_memberships WHERE pilot_id = $1 AND club_id = $2)",
        )
        .bind(&pilot_id)
        .bind(&club_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| MembershipAdminError::Db)?;
        if exists {
            sqlx::query(
                "UPDATE club_memberships SET role = 'admin', status = 'active' WHERE pilot_id = $1 AND club_id = $2",
            )
            .bind(&pilot_id)
            .bind(&club_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| MembershipAdminError::Db)?;
        } else {
            sqlx::query(
                "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
                 VALUES ($1, $2, $3, 'admin', 'active', $4)",
            )
            .bind(&membership_id)
            .bind(&pilot_id)
            .bind(&club_id)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| MembershipAdminError::Db)?;
        }
        tx.commit().await.map_err(|_| MembershipAdminError::Db)?;
        Ok(AdminInviteAccept {
            account_id,
            membership_id,
            club_id,
        })
    }

    async fn count_active_admins_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        club_id: &str,
    ) -> Result<i64, MembershipAdminError> {
        sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM club_memberships
             WHERE club_id = $1 AND role = 'admin' AND status = 'active'",
        )
        .bind(club_id)
        .fetch_one(&mut **tx)
        .await
        .map_err(|_| MembershipAdminError::Db)
    }

    pub(crate) fn would_violate_last_admin(
        current_role: &str,
        current_status: &str,
        new_role: Option<&str>,
        removing: bool,
        admin_count: i64,
    ) -> bool {
        let is_active_admin = membership_role_is_admin(current_role)
            && MembershipStatus::parse(current_status).allows_club_api();
        if !is_active_admin {
            return false;
        }
        if removing {
            return admin_count <= 1;
        }
        let Some(new_role) = new_role else {
            return admin_count <= 1;
        };
        if membership_role_is_admin(new_role) {
            return false;
        }
        admin_count <= 1
    }

    pub async fn set_membership_role(
        &self,
        club_id: &str,
        membership_id: &str,
        new_role: &str,
    ) -> Result<(), MembershipAdminError> {
        if !matches!(new_role, "admin" | "staff" | "pilot") {
            return Err(MembershipAdminError::InvalidRole);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| MembershipAdminError::Db)?;
        sqlx::query("SELECT id FROM clubs WHERE id = $1 FOR UPDATE")
            .bind(club_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| MembershipAdminError::NotFound)?;
        let row = sqlx::query(
            "SELECT pilot_id, role, status FROM club_memberships WHERE id = $1 AND club_id = $2 FOR UPDATE",
        )
        .bind(membership_id)
        .bind(club_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| MembershipAdminError::Db)?;
        let Some(row) = row else {
            return Err(MembershipAdminError::NotFound);
        };
        let pilot_id: String = row.try_get(0).map_err(|_| MembershipAdminError::Db)?;
        let current_role: String = row.try_get(1).map_err(|_| MembershipAdminError::Db)?;
        let current_status: String = row.try_get(2).map_err(|_| MembershipAdminError::Db)?;
        let admin_count = Self::count_active_admins_tx(&mut tx, club_id).await?;
        if Self::would_violate_last_admin(
            &current_role,
            &current_status,
            Some(new_role),
            false,
            admin_count,
        ) {
            return Err(MembershipAdminError::LastAdmin);
        }
        sqlx::query("UPDATE club_memberships SET role = $1 WHERE id = $2")
            .bind(new_role)
            .bind(membership_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| MembershipAdminError::Db)?;
        sqlx::query("UPDATE pilots SET role = $1 WHERE id = $2")
            .bind(new_role)
            .bind(&pilot_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| MembershipAdminError::Db)?;
        tx.commit().await.map_err(|_| MembershipAdminError::Db)?;
        Ok(())
    }

    pub async fn remove_admin_membership(
        &self,
        club_id: &str,
        membership_id: &str,
    ) -> Result<(), MembershipAdminError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| MembershipAdminError::Db)?;
        sqlx::query("SELECT id FROM clubs WHERE id = $1 FOR UPDATE")
            .bind(club_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| MembershipAdminError::NotFound)?;
        let row = sqlx::query(
            "SELECT pilot_id, role, status FROM club_memberships WHERE id = $1 AND club_id = $2 FOR UPDATE",
        )
        .bind(membership_id)
        .bind(club_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| MembershipAdminError::Db)?;
        let Some(row) = row else {
            return Err(MembershipAdminError::NotFound);
        };
        let pilot_id: String = row.try_get(0).map_err(|_| MembershipAdminError::Db)?;
        let current_role: String = row.try_get(1).map_err(|_| MembershipAdminError::Db)?;
        let current_status: String = row.try_get(2).map_err(|_| MembershipAdminError::Db)?;
        let admin_count = Self::count_active_admins_tx(&mut tx, club_id).await?;
        if Self::would_violate_last_admin(&current_role, &current_status, None, true, admin_count)
        {
            return Err(MembershipAdminError::LastAdmin);
        }
        sqlx::query("UPDATE club_memberships SET status = 'suspended' WHERE id = $1")
            .bind(membership_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| MembershipAdminError::Db)?;
        if membership_role_is_admin(&current_role) {
            sqlx::query("UPDATE pilots SET role = 'pilot' WHERE id = $1")
                .bind(&pilot_id)
                .execute(&mut *tx)
                .await
                .map_err(|_| MembershipAdminError::Db)?;
        }
        tx.commit().await.map_err(|_| MembershipAdminError::Db)?;
        Ok(())
    }
}

#[cfg(test)]
mod unit_last_admin {
    use super::*;

    #[test]
    fn last_admin_guard_single() {
        assert!(ClubDb::would_violate_last_admin(
            "admin",
            "active",
            Some("staff"),
            false,
            1,
        ));
        assert!(!ClubDb::would_violate_last_admin(
            "admin",
            "active",
            Some("staff"),
            false,
            2,
        ));
    }
}