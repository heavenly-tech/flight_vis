use crate::club::db::{now_ms, ClubDb};
use rusqlite::params;

impl ClubDb {
    pub fn suppress_email(&self, email: &str, reason: &str) -> Result<(), rusqlite::Error> {
        let email = email.trim().to_ascii_lowercase();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO email_suppressions (email, reason, created_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(email) DO UPDATE SET reason = excluded.reason, created_at = excluded.created_at",
                params![email, reason, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn email_suppressed(&self, email: &str) -> bool {
        let email = email.trim().to_ascii_lowercase();
        self.with_conn(|conn| {
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM email_suppressions WHERE email = ?1",
                params![email],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        })
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::club::db::ClubDb;

    #[test]
    fn bounced_address_suppressed() {
        let db = ClubDb::open_in_memory().unwrap();
        db.suppress_email("bad@test", "bounce").unwrap();
        assert!(db.email_suppressed("bad@test"));
        assert!(!db.email_suppressed("good@test"));
    }
}