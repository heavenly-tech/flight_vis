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
}

impl ClubDb {
    pub async fn kiosk_qr_layout(&self, club_id: &str) -> Result<KioskQrLayoutRow, DbError> {
        let row = sqlx::query(
            "SELECT kiosk_qr_x, kiosk_qr_y, kiosk_qr_size, kiosk_layout_rev FROM clubs WHERE id = $1",
        )
        .bind(club_id)
        .fetch_one(&self.pool)
        .await?;
        let x: Option<f64> = row.try_get(0)?;
        let y: Option<f64> = row.try_get(1)?;
        let size: Option<f64> = row.try_get(2)?;
        let rev: i64 = row.try_get(3)?;
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
        let needs: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM clubs WHERE id = $1 AND kiosk_qr_x IS NULL",
        )
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