use crate::club::{
    db::ClubDb,
    kiosk_qr::{clamp_kiosk_qr_layout, default_kiosk_qr_layout, DEFAULT_PREVIEW_ASPECT},
};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KioskQrLayoutRow {
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub rev: i64,
}

impl ClubDb {
    pub fn kiosk_qr_layout(&self, club_id: &str) -> Result<KioskQrLayoutRow, rusqlite::Error> {
        self.with_conn(|conn| {
            let row = conn.query_row(
                    "SELECT kiosk_qr_x, kiosk_qr_y, kiosk_qr_size, kiosk_layout_rev FROM clubs WHERE id = ?1",
                    params![club_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                );
            let (x, y, size, rev): (Option<f64>, Option<f64>, Option<f64>, i64) = row?;
            let layout = match (x, y, size) {
                (Some(x), Some(y), Some(size)) => {
                    clamp_kiosk_qr_layout(x, y, size, DEFAULT_PREVIEW_ASPECT)
                }
                _ => default_kiosk_qr_layout(DEFAULT_PREVIEW_ASPECT),
            };
            Ok(KioskQrLayoutRow {
                x: layout.x,
                y: layout.y,
                size: layout.size,
                rev,
            })
        })
    }

    pub fn set_kiosk_qr_layout(
        &self,
        club_id: &str,
        x: f64,
        y: f64,
        size: f64,
        aspect: f64,
    ) -> Result<KioskQrLayoutRow, rusqlite::Error> {
        let layout = clamp_kiosk_qr_layout(x, y, size, aspect);
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE clubs SET kiosk_qr_x = ?1, kiosk_qr_y = ?2, kiosk_qr_size = ?3,
                 kiosk_layout_rev = kiosk_layout_rev + 1 WHERE id = ?4",
                params![layout.x, layout.y, layout.size, club_id],
            )?;
            Ok(())
        })?;
        self.kiosk_qr_layout(club_id)
    }

    pub fn reset_kiosk_qr_layout(
        &self,
        club_id: &str,
        aspect: f64,
    ) -> Result<KioskQrLayoutRow, rusqlite::Error> {
        let layout = default_kiosk_qr_layout(aspect);
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE clubs SET kiosk_qr_x = ?1, kiosk_qr_y = ?2, kiosk_qr_size = ?3,
                 kiosk_layout_rev = kiosk_layout_rev + 1 WHERE id = ?4",
                params![layout.x, layout.y, layout.size, club_id],
            )?;
            Ok(())
        })?;
        self.kiosk_qr_layout(club_id)
    }

    pub fn ensure_kiosk_qr_defaults(&self, club_id: &str) -> Result<(), rusqlite::Error> {
        self.with_conn(|conn| {
            let needs: Option<i64> = conn
                .query_row(
                    "SELECT 1 FROM clubs WHERE id = ?1 AND kiosk_qr_x IS NULL",
                    params![club_id],
                    |r| r.get(0),
                )
                .optional()?;
            if needs.is_some() {
                let d = default_kiosk_qr_layout(DEFAULT_PREVIEW_ASPECT);
                conn.execute(
                    "UPDATE clubs SET kiosk_qr_x = ?1, kiosk_qr_y = ?2, kiosk_qr_size = ?3 WHERE id = ?4",
                    params![d.x, d.y, d.size, club_id],
                )?;
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::club::db::ClubDb;

    #[test]
    fn persistence_round_trip() {
        let db = ClubDb::open_in_memory().unwrap();
        let club = db.default_club_id().unwrap();
        db.ensure_kiosk_qr_defaults(&club).unwrap();
        let saved = db
            .set_kiosk_qr_layout(&club, 0.1, 0.15, 0.25, 16.0 / 9.0)
            .unwrap();
        assert_eq!(saved.size, 0.25);
        let read = db.kiosk_qr_layout(&club).unwrap();
        assert!((read.x - saved.x).abs() < 1e-9);
        assert_eq!(read.rev, saved.rev);
        let reset = db.reset_kiosk_qr_layout(&club, 16.0 / 9.0).unwrap();
        assert!(reset.rev > saved.rev);
    }
}
