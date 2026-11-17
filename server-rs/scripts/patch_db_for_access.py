#!/usr/bin/env python3
from pathlib import Path
import re
import sys

p = Path(__file__).resolve().parents[1] / "src/club/db.rs"
text = p.read_text()
if "rusqlite" not in text.split("\n")[10]:
    sys.exit("db.rs does not look like sqlite")
text = text.replace(
    'const MIGRATION_003: &str = include_str!("../../sqlite-legacy/003_device_pair_qr.sql");\n',
    'const MIGRATION_003: &str = include_str!("../../sqlite-legacy/003_device_pair_qr.sql");\n'
    'const MIGRATION_004: &str = include_str!("../../sqlite-legacy/004_fix_meta.sql");\n'
    'const MIGRATION_005: &str = include_str!("../../sqlite-legacy/005_access_requests.sql");\n',
)
text = text.replace(
    "    fn with_conn<F, T>(&self, f: F) -> Result<T, rusqlite::Error>\n    where\n        F: FnOnce(&Connection) -> Result<T, rusqlite::Error>,\n    {\n        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());\n        f(&conn)\n    }\n",
    "    pub(crate) fn with_conn<F, T>(&self, f: F) -> Result<T, rusqlite::Error>\n    where\n        F: FnOnce(&Connection) -> Result<T, rusqlite::Error>,\n    {\n        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());\n        f(&conn)\n    }\n\n    pub(crate) fn with_conn_any<T, E, F>(&self, f: F) -> Result<T, E>\n    where\n        F: FnOnce(&Connection) -> Result<T, E>,\n    {\n        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());\n        f(&conn)\n    }\n",
)
text = text.replace(
    """            if version < 3 {
                conn.execute_batch(MIGRATION_003)?;
                conn.execute("INSERT INTO schema_migrations (version) VALUES (3)", [])?;
            }
            Ok(())
        })
    }""",
    """            if version < 3 {
                conn.execute_batch(MIGRATION_003)?;
                conn.execute("INSERT INTO schema_migrations (version) VALUES (3)", [])?;
            }
            if version < 4 {
                conn.execute_batch(MIGRATION_004)?;
                conn.execute("INSERT INTO schema_migrations (version) VALUES (4)", [])?;
            }
            if version < 5 {
                conn.execute_batch(MIGRATION_005)?;
                conn.execute("INSERT INTO schema_migrations (version) VALUES (5)", [])?;
            }
            Ok(())
        })
    }""",
)
text = text.replace(
    """        let checkout = self
            .with_conn(|conn| Self::checkout_ok(conn, pilot_id, &aircraft_id, now))
            .unwrap_or(false);
        if !checkout {
            return Err(StartFlightError::NoCheckout);
        }""",
    """        let type_code: String = self
            .with_conn(|conn| {
                conn.query_row(
                    \"SELECT t.code FROM aircraft a
                     JOIN aircraft_types t ON t.id = a.type_id
                     WHERE a.id = ?1\",
                    params![aircraft_id],
                    |r| r.get(0),
                )
            })
            .map_err(|_| StartFlightError::NoTail)?;
        let checkout = self
            .with_conn(|conn| Self::checkout_ok(conn, pilot_id, &aircraft_id, now))
            .unwrap_or(false);
        if !checkout {
            return Err(StartFlightError::NotCleared {
                tail: tail.to_string(),
                type_code,
            });
        }""",
)
text = text.replace(
    """#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartFlightError {
    NoTail,
    NoCheckout,
    NoCheckin,
    AlreadyActive,
    DeviceMismatch,
}""",
    """#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartFlightError {
    NoTail,
    NotCleared { tail: String, type_code: String },
    NoCheckin,
    AlreadyActive,
    DeviceMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutError {
    UnknownTail,
    UnknownType,
    Internal,
}

impl From<rusqlite::Error> for CheckoutError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Internal
    }
}""",
)
text, n = re.subn(
    r"    pub fn add_checkout\([\s\S]*?        \}\)\n    \}\n\n    pub fn list_pending_pilots",
    """    pub fn add_checkout(
        &self,
        pilot_id: &str,
        type_code: &str,
        tail: Option<&str>,
        expires_at: Option<i64>,
    ) -> Result<(), CheckoutError> {
        self.add_checkout_typed(pilot_id, type_code, tail, expires_at)
    }

    pub fn list_pending_pilots""",
    text,
    count=1,
)
if n != 1:
    sys.exit(f"add_checkout patch failed: {n}")
text = text.replace(
    "#[cfg(test)]\nmod tests {\n    use super::*;\n    use crate::club::visibility::{FlightVisibility, PilotRole, PilotStatus, Viewer};",
    "#[cfg(test)]\nmod tests {\n    use super::*;\n    use crate::club::visibility::{FlightVisibility, PilotRole, PilotStatus, Viewer};",
)
tests = '''
    fn empty_club_with_approved_pilot(db: &ClubDb) -> (String, String) {
        let club = db.default_club_id().unwrap();
        let p = db.register(&club, "P", "p@test", "L1", "password12").unwrap();
        db.approve_pilot(&p).unwrap();
        (club, p)
    }

    #[test]
    fn access_request_approve_then_start() {
        let db = ClubDb::open_in_memory().unwrap();
        let (club, p) = empty_club_with_approved_pilot(&db);
        let admin = db.register(&club, "A", "admin@test", "", "password12").unwrap();
        db.approve_pilot(&admin).unwrap();
        let dev = db.create_device(&p, "phone").unwrap().0;
        db.issue_open_checkin(&p, &dev, &club, now_ms()).unwrap();
        let rid = db
            .create_access_request(&p, "CC-ABC", None, Some("C152"), now_ms())
            .unwrap();
        db.approve_access_request(&admin, &rid, None, now_ms())
            .unwrap();
        let tails = db.me_tails(&p, now_ms()).unwrap();
        assert!(tails.iter().any(|t| t.tail == "CC-ABC"));
        db.issue_open_checkin(&p, &dev, &club, now_ms()).unwrap();
        assert!(db
            .start_flight(
                &p,
                &dev,
                "CC-ABC",
                FlightVisibility::Club,
                false,
                now_ms()
            )
            .is_ok());
    }

    #[test]
    fn access_request_reject_blocks_start() {
        let db = ClubDb::open_in_memory().unwrap();
        let (club, p) = empty_club_with_approved_pilot(&db);
        let admin = db.register(&club, "A", "admin@test", "", "password12").unwrap();
        db.approve_pilot(&admin).unwrap();
        let dev = db.create_device(&p, "phone").unwrap().0;
        db.issue_open_checkin(&p, &dev, &club, now_ms()).unwrap();
        let rid = db
            .create_access_request(&p, "CC-XYZ", None, Some("DA20"), now_ms())
            .unwrap();
        db.reject_access_request(&admin, &rid, "not yet", now_ms())
            .unwrap();
        let list = db.list_pilot_access_requests(&p);
        assert_eq!(list[0].status, "rejected");
        assert_eq!(list[0].reject_reason.as_deref(), Some("not yet"));
        db.upsert_type(&club, "DA20").unwrap();
        db.upsert_aircraft(&club, "CC-XYZ", "DA20", true).unwrap();
        assert!(matches!(
            db.start_flight(&p, &dev, "CC-XYZ", FlightVisibility::Club, false, now_ms()),
            Err(StartFlightError::NotCleared { .. })
        ));
    }

    #[test]
    fn pending_access_request_cannot_start_without_checkout() {
        let db = ClubDb::open_in_memory().unwrap();
        let (club, p) = empty_club_with_approved_pilot(&db);
        let dev = db.create_device(&p, "phone").unwrap().0;
        db.issue_open_checkin(&p, &dev, &club, now_ms()).unwrap();
        db.create_access_request(&p, "CC-NOP", None, Some("T1"), now_ms())
            .unwrap();
        db.upsert_type(&club, "T1").unwrap();
        db.upsert_aircraft(&club, "CC-NOP", "T1", true).unwrap();
        assert!(matches!(
            db.start_flight(&p, &dev, "CC-NOP", FlightVisibility::Club, false, now_ms()),
            Err(StartFlightError::NotCleared { .. })
        ));
    }
'''
text = re.sub(
    r"    fn add_checkout_unknown_tail_errors\(\) \{[\s\S]*?\n    \}\n\}\n",
    tests.strip() + "\n\n    #[test]\n    fn add_checkout_unknown_tail_errors() {\n        let db = ClubDb::open_in_memory().unwrap();\n        let (club, a) = empty_club_with_approved_pilot(&db);\n        db.upsert_type(&club, \"C152\").unwrap();\n        assert!(matches!(\n            db.add_checkout(&a, \"C152\", Some(\"NOPE\"), None),\n            Err(CheckoutError::UnknownTail)\n        ));\n    }\n}\n",
    text,
    count=1,
)
p.write_text(text)
print("patched", p, "lines", text.count("\n"))