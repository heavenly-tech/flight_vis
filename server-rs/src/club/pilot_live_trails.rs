//! Authenticated pilot live-trails endpoint (`GET /api/me/live-trails?layers=club,public`).
//!
//! Security & scoping contract:
//! - Accepts the same pilot credentials used by the Android app (`Authorization: Bearer <device_token>`
//!   and/or `fv_club_session` / `fv_account_session` cookies). Unauthenticated -> 401.
//! - Never derives any club filter from `default_club_id`, and never relies on `can_view_flight`.
//! - Club layer returns live (`status = 'active'`, `ended_at IS NULL`, `last_fix_at` within
//!   `LIVE_FIX_MAX_AGE_MS`) `club` or `public` flights strictly in clubs where the caller has an
//!   ACTIVE membership (`club_memberships.status IN ('active', 'approved')`) and the club is
//!   operational (`status = 'live'`, `email_verified_at IS NOT NULL`, `suspended_at IS NULL`).
//!   Callers with no active membership receive zero club rows.
//! - Public layer is queried ONLY when `layers` explicitly includes `public`. Returns live `public`
//!   flights from operational non-suspended clubs. Flights with `club_id IS NULL` are excluded for
//!   now (club-less support is a separate follow-up).
//! - Flights qualifying for both layers are returned once with `layer = "club"`.
//! - Caller's own flights (matching any `pilot_id` of the caller's account, or `owner_account_id`)
//!   are always excluded. `private` flights are never returned.
//! - Off (`trailMinutes = 0`) flights are still returned with their latest point only so the mobile
//!   app can render a marker without a trail polyline.
//! - Weak ETag (`W/"..."`) over `(layers, [(flight_id, layer, latest_fix_t, effective_trail_minutes)])`
//!   allows `304 Not Modified` short-circuiting before querying the historical `fixes` batch.

use crate::club::{
    auth::{self, parse_session_cookie},
    db::{now_ms, ClubDb},
    routes::{bearer_token, club_db},
    trail::{
        window_and_downsample_trail_with_alt, TrailMinutes, TrailPointWithAlt,
        DEFAULT_TRAIL_MINUTES, LIVE_FIX_MAX_AGE_MS, MAX_TRAIL_FLIGHTS, MAX_TRAIL_POINTS,
        MAX_TRAIL_WINDOW_MIN,
    },
};
use crate::AppState;
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

/// Maximum flights returned by `GET /api/me/live-trails` (most recent `last_fix_at` first).
pub const PILOT_LIVE_TRAILS_MAX_FLIGHTS: usize = MAX_TRAIL_FLIGHTS;

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
struct CandidateLiveFlight {
    flight_id: String,
    tail: String,
    #[allow(dead_code)]
    club_id: Option<String>,
    layer: TrailLayer,
    effective_trail: TrailMinutes,
    last_fix_at: i64,
    latest_t: i64,
    latest_lat: f64,
    latest_lon: f64,
    latest_alt: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PilotLiveTrailFlight {
    flight_id: String,
    tail: String,
    layer: &'static str,
    trail_minutes: i16,
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
}

fn compute_weak_etag(layers: LiveTrailsLayers, candidates: &[CandidateLiveFlight]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"fv-me-live-trails-v1\n");
    hasher.update(layers.canonical_str().as_bytes());
    hasher.update(b"\n");
    for c in candidates {
        hasher.update(c.flight_id.as_bytes());
        hasher.update(b"|");
        hasher.update(c.layer.as_str().as_bytes());
        hasher.update(b"|");
        hasher.update(c.latest_t.to_le_bytes());
        hasher.update(b"|");
        hasher.update(c.effective_trail.as_i16().to_le_bytes());
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

        rows_to_candidates(rows, TrailLayer::Club)
    }

    async fn query_public_layer_live_flights(
        &self,
        caller: &LiveTrailsCaller,
        exclude_club_ids: &[String],
        live_cutoff_ms: i64,
    ) -> Vec<CandidateLiveFlight> {
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
             LEFT JOIN clubs c ON c.id = f.club_id
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
               AND f.visibility = 'public'
               AND f.club_id IS NOT NULL
               AND NOT (f.club_id = ANY($2::text[]))
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
        .bind(exclude_club_ids)
        .bind(&caller.own_pilot_ids)
        .bind(caller.account_id.as_deref())
        .bind(PILOT_LIVE_TRAILS_MAX_FLIGHTS as i64)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();

        rows_to_candidates(rows, TrailLayer::Public)
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
            })
        })
        .collect()
}

pub(crate) async fn get_me_live_trails(
    State(state): State<AppState>,
    headers: HeaderMap,
    uri: Uri,
) -> impl IntoResponse {
    let layers = match LiveTrailsLayers::parse_query(uri.query()) {
        Ok(l) => l,
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

    // Public flights are queried ONLY when `public` is explicitly requested.
    if layers.public {
        let exclude_clubs: &[String] = if layers.club {
            &caller.active_club_ids
        } else {
            &[]
        };
        for c in db
            .query_public_layer_live_flights(&caller, exclude_clubs, live_cutoff_ms)
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

    let etag = compute_weak_etag(layers, &candidates);
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

    let by_flight = db.batch_fixes_with_alt(&candidates).await;
    let flights: Vec<PilotLiveTrailFlight> = candidates
        .into_iter()
        .map(|c| {
            let points = if c.effective_trail == TrailMinutes::Off {
                // Off flights return their latest point only so the pilot app can still draw a marker.
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
                points,
            }
        })
        .collect();

    let payload = PilotLiveTrailsResponse {
        flights,
        caps: PilotLiveTrailsCaps {
            max_flights: PILOT_LIVE_TRAILS_MAX_FLIGHTS,
            max_points_per_flight: MAX_TRAIL_POINTS,
            max_window_minutes: MAX_TRAIL_WINDOW_MIN,
        },
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
