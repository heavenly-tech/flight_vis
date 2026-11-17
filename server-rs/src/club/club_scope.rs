use crate::club::db::ClubDb;
use rusqlite::{params, OptionalExtension};

#[derive(Debug, Clone)]
pub struct ClubContext {
    pub club_id: String,
    pub slug: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClubScopeError {
    NotFound,
    Forbidden,
}

impl ClubDb {
    pub fn club_id_for_slug(&self, slug: &str) -> Option<String> {
        let slug = slug.trim();
        if slug.is_empty() {
            return None;
        }
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT id FROM clubs WHERE slug = ?1",
                params![slug],
                |r| r.get(0),
            )
            .optional()
        })
        .ok()
        .flatten()
    }

    pub fn club_slug(&self, club_id: &str) -> Option<String> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT slug FROM clubs WHERE id = ?1",
                params![club_id],
                |r| r.get(0),
            )
            .optional()
        })
        .ok()
        .flatten()
    }

    pub fn resolve_club_context(&self, slug: &str) -> Result<ClubContext, ClubScopeError> {
        let Some(club_id) = self.club_id_for_slug(slug) else {
            return Err(ClubScopeError::NotFound);
        };
        Ok(ClubContext {
            club_id,
            slug: slug.to_string(),
        })
    }

  pub fn is_superadmin(&self, pilot_id: &str) -> bool {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT global_role FROM pilots WHERE id = ?1",
                params![pilot_id],
                |r| r.get::<_, String>(0),
            )
        })
        .map(|r| r == "superadmin")
        .unwrap_or(false)
    }

    pub fn membership_in_club(
        &self,
        pilot_id: &str,
        club_id: &str,
    ) -> Option<(String, String)> {
        self.with_conn(|conn| {
            conn.query_row(
                "SELECT role, status FROM club_memberships
                 WHERE pilot_id = ?1 AND club_id = ?2",
                params![pilot_id, club_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
        .ok()
        .flatten()
    }

    pub fn require_club_admin(
        &self,
        pilot_id: &str,
        club_id: &str,
    ) -> Result<(), ClubScopeError> {
        if self.is_superadmin(pilot_id) {
            return Ok(());
        }
        match self.membership_in_club(pilot_id, club_id) {
            Some((role, status)) if role == "admin" && status == "approved" => Ok(()),
            _ => Err(ClubScopeError::Forbidden),
        }
    }

    pub fn ensure_membership_row(
        &self,
        pilot_id: &str,
        club_id: &str,
        role: &str,
        status: &str,
    ) -> Result<(), rusqlite::Error> {
        use crate::club::auth::new_id;
        use crate::club::db::now_ms;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(pilot_id, club_id) DO UPDATE SET
                   role = excluded.role,
                   status = excluded.status",
                params![new_id(), pilot_id, club_id, role, status, now_ms()],
            )?;
            Ok(())
        })
    }
}

pub fn checkin_qr_path(slug: &str, token: &str) -> String {
    format!("/c/{slug}/q/{token}")
}