//! Pilot & anonymous public live-trails endpoints:
//! - `GET /api/me/live-trails` (authenticated pilot endpoint: `club` and/or geo-bounded `public` layers)
//! - `GET /api/public/live-trails` (anonymous geo-bounded `public`-only endpoint for signed-out clients)
//!
//! Security & scoping contract (`GET /api/me/live-trails`):
//! - Accepts the same pilot credentials used by the Android app (`Authorization: Bearer <device_token>`
//!   and/or `fv_club_session` / `fv_account_session` cookies). Auth runs BEFORE query parsing so
//!   unauthenticated callers always receive `401` (even with missing/invalid `layers` or coords).
//! - Never derives any club filter from `default_club_id`, and never relies on `can_view_flight`.
//! - Club layer returns live (`status = 'active'`, `ended_at IS NULL`, `last_fix_at` within
//!   `LIVE_FIX_MAX_AGE_MS`) `club` or `public` flights strictly in clubs where the caller has an
//!   ACTIVE membership (`club_memberships.status IN ('active', 'approved')`) and the club is
//!   operational (`status = 'live'`, `email_verified_at IS NOT NULL`, `suspended_at IS NULL`).
//!   Callers with no active membership receive zero club rows.
//! - Public layer is queried ONLY when `layers` explicitly includes `public` AND is NEVER a global
//!   feed: requests including `public` MUST supply finite `lat` (`[-90, 90]`) and `lon`
//!   (`[-180, 180]`), may supply optional finite `radius_km` (default 20 km, clamped to
//!   `5..=500` km), and may supply optional finite `min_agl_ft` (clamped `0..=20000`, absent = no
//!   altitude floor; applied to the public layer only). Public candidates are prefiltered in SQL by
//!   a spherical-cap bounding box on each flight's latest fix (skipping the longitude prefilter
//!   when the box crosses the antimeridian or covers a pole) with a hard SQL `LIMIT 500`
//!   (`PUBLIC_LIVE_TRAILS_SQL_CANDIDATE_LIMIT`), then filtered in Rust by exact great-circle
//!   distance `haversine_km(lat, lon, latest_lat, latest_lon) <= radius_km` and optional AGL floor
//!   (`evaluate_public_altitude` with `Pass | NoDemFallbackMsl`) before applying the 50-flight cap.
//!   Flights with `club_id IS NULL` are excluded for now (club-less support is a separate follow-up).
//! - Flights qualifying for both layers are returned once with `layer = "club"`.
//! - Caller's own flights (matching any `pilot_id` of the caller's account, or `owner_account_id`)
//!   are always excluded. `private` flights are never returned.
//! - Off (`trailMinutes = 0`) flights are still returned with their latest point only so the mobile
//!   app can render a marker without a trail polyline.
//! - Weak ETag (`W/"..."`) allows `304 Not Modified` short-circuiting before querying historical `fixes`.
//!
//! Security & scoping contract (`GET /api/public/live-trails`):
//! - Anonymous: ignores `Cookie` and `Authorization` headers completely; `sliding_session_renewal`
//!   skips `/api/public/*` so `Set-Cookie` is never emitted on cached public responses.
//! - Per-IP rate-limited via `AppState::public_live_trails_throttle` (`LoginThrottle`) and
//!   `ClientIp` (`PUBLIC_LIVE_TRAILS_MAX_REQUESTS = 30` req/min -> `429 {"error":"rate_limited"}`
//!   with `Retry-After: <seconds>`).
//! - Returns ONLY live `visibility = 'public'` flights from operational (`live`, verified,
//!   non-suspended) clubs (`flights.club_id IS NOT NULL`), geo-bounded by `PublicArea` with the
//!   same SQL `LIMIT 500`, exact haversine, optional `min_agl_ft` AGL floor, and 50-flight /
//!   300-point / 120-min caps.
//! - Includes `"pilotName"` ONLY when `flights.show_pilot_name_public = TRUE`; never exposes
//!   pilot/account/device IDs.
//! - Weak ETag + `If-None-Match` -> `304`, `Cache-Control: public, max-age=5`, no CORS headers.

use crate::club::{
    auth::{self, parse_session_cookie},
    db::{now_ms, ClubDb},
    geo::{haversine_km, safe_bbox_for_radius_km},
    map_filter::{evaluate_public_altitude, AltitudeEval, AltitudeMode},
    routes::{bearer_token, club_db},
    trail::{
        window_and_downsample_trail_with_alt, TrailMinutes, TrailPointWithAlt,
        DEFAULT_TRAIL_MINUTES, LIVE_FIX_MAX_AGE_MS, MAX_TRAIL_FLIGHTS, MAX_TRAIL_POINTS,
        MAX_TRAIL_WINDOW_MIN,
    },
};
use crate::{AppState, ClientIp};
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Json},
};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::{HashMap, HashSet};

/// Maximum flights returned by `GET /api/me/live-trails` and `GET /api/public/live-trails`
/// (most recent `last_fix_at` first).
pub const PILOT_LIVE_TRAILS_MAX_FLIGHTS: usize = MAX_TRAIL_FLIGHTS;
/// Hard SQL candidate row limit before Rust exact-haversine / altitude filtering and the 50-flight cap.
pub const PUBLIC_LIVE_TRAILS_SQL_CANDIDATE_LIMIT: usize = 500;
/// Default search radius (km) for the `public` layer when `radius_km` is omitted.
pub const PILOT_LIVE_TRAILS_DEFAULT_PUBLIC_RADIUS_KM: f64 = 20.0;
/// Minimum server-clamped search radius (km) for the `public` layer.
pub const PILOT_LIVE_TRAILS_MIN_PUBLIC_RADIUS_KM: f64 = 5.0;
/// Maximum server-clamped search radius (km) for the `public` layer.
pub const PILOT_LIVE_TRAILS_MAX_PUBLIC_RADIUS_KM: f64 = 500.0;
/// Minimum server-clamped altitude floor (ft AGL) for `min_agl_ft` when supplied.
pub const PUBLIC_LIVE_TRAILS_MIN_AGL_FT_MIN: f64 = 0.0;
/// Maximum server-clamped altitude floor (ft AGL) for `min_agl_ft` when supplied.
pub const PUBLIC_LIVE_TRAILS_MIN_AGL_FT_MAX: f64 = 20_000.0;
/// Cache-Control header for anonymous `GET /api/public/live-trails` responses (200 and 304).
pub const PUBLIC_LIVE_TRAILS_CACHE_CONTROL: &str = "public, max-age=5";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveTrailsLayers {
    pub club: bool,
    pub public: bool,
}

impl LiveTrailsLayers {
    pub const CLUB_ONLY: Self = Self {
        club: true,
        public: false,
    };

    /// Parses `?layers=...` from the raw query string.
    /// Absent -> `club` only. Empty, duplicate, repeated param, or unknown value -> `Err`.
    pub fn parse_query(raw_query: Option<&str>) -> Result<Self, &'static str> {
        let Some(query) = raw_query else {
            return Ok(Self::CLUB_ONLY);
        };
        let mut layers_value: Option<String> = None;
        for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
            if k == "layers" {
                if layers_value.is_some() {
                    return Err("invalid layers");
                }
                layers_value = Some(v.into_owned());
            }
        }
        let Some(raw) = layers_value else {
            return Ok(Self::CLUB_ONLY);
        };
        if raw.is_empty() {
            return Err("invalid layers");
        }
        let mut club = false;
        let mut public = false;
        for part in raw.split(',') {
            match part {
                "club" => {
                    if club {
                        return Err("invalid layers");
                    }
                    club = true;
                }
                "public" => {
                    if public {
                        return Err("invalid layers");
                    }
                    public = true;
                }
                _ => return Err("invalid layers"),
            }
        }
        if !club && !public {
            return Err("invalid layers");
        }
        Ok(Self { club, public })
    }

    pub const fn canonical_str(self) -> &'static str {
        match (self.club, self.public) {
            (true, false) => "club",
            (false, true) => "public",
            (true, true) => "club,public",
            (false, false) => "",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicArea {
    pub lat: f64,
    pub lon: f64,
    pub radius_km: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_agl_ft: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveTrailsQuery {
    pub layers: LiveTrailsLayers,
    pub public_area: Option<PublicArea>,
}

fn normalize_zero(v: f64) -> f64 {
    if v == 0.0 {
        0.0
    } else {
        v
    }
}

impl PublicArea {
    /// Parses required finite `lat` (`[-90, 90]`) & `lon` (`[-180, 180]`), optional
    /// `radius_km` (default `20.0`, clamped `5.0..=500.0`), and optional `min_agl_ft`
    /// (clamped `0.0..=20000.0`; absent = `None`) from the raw query string.
    pub fn parse_query(raw_query: Option<&str>) -> Result<Self, &'static str> {
        let Some(query) = raw_query else {
            return Err("invalid location");
        };

        let mut lat_raw: Option<String> = None;
        let mut lon_raw: Option<String> = None;
        let mut radius_raw: Option<String> = None;
        let mut min_agl_raw: Option<String> = None;

        for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
            match k.as_ref() {
                "lat" => {
                    if lat_raw.is_some() {
                        return Err("invalid location");
                    }
                    lat_raw = Some(v.into_owned());
                }
                "lon" => {
                    if lon_raw.is_some() {
                        return Err("invalid location");
                    }
                    lon_raw = Some(v.into_owned());
                }
                "radius_km" => {
                    if radius_raw.is_some() {
                        return Err("invalid radius_km");
                    }
                    radius_raw = Some(v.into_owned());
                }
                "min_agl_ft" => {
                    if min_agl_raw.is_some() {
                        return Err("invalid min_agl_ft");
                    }
                    min_agl_raw = Some(v.into_owned());
                }
                _ => {}
            }
        }

        let (Some(lat_s), Some(lon_s)) = (lat_raw, lon_raw) else {
            return Err("invalid location");
        };
        let Ok(lat) = lat_s.parse::<f64>() else {
            return Err("invalid location");
        };
        let Ok(lon) = lon_s.parse::<f64>() else {
            return Err("invalid location");
        };
        if !lat.is_finite()
            || !lon.is_finite()
            || !(-90.0..=90.0).contains(&lat)
            || !(-180.0..=180.0).contains(&lon)
        {
            return Err("invalid location");
        }

        let radius_km = match radius_raw {
            None => PILOT_LIVE_TRAILS_DEFAULT_PUBLIC_RADIUS_KM,
            Some(ref r_s) => {
                let Ok(r) = r_s.parse::<f64>() else {
                    return Err("invalid radius_km");
                };
                if !r.is_finite() {
                    return Err("invalid radius_km");
                }
                r.clamp(
                    PILOT_LIVE_TRAILS_MIN_PUBLIC_RADIUS_KM,
                    PILOT_LIVE_TRAILS_MAX_PUBLIC_RADIUS_KM,
                )
            }
        };

        let min_agl_ft = match min_agl_raw {
            None => None,
            Some(ref a_s) => {
                let Ok(a) = a_s.parse::<f64>() else {
                    return Err("invalid min_agl_ft");
                };
                if !a.is_finite() {
                    return Err("invalid min_agl_ft");
                }
                Some(normalize_zero(a.clamp(
                    PUBLIC_LIVE_TRAILS_MIN_AGL_FT_MIN,
                    PUBLIC_LIVE_TRAILS_MIN_AGL_FT_MAX,
                )))
            }
        };

        Ok(Self {
            lat: normalize_zero(lat),
            lon: normalize_zero(lon),
            radius_km,
            min_agl_ft,
        })
    }
}

impl LiveTrailsQuery {
    /// Parses `layers`, plus required `lat`/`lon` and optional `radius_km`/`min_agl_ft` whenever
    /// `layers` includes `public`. When only `club` is requested, public-area params are ignored.
    pub fn parse_query(raw_query: Option<&str>) -> Result<Self, &'static str> {
        let layers = LiveTrailsLayers::parse_query(raw_query)?;
        if !layers.public {
            return Ok(Self {
                layers,
                public_area: None,
            });
        }
        let public_area = PublicArea::parse_query(raw_query)?;
        Ok(Self {
            layers,
            public_area: Some(public_area),
        })
    }
}

#[derive(Debug, Clone)]
pub struct LiveTrailsCaller {
    pub account_id: Option<String>,
    pub own_pilot_ids: Vec<String>,
    pub active_club_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallerAuthError {
    Unauthorized,
    Forbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrailLayer {
    Club,
    Public,
}

impl TrailLayer {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Club => "club",
            Self::Public => "public",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CandidateLiveFlight {
    pub(crate) flight_id: String,
    #[allow(dead_code)]
    pub(crate) tail: String,
    #[allow(dead_code)]
    pub(crate) club_id: Option<String>,
    layer: TrailLayer,
    effective_trail: TrailMinutes,
    last_fix_at: i64,
    latest_t: i64,
    latest_lat: f64,
    latest_lon: f64,
    latest_alt: Option<f64>,
    latest_alt_ref: Option<String>,
    pilot_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PilotLiveTrailFlight {
    flight_id: String,
    tail: String,
    layer: &'static str,
    trail_minutes: i16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pilot_name: Option<String>,
    points: Vec<TrailPointWithAlt>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PilotLiveTrailsCaps {
    max_flights: usize,
    max_points_per_flight: usize,
    max_window_minutes: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PilotLiveTrailsResponse {
    flights: Vec<PilotLiveTrailFlight>,
    caps: PilotLiveTrailsCaps,
    #[serde(skip_serializing_if = "Option::is_none")]
    public_area: Option<PublicArea>,
}

fn compute_weak_etag(
    domain: &[u8],
    layers: Option<LiveTrailsLayers>,
    public_area: Option<PublicArea>,
    candidates: &[CandidateLiveFlight],
    include_pilot_name: bool,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    if let Some(layers) = layers {
        hasher.update(layers.canonical_str().as_bytes());
        hasher.update(b"\n");
    }
    if let Some(area) = public_area {
        hasher.update(b"area|");
        hasher.update(area.lat.to_bits().to_le_bytes());
        hasher.update(b"|");
        hasher.update(area.lon.to_bits().to_le_bytes());
        hasher.update(b"|");
        hasher.update(area.radius_km.to_bits().to_le_bytes());
        if let Some(min_agl_ft) = area.min_agl_ft {
            hasher.update(b"|agl|");
            hasher.update(min_agl_ft.to_bits().to_le_bytes());
        }
        hasher.update(b"\n");
    }
    for c in candidates {
        hasher.update(c.flight_id.as_bytes());
        hasher.update(b"|");
        hasher.update(c.layer.as_str().as_bytes());
        hasher.update(b"|");
        hasher.update(c.latest_t.to_le_bytes());
        hasher.update(b"|");
        hasher.update(c.effective_trail.as_i16().to_le_bytes());
        if include_pilot_name {
            hasher.update(b"|");
            if let Some(name) = c.pilot_name.as_deref() {
                hasher.update(b"1:");
                hasher.update(name.as_bytes());
            } else {
                hasher.update(b"0");
            }
        }
        hasher.update(b"\n");
    }
    let digest = hasher.finalize();
    let short_hex = hex::encode(&digest[..16]);
    format!("W/\"{short_hex}\"")
}

/// RFC 9110 §8.8.3.2 weak comparison for `If-None-Match`.
fn if_none_match_matches(headers: &HeaderMap, current_etag: &str) -> bool {
    let current_opaque = current_etag.strip_prefix("W/").unwrap_or(current_etag);
    for raw_val in headers.get_all(header::IF_NONE_MATCH) {
        let Ok(val_str) = raw_val.to_str() else {
            continue;
        };
        for part in val_str.split(',') {
            let trimmed = part.trim();
            if trimmed == "*" {
                return true;
            }
            let opaque = trimmed.strip_prefix("W/").unwrap_or(trimmed);
            if opaque == current_opaque {
                return true;
            }
        }
    }
    false
}

fn sanitize_point(t: i64, lat: f64, lon: f64, alt: Option<f64>) -> Option<TrailPointWithAlt> {
    if lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
    {
        Some((t, lat, lon, alt.filter(|a| a.is_finite())))
    } else {
        None
    }
}

impl ClubDb {
    async fn pilot_account_and_suspended(&self, pilot_id: &str) -> Option<(Option<String>, bool)> {
        let row = sqlx::query(
            "SELECT p.account_id, a.suspended_at
             FROM pilots p
             LEFT JOIN accounts a ON a.id = p.account_id
             WHERE p.id = $1",
        )
        .bind(pilot_id)
        .fetch_optional(&self.pool)
        .await
        .ok()??;
        let account_id: Option<String> = row.try_get(0).ok().flatten();
        let suspended_at: Option<i64> = row.try_get(1).ok().flatten();
        Some((account_id, suspended_at.is_some()))
    }

    async fn resolve_cookie_identity(
        &self,
        headers: &HeaderMap,
        now: i64,
    ) -> Option<(Option<String>, Option<String>)> {
        if let Some(acct_sess) = auth::parse_account_session_cookie(headers) {
            if let Some((acct, _)) = self.account_from_session(&acct_sess, now).await {
                return Some((Some(acct.id), None));
            }
        }
        if let Some(club_sess) = parse_session_cookie(headers) {
            if let Some(pilot) = self.pilot_from_session(&club_sess, now).await {
                let (account_id, suspended) = self.pilot_account_and_suspended(&pilot.id).await?;
                if suspended {
                    return None;
                }
                return Some((account_id, Some(pilot.id)));
            }
        }
        None
    }

    async fn resolve_live_trails_caller(
        &self,
        headers: &HeaderMap,
        now: i64,
    ) -> Result<LiveTrailsCaller, CallerAuthError> {
        let (account_id, primary_pilot_id) = if headers.contains_key(header::AUTHORIZATION) {
            let Some(token) = bearer_token(headers) else {
                return Err(CallerAuthError::Unauthorized);
            };
            let Some(device_id) = self.resolve_device_token(&token).await else {
                return Err(CallerAuthError::Unauthorized);
            };
            let Some(dev_pilot_id) = self.device_pilot_id(&device_id).await else {
                return Err(CallerAuthError::Unauthorized);
            };
            let Some((acct_id, suspended)) = self.pilot_account_and_suspended(&dev_pilot_id).await
            else {
                return Err(CallerAuthError::Unauthorized);
            };
            if suspended {
                return Err(CallerAuthError::Unauthorized);
            }
            // If a session cookie is also supplied and resolves, ensure it belongs to the same pilot/account.
            if let Some((cookie_acct, cookie_pilot)) =
                self.resolve_cookie_identity(headers, now).await
            {
                let same_pilot = cookie_pilot.as_deref() == Some(dev_pilot_id.as_str());
                let same_acct = acct_id.is_some() && cookie_acct == acct_id;
                if !same_pilot && !same_acct {
                    return Err(CallerAuthError::Forbidden);
                }
            }
            (acct_id, Some(dev_pilot_id))
        } else {
            let Some((acct_id, pilot_id)) = self.resolve_cookie_identity(headers, now).await else {
                return Err(CallerAuthError::Unauthorized);
            };
            (acct_id, pilot_id)
        };

        let mut own_pilot_ids: Vec<String> = if let Some(ref aid) = account_id {
            sqlx::query_scalar("SELECT id FROM pilots WHERE account_id = $1")
                .bind(aid)
                .fetch_all(&self.pool)
                .await
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if let Some(pid) = primary_pilot_id {
            if !own_pilot_ids.iter().any(|p| p == &pid) {
                own_pilot_ids.push(pid);
            }
        }

        let active_club_ids: Vec<String> = if own_pilot_ids.is_empty() {
            Vec::new()
        } else {
            sqlx::query_scalar(
                "SELECT DISTINCT m.club_id
                 FROM club_memberships m
                 JOIN clubs c ON c.id = m.club_id
                 WHERE m.pilot_id = ANY($1::text[])
                   AND m.status IN ('active', 'approved')
                   AND c.status = 'live'
                   AND c.email_verified_at IS NOT NULL
                   AND c.suspended_at IS NULL",
            )
            .bind(&own_pilot_ids)
            .fetch_all(&self.pool)
            .await
            .unwrap_or_default()
        };

        Ok(LiveTrailsCaller {
            account_id,
            own_pilot_ids,
            active_club_ids,
        })
    }

    async fn query_club_layer_live_flights(
        &self,
        caller: &LiveTrailsCaller,
        live_cutoff_ms: i64,
    ) -> Vec<CandidateLiveFlight> {
        if caller.active_club_ids.is_empty() {
            return Vec::new();
        }
        let rows = sqlx::query(
            "SELECT f.id,
                    a.tail,
                    f.club_id,
                    f.trail_minutes,
                    c.trail_minutes_default,
                    f.last_fix_at,
                    fx.t_ms,
                    fx.lat,
                    fx.lon,
                    fx.alt
             FROM flights f
             JOIN clubs c ON c.id = f.club_id
             LEFT JOIN aircraft a ON a.id = f.aircraft_id
             JOIN LATERAL (
                 SELECT t_ms, lat, lon, alt
                 FROM fixes
                 WHERE flight_id = f.id
                 ORDER BY t_ms DESC
                 LIMIT 1
             ) fx ON TRUE
             WHERE f.status = 'active'
               AND f.ended_at IS NULL
               AND f.last_fix_at IS NOT NULL
               AND f.last_fix_at >= $1
               AND f.club_id IS NOT NULL
               AND f.club_id = ANY($2::text[])
               AND f.visibility IN ('club', 'public')
               AND c.status = 'live'
               AND c.email_verified_at IS NOT NULL
               AND c.suspended_at IS NULL
               AND NOT (f.pilot_id = ANY($3::text[]))
               AND ($4::text IS NULL OR f.owner_account_id IS DISTINCT FROM $4)
               AND a.tail IS NOT NULL
             ORDER BY f.last_fix_at DESC, f.id ASC
             LIMIT $5",
        )
        .bind(live_cutoff_ms)
        .bind(&caller.active_club_ids)
        .bind(&caller.own_pilot_ids)
        .bind(caller.account_id.as_deref())
        .bind(PILOT_LIVE_TRAILS_MAX_FLIGHTS as i64)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows_to_candidates(rows, TrailLayer::Club, false, false)
    }

    fn filter_public_candidates(
        &self,
        candidates: Vec<CandidateLiveFlight>,
        area: PublicArea,
    ) -> Vec<CandidateLiveFlight> {
        let dem = area.min_agl_ft.map(|_| self.dem_index());
        let mut filtered: Vec<CandidateLiveFlight> = candidates
            .into_iter()
            .filter(|c| {
                if !c.latest_lat.is_finite()
                    || !c.latest_lon.is_finite()
                    || haversine_km(area.lat, area.lon, c.latest_lat, c.latest_lon) > area.radius_km
                {
                    return false;
                }
                if let (Some(min_agl_ft), Some(dem)) = (area.min_agl_ft, dem.as_ref()) {
                    let fix = crate::live::LiveFix {
                        t: c.latest_t,
                        lat: c.latest_lat,
                        lon: c.latest_lon,
                        alt: c.latest_alt,
                        alt_ref: c.latest_alt_ref.clone(),
                        speed: None,
                        course: None,
                        accuracy: None,
                    };
                    if !matches!(
                        evaluate_public_altitude(&fix, AltitudeMode::Agl, min_agl_ft, dem),
                        AltitudeEval::Pass | AltitudeEval::NoDemFallbackMsl
                    ) {
                        return false;
                    }
                }
                true
            })
            .collect();
        filtered.truncate(PILOT_LIVE_TRAILS_MAX_FLIGHTS);
        filtered
    }

    async fn query_public_layer_live_flights(
        &self,
        caller: &LiveTrailsCaller,
        exclude_club_ids: &[String],
        live_cutoff_ms: i64,
        area: PublicArea,
    ) -> Vec<CandidateLiveFlight> {
        self.query_public_layer_live_flights_with_limit(
            caller,
            exclude_club_ids,
            live_cutoff_ms,
            area,
            PUBLIC_LIVE_TRAILS_SQL_CANDIDATE_LIMIT,
        )
        .await
    }

    pub(crate) async fn query_public_layer_live_flights_with_limit(
        &self,
        caller: &LiveTrailsCaller,
        exclude_club_ids: &[String],
        live_cutoff_ms: i64,
        area: PublicArea,
        sql_limit: usize,
    ) -> Vec<CandidateLiveFlight> {
        let (min_lat, max_lat, lon_range) =
            safe_bbox_for_radius_km(area.lat, area.lon, area.radius_km);
        let (min_lon, max_lon) = match lon_range {
            Some((lo, hi)) => (Some(lo), Some(hi)),
            None => (None, None),
        };

        let rows = sqlx::query(
            "SELECT f.id,
                    a.tail,
                    f.club_id,
                    f.trail_minutes,
                    c.trail_minutes_default,
                    f.last_fix_at,
                    fx.t_ms,
                    fx.lat,
                    fx.lon,
                    fx.alt,
                    fx.alt_ref
             FROM flights f
             LEFT JOIN clubs c ON c.id = f.club_id
             LEFT JOIN aircraft a ON a.id = f.aircraft_id
             JOIN LATERAL (
                 SELECT t_ms, lat, lon, alt, alt_ref
                 FROM fixes
                 WHERE flight_id = f.id
                 ORDER BY t_ms DESC
                 LIMIT 1
             ) fx ON TRUE
             WHERE f.status = 'active'
               AND f.ended_at IS NULL
               AND f.last_fix_at IS NOT NULL
               AND f.last_fix_at >= $1
               AND f.visibility = 'public'
               AND f.club_id IS NOT NULL
               AND NOT (f.club_id = ANY($2::text[]))
               AND c.status = 'live'
               AND c.email_verified_at IS NOT NULL
               AND c.suspended_at IS NULL
               AND NOT (f.pilot_id = ANY($3::text[]))
               AND ($4::text IS NULL OR f.owner_account_id IS DISTINCT FROM $4)
               AND a.tail IS NOT NULL
               AND fx.lat BETWEEN $5 AND $6
               AND ($7::double precision IS NULL OR fx.lon BETWEEN $7 AND $8)
             ORDER BY f.last_fix_at DESC, f.id ASC
             LIMIT $9",
        )
        .bind(live_cutoff_ms)
        .bind(exclude_club_ids)
        .bind(&caller.own_pilot_ids)
        .bind(caller.account_id.as_deref())
        .bind(min_lat)
        .bind(max_lat)
        .bind(min_lon)
        .bind(max_lon)
        .bind(sql_limit as i64)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        let candidates = rows_to_candidates(rows, TrailLayer::Public, true, false);
        self.filter_public_candidates(candidates, area)
    }

    async fn query_anonymous_public_live_flights(
        &self,
        live_cutoff_ms: i64,
        area: PublicArea,
    ) -> Vec<CandidateLiveFlight> {
        self.query_anonymous_public_live_flights_with_limit(
            live_cutoff_ms,
            area,
            PUBLIC_LIVE_TRAILS_SQL_CANDIDATE_LIMIT,
        )
        .await
    }

    pub(crate) async fn query_anonymous_public_live_flights_with_limit(
        &self,
        live_cutoff_ms: i64,
        area: PublicArea,
        sql_limit: usize,
    ) -> Vec<CandidateLiveFlight> {
        let (min_lat, max_lat, lon_range) =
            safe_bbox_for_radius_km(area.lat, area.lon, area.radius_km);
        let (min_lon, max_lon) = match lon_range {
            Some((lo, hi)) => (Some(lo), Some(hi)),
            None => (None, None),
        };

        let rows = sqlx::query(
            "SELECT f.id,
                    a.tail,
                    f.club_id,
                    f.trail_minutes,
                    c.trail_minutes_default,
                    f.last_fix_at,
                    fx.t_ms,
                    fx.lat,
                    fx.lon,
                    fx.alt,
                    fx.alt_ref,
                    CASE WHEN f.show_pilot_name_public THEN p.name ELSE NULL END AS pilot_name
             FROM flights f
             LEFT JOIN clubs c ON c.id = f.club_id
             LEFT JOIN aircraft a ON a.id = f.aircraft_id
             LEFT JOIN pilots p ON p.id = f.pilot_id
             JOIN LATERAL (
                 SELECT t_ms, lat, lon, alt, alt_ref
                 FROM fixes
                 WHERE flight_id = f.id
                 ORDER BY t_ms DESC
                 LIMIT 1
             ) fx ON TRUE
             WHERE f.status = 'active'
               AND f.ended_at IS NULL
               AND f.last_fix_at IS NOT NULL
               AND f.last_fix_at >= $1
               AND f.visibility = 'public'
               AND f.club_id IS NOT NULL
               AND c.status = 'live'
               AND c.email_verified_at IS NOT NULL
               AND c.suspended_at IS NULL
               AND a.tail IS NOT NULL
               AND fx.lat BETWEEN $2 AND $3
               AND ($4::double precision IS NULL OR fx.lon BETWEEN $4 AND $5)
             ORDER BY f.last_fix_at DESC, f.id ASC
             LIMIT $6",
        )
        .bind(live_cutoff_ms)
        .bind(min_lat)
        .bind(max_lat)
        .bind(min_lon)
        .bind(max_lon)
        .bind(sql_limit as i64)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        let candidates = rows_to_candidates(rows, TrailLayer::Public, true, true);
        self.filter_public_candidates(candidates, area)
    }

    async fn batch_fixes_with_alt(
        &self,
        candidates: &[CandidateLiveFlight],
    ) -> HashMap<String, Vec<TrailPointWithAlt>> {
        let mut query_flight_ids: Vec<String> = Vec::new();
        let mut query_cutoffs: Vec<i64> = Vec::new();

        for c in candidates {
            if let Some(window_ms) = c.effective_trail.window_ms() {
                query_flight_ids.push(c.flight_id.clone());
                query_cutoffs.push(c.latest_t.saturating_sub(window_ms));
            }
        }

        let mut by_flight: HashMap<String, Vec<TrailPointWithAlt>> = HashMap::new();
        if query_flight_ids.is_empty() {
            return by_flight;
        }

        let rows = sqlx::query(
            "SELECT fx.flight_id, fx.t_ms, fx.lat, fx.lon, fx.alt
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
            let alt: Option<f64> = row.try_get(4).ok().flatten();
            by_flight
                .entry(fid)
                .or_default()
                .push((t_ms, lat, lon, alt));
        }
        by_flight
    }
}

fn rows_to_candidates(
    rows: Vec<sqlx::postgres::PgRow>,
    layer: TrailLayer,
    has_alt_ref: bool,
    has_pilot_name: bool,
) -> Vec<CandidateLiveFlight> {
    rows.into_iter()
        .filter_map(|r| {
            let flight_id: String = r.try_get(0).ok()?;
            let tail: String = r.try_get(1).ok()?;
            let club_id: Option<String> = r.try_get(2).ok().flatten();
            let flight_override: Option<i16> = r.try_get(3).ok().flatten();
            let club_default: i16 = r
                .try_get::<Option<i16>, _>(4)
                .ok()
                .flatten()
                .unwrap_or(DEFAULT_TRAIL_MINUTES);
            let last_fix_at: i64 = r.try_get(5).ok()?;
            let latest_t: i64 = r.try_get(6).ok()?;
            let latest_lat: f64 = r.try_get(7).ok()?;
            let latest_lon: f64 = r.try_get(8).ok()?;
            let latest_alt: Option<f64> = r.try_get(9).ok().flatten();
            let latest_alt_ref: Option<String> = if has_alt_ref {
                r.try_get(10).ok().flatten()
            } else {
                None
            };
            let pilot_name: Option<String> = if has_pilot_name {
                r.try_get(11).ok().flatten()
            } else {
                None
            };
            let effective_trail = TrailMinutes::effective(flight_override, club_default);
            Some(CandidateLiveFlight {
                flight_id,
                tail,
                club_id,
                layer,
                effective_trail,
                last_fix_at,
                latest_t,
                latest_lat,
                latest_lon,
                latest_alt,
                latest_alt_ref,
                pilot_name,
            })
        })
        .collect()
}

async fn build_live_trail_flights(
    db: &ClubDb,
    candidates: Vec<CandidateLiveFlight>,
    include_pilot_name: bool,
) -> Vec<PilotLiveTrailFlight> {
    let by_flight = db.batch_fixes_with_alt(&candidates).await;
    candidates
        .into_iter()
        .map(|c| {
            let points = if c.effective_trail == TrailMinutes::Off {
                // Off flights return their latest point only so the app can still draw a marker.
                sanitize_point(c.latest_t, c.latest_lat, c.latest_lon, c.latest_alt)
                    .into_iter()
                    .collect()
            } else {
                let raw = by_flight
                    .get(&c.flight_id)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let mut windowed =
                    window_and_downsample_trail_with_alt(raw, c.effective_trail, MAX_TRAIL_POINTS);
                if windowed.is_empty() {
                    if let Some(pt) =
                        sanitize_point(c.latest_t, c.latest_lat, c.latest_lon, c.latest_alt)
                    {
                        windowed.push(pt);
                    }
                }
                windowed
            };
            PilotLiveTrailFlight {
                flight_id: c.flight_id,
                tail: c.tail,
                layer: c.layer.as_str(),
                trail_minutes: c.effective_trail.as_i16(),
                pilot_name: if include_pilot_name {
                    c.pilot_name
                } else {
                    None
                },
                points,
            }
        })
        .collect()
}

pub(crate) async fn get_me_live_trails(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let now = now_ms();
    // Auth runs BEFORE query parsing (layers / lat / lon / radius_km / min_agl_ft) so
    // signed-out callers always receive 401 rather than 400.
    let caller = match db.resolve_live_trails_caller(&headers, now).await {
        Ok(c) => c,
        Err(CallerAuthError::Unauthorized) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "unauthorized" })),
            )
                .into_response();
        }
        Err(CallerAuthError::Forbidden) => {
            return (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))).into_response();
        }
    };
    let query = match LiveTrailsQuery::parse_query(uri.query()) {
        Ok(q) => q,
        Err(msg) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response();
        }
    };
    let layers = query.layers;
    let public_area = query.public_area;

    let live_cutoff_ms = now.saturating_sub(LIVE_FIX_MAX_AGE_MS);
    let mut candidates: Vec<CandidateLiveFlight> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new();

    if layers.club {
        for c in db
            .query_club_layer_live_flights(&caller, live_cutoff_ms)
            .await
        {
            if seen_ids.insert(c.flight_id.clone()) {
                candidates.push(c);
            }
        }
    }

    // Public flights are queried ONLY when `public` is explicitly requested,
    // strictly bounded to `public_area` (never a global feed).
    if let Some(area) = public_area {
        let exclude_clubs: &[String] = if layers.club {
            &caller.active_club_ids
        } else {
            &[]
        };
        for c in db
            .query_public_layer_live_flights(&caller, exclude_clubs, live_cutoff_ms, area)
            .await
        {
            if seen_ids.insert(c.flight_id.clone()) {
                candidates.push(c);
            }
        }
    }

    candidates.sort_by(|a, b| {
        b.last_fix_at
            .cmp(&a.last_fix_at)
            .then_with(|| a.flight_id.cmp(&b.flight_id))
    });
    candidates.truncate(PILOT_LIVE_TRAILS_MAX_FLIGHTS);

    let etag = compute_weak_etag(
        b"fv-me-live-trails-v1\n",
        Some(layers),
        public_area,
        &candidates,
        false,
    );
    if if_none_match_matches(&headers, &etag) {
        return (
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (header::CACHE_CONTROL, "private, no-cache".to_string()),
            ],
        )
            .into_response();
    }

    let flights = build_live_trail_flights(db, candidates, false).await;
    let payload = PilotLiveTrailsResponse {
        flights,
        caps: PilotLiveTrailsCaps {
            max_flights: PILOT_LIVE_TRAILS_MAX_FLIGHTS,
            max_points_per_flight: MAX_TRAIL_POINTS,
            max_window_minutes: MAX_TRAIL_WINDOW_MIN,
        },
        public_area,
    };

    (
        StatusCode::OK,
        [
            (header::ETAG, etag),
            (header::CACHE_CONTROL, "private, no-cache".to_string()),
        ],
        Json(payload),
    )
        .into_response()
}

pub(crate) async fn get_public_live_trails(
    State(state): State<AppState>,
    client_ip: ClientIp,
    headers: HeaderMap,
    uri: Uri,
) -> impl IntoResponse {
    if let Err(retry_after) = state.public_live_trails_throttle.check_up_to(
        &client_ip.0,
        crate::now_secs(),
        crate::PUBLIC_LIVE_TRAILS_MAX_REQUESTS,
    ) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_after.to_string())],
            Json(json!({ "error": "rate_limited" })),
        )
            .into_response();
    }
    let area = match PublicArea::parse_query(uri.query()) {
        Ok(area) => area,
        Err(msg) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response();
        }
    };
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let now = now_ms();
    let live_cutoff_ms = now.saturating_sub(LIVE_FIX_MAX_AGE_MS);
    let candidates = db
        .query_anonymous_public_live_flights(live_cutoff_ms, area)
        .await;

    let etag = compute_weak_etag(
        b"fv-public-live-trails-v1\n",
        None,
        Some(area),
        &candidates,
        true,
    );
    if if_none_match_matches(&headers, &etag) {
        return (
            StatusCode::NOT_MODIFIED,
            [
                (header::ETAG, etag),
                (
                    header::CACHE_CONTROL,
                    PUBLIC_LIVE_TRAILS_CACHE_CONTROL.to_string(),
                ),
            ],
        )
            .into_response();
    }

    let flights = build_live_trail_flights(db, candidates, true).await;
    let payload = PilotLiveTrailsResponse {
        flights,
        caps: PilotLiveTrailsCaps {
            max_flights: PILOT_LIVE_TRAILS_MAX_FLIGHTS,
            max_points_per_flight: MAX_TRAIL_POINTS,
            max_window_minutes: MAX_TRAIL_WINDOW_MIN,
        },
        public_area: Some(area),
    };

    (
        StatusCode::OK,
        [
            (header::ETAG, etag),
            (
                header::CACHE_CONTROL,
                PUBLIC_LIVE_TRAILS_CACHE_CONTROL.to_string(),
            ),
        ],
        Json(payload),
    )
        .into_response()
}
