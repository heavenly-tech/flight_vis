//! Live aircraft trail settings, windowing/downsampling, nearby-public settings, and bounded DB queries.

use crate::club::{
    db::{ClubDb, DbError},
    map::TailLivePosition,
};
use serde::Serialize;
use sqlx::Row;
use std::collections::HashMap;

/// Wire/DB value meaning "no trail rendered".
pub const TRAIL_OFF: i16 = 0;
/// Wire/DB value meaning "all recent fixes" (server-capped at `MAX_TRAIL_WINDOW_MIN`).
pub const TRAIL_ALL: i16 = -1;
/// Default club trail window in minutes when not customized.
pub const DEFAULT_TRAIL_MINUTES: i16 = 5;
/// Canonical ordered list of allowed trail values (Off, 2..30 min, All).
#[allow(dead_code)]
pub const ALLOWED_TRAIL_MINUTES: [i16; 7] = [TRAIL_OFF, 2, 5, 10, 15, 30, TRAIL_ALL];

/// Maximum lookback window (minutes back from a flight's latest fix) when effective value is All (-1).
pub const MAX_TRAIL_WINDOW_MIN: i64 = 120;
/// Maximum points returned per flight after even-by-time downsampling (preserving first and last).
pub const MAX_TRAIL_POINTS: usize = 300;
/// Maximum number of live flights returned in a single `/trails` or `/api/me/live-trails` response.
pub const MAX_TRAIL_FLIGHTS: usize = 50;
/// Maximum age of a flight's latest fix to count as live for pilot live trails and nearby public flights (10 min).
pub const LIVE_FIX_MAX_AGE_MS: i64 = 10 * 60 * 1000;

/// Default toggle for showing nearby public flights from other clubs on the club view.
pub const DEFAULT_SHOW_NEARBY_PUBLIC: bool = true;
/// Default search radius (km) around the club's base aerodrome for nearby public flights.
pub const DEFAULT_NEARBY_PUBLIC_RADIUS_KM: i16 = 50;
/// Minimum allowed nearby public search radius (km).
pub const MIN_NEARBY_PUBLIC_RADIUS_KM: i16 = 5;
/// Maximum allowed nearby public search radius (km).
pub const MAX_NEARBY_PUBLIC_RADIUS_KM: i16 = 500;
/// Maximum number of nearby public flights from other clubs included on the club view (nearest first).
pub const MAX_NEARBY_PUBLIC_FLIGHTS: usize = 30;

const MS_PER_MIN: i64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i16)]
pub enum TrailMinutes {
    Off = TRAIL_OFF,
    Min2 = 2,
    Min5 = 5,
    Min10 = 10,
    Min15 = 15,
    Min30 = 30,
    All = TRAIL_ALL,
}

impl TrailMinutes {
    pub fn from_i16(value: i16) -> Option<Self> {
        match value {
            TRAIL_OFF => Some(Self::Off),
            2 => Some(Self::Min2),
            5 => Some(Self::Min5),
            10 => Some(Self::Min10),
            15 => Some(Self::Min15),
            30 => Some(Self::Min30),
            TRAIL_ALL => Some(Self::All),
            _ => None,
        }
    }

    pub fn from_i64(value: i64) -> Option<Self> {
        i16::try_from(value).ok().and_then(Self::from_i16)
    }

    /// Strictly parses a JSON number into `TrailMinutes` (rejects floats, strings, booleans, null).
    pub fn from_json_value(value: &serde_json::Value) -> Result<Self, &'static str> {
        let Some(n) = value.as_i64() else {
            return Err("invalid trailMinutes");
        };
        Self::from_i64(n).ok_or("invalid trailMinutes")
    }

    /// Parses an optional JSON value: `null` -> `Ok(None)` (inherit club default), valid int -> `Ok(Some(_))`.
    pub fn from_optional_json_value(
        value: &serde_json::Value,
    ) -> Result<Option<Self>, &'static str> {
        if value.is_null() {
            return Ok(None);
        }
        Self::from_json_value(value).map(Some)
    }

    pub const fn as_i16(self) -> i16 {
        self as i16
    }

    /// Resolves a flight's effective trail setting: flight override if present, otherwise club default.
    pub fn effective(flight_override: Option<i16>, club_default: i16) -> Self {
        if let Some(raw) = flight_override {
            if let Some(parsed) = Self::from_i16(raw) {
                return parsed;
            }
        }
        Self::from_i16(club_default).unwrap_or(Self::Min5)
    }

    /// Lookback window in milliseconds (`None` for `Off`, capped at `MAX_TRAIL_WINDOW_MIN` for `All`).
    pub const fn window_ms(self) -> Option<i64> {
        match self {
            Self::Off => None,
            Self::Min2 => Some(2 * MS_PER_MIN),
            Self::Min5 => Some(5 * MS_PER_MIN),
            Self::Min10 => Some(10 * MS_PER_MIN),
            Self::Min15 => Some(15 * MS_PER_MIN),
            Self::Min30 => Some(30 * MS_PER_MIN),
            Self::All => Some(MAX_TRAIL_WINDOW_MIN * MS_PER_MIN),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClubNearbyPublicSettings {
    pub show_nearby_public: bool,
    pub nearby_public_radius_km: i16,
}

impl Default for ClubNearbyPublicSettings {
    fn default() -> Self {
        Self {
            show_nearby_public: DEFAULT_SHOW_NEARBY_PUBLIC,
            nearby_public_radius_km: DEFAULT_NEARBY_PUBLIC_RADIUS_KM,
        }
    }
}

/// Strictly validates a JSON integer for `nearbyPublicRadiusKm` (5..=500).
pub fn parse_nearby_public_radius_km(value: &serde_json::Value) -> Result<i16, &'static str> {
    let Some(n) = value.as_i64() else {
        return Err("invalid nearbyPublicRadiusKm");
    };
    let Ok(km) = i16::try_from(n) else {
        return Err("invalid nearbyPublicRadiusKm");
    };
    if !(MIN_NEARBY_PUBLIC_RADIUS_KM..=MAX_NEARBY_PUBLIC_RADIUS_KM).contains(&km) {
        return Err("invalid nearbyPublicRadiusKm");
    }
    Ok(km)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlightTrailError {
    NotFound,
    Forbidden,
    NotActive,
    Db,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlightTrail {
    pub flight_id: String,
    pub tail: String,
    pub tail_source: &'static str,
    pub trail_minutes: i16,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub non_club: bool,
    pub points: Vec<(i64, f64, f64)>,
}

/// `(t_ms, lat, lon, alt_m)` point tuple used for live pilot trail responses.
pub type TrailPointWithAlt = (i64, f64, f64, Option<f64>);

/// Shared core windowing & even-by-time downsampling for `(t_ms, lat, lon, alt)` points.
///
/// - `TrailMinutes::Off` always returns an empty vector.
/// - `TrailMinutes::All` caps the lookback window at `MAX_TRAIL_WINDOW_MIN` (120 min) from the latest fix.
/// - Sanitizes non-finite altitudes to `None` and drops non-finite or out-of-range coordinates.
/// - Preserves the first and last points in the window when downsampling to `max_points`.
pub fn window_and_downsample_trail_with_alt(
    raw_points: &[TrailPointWithAlt],
    trail_minutes: TrailMinutes,
    max_points: usize,
) -> Vec<TrailPointWithAlt> {
    let Some(window_ms) = trail_minutes.window_ms() else {
        return Vec::new();
    };
    if raw_points.is_empty() || max_points == 0 {
        return Vec::new();
    }

    let mut clean: Vec<(i64, f64, f64, Option<f64>)> = raw_points
        .iter()
        .copied()
        .filter(|&(_, lat, lon, _)| {
            lat.is_finite()
                && lon.is_finite()
                && (-90.0..=90.0).contains(&lat)
                && (-180.0..=180.0).contains(&lon)
        })
        .map(|(t, lat, lon, alt)| (t, lat, lon, alt.filter(|a| a.is_finite())))
        .collect();
    if clean.is_empty() {
        return Vec::new();
    }

    clean.sort_by_key(|p| p.0);

    // Deduplicate identical timestamps, keeping the latest record for a given t_ms.
    let mut deduped: Vec<(i64, f64, f64, Option<f64>)> = Vec::with_capacity(clean.len());
    for pt in clean {
        if let Some(last) = deduped.last_mut() {
            if last.0 == pt.0 {
                *last = pt;
                continue;
            }
        }
        deduped.push(pt);
    }

    let latest_t = deduped.last().expect("non-empty").0;
    let cutoff = latest_t.saturating_sub(window_ms);
    let start_idx = deduped.partition_point(|p| p.0 < cutoff);
    let windowed = &deduped[start_idx..];

    if windowed.len() <= max_points {
        return windowed.to_vec();
    }
    if max_points == 1 {
        return vec![*windowed.last().expect("non-empty")];
    }

    let n = windowed.len();
    let t_first = windowed[0].0;
    let t_last = windowed[n - 1].0;
    let mut out = Vec::with_capacity(max_points);
    out.push(windowed[0]);
    let mut last_picked: usize = 0;

    let slots = (max_points - 1) as i128;
    let span = (t_last - t_first) as i128;

    for i in 1..(max_points - 1) {
        let target_t = t_first + ((span * i as i128) / slots) as i64;
        let idx = windowed.partition_point(|p| p.0 < target_t);
        let best = if idx == 0 {
            0
        } else if idx >= n {
            n - 1
        } else {
            let d_prev = (target_t - windowed[idx - 1].0).unsigned_abs();
            let d_curr = (windowed[idx].0 - target_t).unsigned_abs();
            if d_prev <= d_curr {
                idx - 1
            } else {
                idx
            }
        };
        if best > last_picked && best < n - 1 {
            out.push(windowed[best]);
            last_picked = best;
        }
    }

    out.push(windowed[n - 1]);
    out
}

/// Filters, time-windows, and evenly downsamples `(t_ms, lat, lon)` fixes to at most `max_points`.
pub fn window_and_downsample_trail(
    raw_points: &[(i64, f64, f64)],
    trail_minutes: TrailMinutes,
    max_points: usize,
) -> Vec<(i64, f64, f64)> {
    let with_alt: Vec<(i64, f64, f64, Option<f64>)> = raw_points
        .iter()
        .map(|&(t, lat, lon)| (t, lat, lon, None))
        .collect();
    window_and_downsample_trail_with_alt(&with_alt, trail_minutes, max_points)
        .into_iter()
        .map(|(t, lat, lon, _)| (t, lat, lon))
        .collect()
}

impl ClubDb {
    pub async fn club_trail_minutes_default(&self, club_id: &str) -> Result<TrailMinutes, DbError> {
        let raw: i16 = sqlx::query_scalar("SELECT trail_minutes_default FROM clubs WHERE id = $1")
            .bind(club_id)
            .fetch_one(&self.pool)
            .await?;
        Ok(TrailMinutes::from_i16(raw).unwrap_or(TrailMinutes::Min5))
    }

    pub async fn set_club_trail_minutes_default(
        &self,
        club_id: &str,
        value: TrailMinutes,
    ) -> Result<TrailMinutes, DbError> {
        sqlx::query("UPDATE clubs SET trail_minutes_default = $1 WHERE id = $2")
            .bind(value.as_i16())
            .bind(club_id)
            .execute(&self.pool)
            .await?;
        Ok(value)
    }

    pub async fn club_nearby_public_settings(
        &self,
        club_id: &str,
    ) -> Result<ClubNearbyPublicSettings, DbError> {
        let row = sqlx::query(
            "SELECT show_nearby_public, nearby_public_radius_km FROM clubs WHERE id = $1",
        )
        .bind(club_id)
        .fetch_one(&self.pool)
        .await?;
        let show: bool = row.try_get(0).unwrap_or(DEFAULT_SHOW_NEARBY_PUBLIC);
        let radius: i16 = row.try_get(1).unwrap_or(DEFAULT_NEARBY_PUBLIC_RADIUS_KM);
        Ok(ClubNearbyPublicSettings {
            show_nearby_public: show,
            nearby_public_radius_km: radius
                .clamp(MIN_NEARBY_PUBLIC_RADIUS_KM, MAX_NEARBY_PUBLIC_RADIUS_KM),
        })
    }

    pub async fn set_club_nearby_public_settings(
        &self,
        club_id: &str,
        show_nearby_public: Option<bool>,
        radius_km: Option<i16>,
    ) -> Result<ClubNearbyPublicSettings, DbError> {
        let current = self.club_nearby_public_settings(club_id).await?;
        let next_show = show_nearby_public.unwrap_or(current.show_nearby_public);
        let next_radius = radius_km.unwrap_or(current.nearby_public_radius_km);
        if !(MIN_NEARBY_PUBLIC_RADIUS_KM..=MAX_NEARBY_PUBLIC_RADIUS_KM).contains(&next_radius) {
            return Err(DbError::protocol("invalid nearbyPublicRadiusKm"));
        }
        sqlx::query(
            "UPDATE clubs
             SET show_nearby_public = $1, nearby_public_radius_km = $2
             WHERE id = $3",
        )
        .bind(next_show)
        .bind(next_radius)
        .bind(club_id)
        .execute(&self.pool)
        .await?;
        Ok(ClubNearbyPublicSettings {
            show_nearby_public: next_show,
            nearby_public_radius_km: next_radius,
        })
    }

    pub async fn set_flight_trail_minutes(
        &self,
        pilot_id: &str,
        flight_id: &str,
        value: Option<TrailMinutes>,
    ) -> Result<(Option<TrailMinutes>, TrailMinutes), FlightTrailError> {
        let row = sqlx::query(
            "SELECT f.pilot_id, f.status, COALESCE(c.trail_minutes_default, $2)
             FROM flights f
             LEFT JOIN aircraft a ON a.id = f.aircraft_id
             LEFT JOIN clubs c ON c.id = COALESCE(f.club_id, a.club_id)
             WHERE f.id = $1",
        )
        .bind(flight_id)
        .bind(DEFAULT_TRAIL_MINUTES)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| FlightTrailError::Db)?;

        let Some(row) = row else {
            return Err(FlightTrailError::NotFound);
        };
        let owner: String = row.try_get(0).map_err(|_| FlightTrailError::Db)?;
        let status: String = row.try_get(1).map_err(|_| FlightTrailError::Db)?;
        let club_default: i16 = row.try_get(2).unwrap_or(DEFAULT_TRAIL_MINUTES);

        if owner != pilot_id {
            return Err(FlightTrailError::Forbidden);
        }
        if status != "active" {
            return Err(FlightTrailError::NotActive);
        }

        let raw_override = value.map(TrailMinutes::as_i16);
        sqlx::query("UPDATE flights SET trail_minutes = $1 WHERE id = $2")
            .bind(raw_override)
            .bind(flight_id)
            .execute(&self.pool)
            .await
            .map_err(|_| FlightTrailError::Db)?;

        let effective = TrailMinutes::effective(raw_override, club_default);
        Ok((value, effective))
    }

    /// Queries bounded recent fixes for the exact server-scoped `positions` list
    /// (own-club flights capped at `MAX_TRAIL_FLIGHTS`, nearby non-club flights capped at `MAX_NEARBY_PUBLIC_FLIGHTS`).
    pub async fn trails_for_positions(&self, positions: &[TailLivePosition]) -> Vec<FlightTrail> {
        if positions.is_empty() {
            return Vec::new();
        }
        let mut selected: Vec<&TailLivePosition> = Vec::with_capacity(positions.len());
        let mut club_count = 0usize;
        let mut non_club_count = 0usize;
        for pos in positions {
            if pos.non_club {
                if non_club_count < MAX_NEARBY_PUBLIC_FLIGHTS {
                    non_club_count += 1;
                    selected.push(pos);
                }
            } else if club_count < MAX_TRAIL_FLIGHTS {
                club_count += 1;
                selected.push(pos);
            }
        }

        let mut query_flight_ids: Vec<String> = Vec::new();
        let mut query_cutoffs: Vec<i64> = Vec::new();

        for pos in &selected {
            let effective = TrailMinutes::from_i16(pos.trail_minutes).unwrap_or(TrailMinutes::Min5);
            if let Some(window_ms) = effective.window_ms() {
                query_flight_ids.push(pos.flight_id.clone());
                query_cutoffs.push(pos.fix.t.saturating_sub(window_ms));
            }
        }

        let mut by_flight: HashMap<String, Vec<(i64, f64, f64)>> = HashMap::new();
        if !query_flight_ids.is_empty() {
            let rows = sqlx::query(
                "SELECT fx.flight_id, fx.t_ms, fx.lat, fx.lon
                 FROM UNNEST($1::text[], $2::bigint[]) AS q(flight_id, cutoff_t)
                 JOIN fixes fx ON fx.flight_id = q.flight_id AND fx.t_ms >= q.cutoff_t
                 ORDER BY fx.flight_id ASC, fx.t_ms ASC",
            )
            .bind(&query_flight_ids)
            .bind(&query_cutoffs)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default();

            for row in rows {
                let (Ok(fid), Ok(t_ms), Ok(lat), Ok(lon)) = (
                    row.try_get::<String, _>(0),
                    row.try_get::<i64, _>(1),
                    row.try_get::<f64, _>(2),
                    row.try_get::<f64, _>(3),
                ) else {
                    continue;
                };
                by_flight.entry(fid).or_default().push((t_ms, lat, lon));
            }
        }

        selected
            .into_iter()
            .map(|pos| {
                let effective =
                    TrailMinutes::from_i16(pos.trail_minutes).unwrap_or(TrailMinutes::Min5);
                let points = if effective == TrailMinutes::Off {
                    Vec::new()
                } else {
                    let raw = by_flight
                        .get(&pos.flight_id)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]);
                    window_and_downsample_trail(raw, effective, MAX_TRAIL_POINTS)
                };
                FlightTrail {
                    flight_id: pos.flight_id.clone(),
                    tail: pos.tail.clone(),
                    tail_source: pos.tail_source,
                    trail_minutes: effective.as_i16(),
                    non_club: pos.non_club,
                    points,
                }
            })
            .collect()
    }
}
