use crate::club::{
    db::{ClubDb, DbError},
    db_club_lifecycle::validate_icao,
    kiosk_qr::{clamp_kiosk_qr_layout, default_kiosk_qr_layout, DEFAULT_PREVIEW_ASPECT},
};
use serde::Serialize;
use sqlx::Row;

/// Validates and normalizes base aerodrome data.
/// Returns (icao, lat, lon) on success, or an error string.
pub fn validate_base_aerodrome(
    icao: Option<&str>,
    lat: Option<f64>,
    lon: Option<f64>,
) -> Result<(String, f64, f64), &'static str> {
    let icao = icao.map(|s| s.trim().to_ascii_uppercase());
    
    // Validate ICAO
    let icao_str = match icao {
        Some(ref s) if !s.is_empty() => {
            if !validate_icao(s) {
                return Err("invalid icao");
            }
            s.clone()
        }
        Some(_) => return Err("invalid icao"),
        None => return Err("invalid icao"),
    };
    
    // Validate lat
    let lat_val = match lat {
        Some(l) if l.is_finite() && l >= -90.0 && l <= 90.0 => l,
        _ => return Err("invalid lat"),
    };
    
    // Validate lon
    let lon_val = match lon {
        Some(l) if l.is_finite() && l >= -180.0 && l <= 180.0 => l,
        _ => return Err("invalid lon"),
    };
    
    Ok((icao_str, lat_val, lon_val))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClubBaseAerodrome {
    pub icao: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

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

    /// Get the club's base aerodrome (icao, lat, lon).
    /// Returns null values when missing or out of range.
    pub async fn club_base_aerodrome(&self, club_id: &str) -> Result<ClubBaseAerodrome, DbError> {
        let row = sqlx::query(
            "SELECT aerodrome_icao, latitude, longitude FROM clubs WHERE id = $1",
        )
        .bind(club_id)
        .fetch_one(&self.pool)
        .await?;
        
        let icao: Option<String> = row.try_get(0).ok().flatten();
        let lat: Option<f64> = row.try_get(1).ok().flatten();
        let lon: Option<f64> = row.try_get(2).ok().flatten();
        
        // Return null if coords are out of range
        let lat_valid = lat.map(|l| l.is_finite() && l >= -90.0 && l <= 90.0).unwrap_or(false);
        let lon_valid = lon.map(|l| l.is_finite() && l >= -180.0 && l <= 180.0).unwrap_or(false);
        
        let (icao_out, lat_out, lon_out) = if lat_valid && lon_valid {
            (icao, lat, lon)
        } else {
            (None, None, None)
        };
        
        Ok(ClubBaseAerodrome {
            icao: icao_out,
            lat: lat_out,
            lon: lon_out,
        })
    }

    /// Set the club's base aerodrome. Validates inputs.
    /// Returns the updated base on success.
    pub async fn set_club_base_aerodrome(
        &self,
        club_id: &str,
        icao: &str,
        lat: f64,
        lon: f64,
    ) -> Result<ClubBaseAerodrome, DbError> {
        // Validate inputs
        let icao_upper = icao.trim().to_ascii_uppercase();
        if !validate_icao(&icao_upper) {
            return Err(DbError::protocol("invalid icao"));
        }
        if !lat.is_finite() || lat < -90.0 || lat > 90.0 {
            return Err(DbError::protocol("invalid lat"));
        }
        if !lon.is_finite() || lon < -180.0 || lon > 180.0 {
            return Err(DbError::protocol("invalid lon"));
        }
        
        sqlx::query(
            "UPDATE clubs SET aerodrome_icao = $1, latitude = $2, longitude = $3 WHERE id = $4",
        )
        .bind(&icao_upper)
        .bind(lat)
        .bind(lon)
        .bind(club_id)
        .execute(&self.pool)
        .await?;
        
        self.club_base_aerodrome(club_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_base_aerodrome_valid_sctb() {
        let result = validate_base_aerodrome(Some("SCTB"), Some(-33.456), Some(-70.548));
        assert!(result.is_ok());
        let (icao, lat, lon) = result.unwrap();
        assert_eq!(icao, "SCTB");
        assert!((lat - (-33.456)).abs() < f64::EPSILON);
        assert!((lon - (-70.548)).abs() < f64::EPSILON);
    }

    #[test]
    fn validate_base_aerodrome_lowercase_icao() {
        let result = validate_base_aerodrome(Some("sctb"), Some(-33.456), Some(-70.548));
        assert!(result.is_ok());
        let (icao, _, _) = result.unwrap();
        assert_eq!(icao, "SCTB");
    }

    #[test]
    fn validate_base_aerodrome_invalid_icao_too_short() {
        let result = validate_base_aerodrome(Some("SC"), Some(-33.456), Some(-70.548));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "invalid icao");
    }

    #[test]
    fn validate_base_aerodrome_invalid_icao_with_digit() {
        let result = validate_base_aerodrome(Some("SCT1"), Some(-33.456), Some(-70.548));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "invalid icao");
    }

    #[test]
    fn validate_base_aerodrome_lat_out_of_range_high() {
        let result = validate_base_aerodrome(Some("SCTB"), Some(91.0), Some(-70.548));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "invalid lat");
    }

    #[test]
    fn validate_base_aerodrome_lat_out_of_range_low() {
        let result = validate_base_aerodrome(Some("SCTB"), Some(-91.0), Some(-70.548));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "invalid lat");
    }

    #[test]
    fn validate_base_aerodrome_lon_out_of_range_high() {
        let result = validate_base_aerodrome(Some("SCTB"), Some(-33.456), Some(181.0));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "invalid lon");
    }

    #[test]
    fn validate_base_aerodrome_lon_out_of_range_low() {
        let result = validate_base_aerodrome(Some("SCTB"), Some(-33.456), Some(-181.0));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "invalid lon");
    }

    #[test]
    fn validate_base_aerodrome_nan_lat() {
        let result = validate_base_aerodrome(Some("SCTB"), Some(f64::NAN), Some(-70.548));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "invalid lat");
    }

    #[test]
    fn validate_base_aerodrome_nan_lon() {
        let result = validate_base_aerodrome(Some("SCTB"), Some(-33.456), Some(f64::NAN));
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "invalid lon");
    }
}
