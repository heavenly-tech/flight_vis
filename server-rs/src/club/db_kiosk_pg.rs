use crate::club::{
    db::{ClubDb, DbError},
    kiosk_qr::{clamp_kiosk_qr_layout, default_kiosk_qr_layout, DEFAULT_PREVIEW_ASPECT},
};
use serde::Serialize;
use sqlx::Row;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KioskQrLayoutRow {
    pub x: f64,
    pub y: f64,
    pub size: f64,
    pub rev: i64,
    pub checkin_qr_visible: bool,
}

impl ClubDb {
    pub async fn kiosk_qr_layout(&self, club_id: &str) -> Result<KioskQrLayoutRow, DbError> {
        let row = sqlx::query(
            "SELECT kiosk_qr_x, kiosk_qr_y, kiosk_qr_size, kiosk_layout_rev, kiosk_checkin_qr_visible
             FROM clubs WHERE id = $1",
        )
        .bind(club_id)
        .fetch_one(&self.pool)
        .await?;
        let x: Option<f64> = row.try_get(0)?;
        let y: Option<f64> = row.try_get(1)?;
        let size: Option<f64> = row.try_get(2)?;
        let rev: i64 = row.try_get(3)?;
        let checkin_qr_visible: bool = row.try_get(4)?;
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
            checkin_qr_visible,
        })
    }

    pub async fn set_kiosk_checkin_qr_visible(
        &self,
        club_id: &str,
        visible: bool,
    ) -> Result<KioskQrLayoutRow, DbError> {
        sqlx::query(
            "UPDATE clubs SET kiosk_checkin_qr_visible = $1, kiosk_layout_rev = kiosk_layout_rev + 1
             WHERE id = $2",
        )
        .bind(visible)
        .bind(club_id)
        .execute(&self.pool)
        .await?;
        self.kiosk_qr_layout(club_id).await
    }

    pub async fn club_checkin_ok_for_qr_window(
        &self,
        club_id: &str,
        since_ms: i64,
        now_ms: i64,
    ) -> Result<bool, DbError> {
        use crate::club::checkin;
        let (secret, _) = self.club_checkin_settings(club_id).await?;
        let mut step_lo = checkin::current_step(since_ms);
        if step_lo > 0 {
            step_lo -= 1;
        }
        let step_hi = checkin::current_step(now_ms) + 1;
        let mut digests: Vec<String> = Vec::new();
        for step in step_lo..=step_hi {
            let token = checkin::mint_checkin_token(&secret, club_id, step);
            digests.push(checkin::token_digest(&token));
        }
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM checkins
             WHERE club_id = $1 AND outcome = 'ok' AND created_at > $2 AND token_digest = ANY($3)",
        )
        .bind(club_id)
        .bind(since_ms)
        .bind(&digests)
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }

    pub async fn set_kiosk_qr_layout(
        &self,
        club_id: &str,
        x: f64,
        y: f64,
        size: f64,
        aspect: f64,
    ) -> Result<KioskQrLayoutRow, DbError> {
        let layout = clamp_kiosk_qr_layout(x, y, size, aspect);
        sqlx::query(
            "UPDATE clubs SET kiosk_qr_x = $1, kiosk_qr_y = $2, kiosk_qr_size = $3,
             kiosk_layout_rev = kiosk_layout_rev + 1 WHERE id = $4",
        )
        .bind(layout.x)
        .bind(layout.y)
        .bind(layout.size)
        .bind(club_id)
        .execute(&self.pool)
        .await?;
        self.kiosk_qr_layout(club_id).await
    }

    pub async fn reset_kiosk_qr_layout(
        &self,
        club_id: &str,
        aspect: f64,
    ) -> Result<KioskQrLayoutRow, DbError> {
        let layout = default_kiosk_qr_layout(aspect);
        sqlx::query(
            "UPDATE clubs SET kiosk_qr_x = $1, kiosk_qr_y = $2, kiosk_qr_size = $3,
             kiosk_layout_rev = kiosk_layout_rev + 1 WHERE id = $4",
        )
        .bind(layout.x)
        .bind(layout.y)
        .bind(layout.size)
        .bind(club_id)
        .execute(&self.pool)
        .await?;
        self.kiosk_qr_layout(club_id).await
    }

    pub async fn ensure_kiosk_qr_defaults(&self, club_id: &str) -> Result<(), DbError> {
        let needs: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM clubs WHERE id = $1 AND kiosk_qr_x IS NULL")
                .bind(club_id)
                .fetch_optional(&self.pool)
                .await?;
        if needs.is_some() {
            let d = default_kiosk_qr_layout(DEFAULT_PREVIEW_ASPECT);
            sqlx::query(
                "UPDATE clubs SET kiosk_qr_x = $1, kiosk_qr_y = $2, kiosk_qr_size = $3 WHERE id = $4",
            )
            .bind(d.x)
            .bind(d.y)
            .bind(d.size)
            .bind(club_id)
            .execute(&self.pool)
            .await?;
        }
        Ok(())
    }
}
