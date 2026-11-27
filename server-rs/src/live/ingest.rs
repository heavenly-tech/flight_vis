use crate::{
    club::AppendOpenFlightError,
    live::{
        engine::LiveEngine,
        types::{
            knots_to_mps, parse_ingest_payload, valid_lat_lon, IngestBatch, LiveFix,
            ALT_REF_WGS84_ELLIPSOID,
        },
    },
    AppState,
};
use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

/// Maximum number of fixes accepted in a single `POST /api/ingest` batch.
/// Larger batches are rejected atomically with HTTP 413 (`{"error":"batch too large","max":100}`)
/// before consuming rate-limit budget or writing any fixes.
pub const MAX_INGEST_BATCH_FIXES: usize = 100;

/// Clock-skew allowance before an open flight's `started_at` (2 minutes = `120_000` ms).
/// Fixes with device timestamp strictly earlier than `flight.started_at - PRE_START_SKEW_MS`
/// are dropped as pre-start leftovers from earlier sessions (counted in `droppedPreStart`,
/// not written to the DB, and not charged against the per-device 120/60s ingest cap).
pub const PRE_START_SKEW_MS: i64 = 120_000;

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn unauthorized() -> impl IntoResponse {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "unauthorized" })),
    )
}

fn bad_request(msg: &'static str) -> impl IntoResponse {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg })))
}

fn flight_mismatch() -> impl IntoResponse {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "flight_mismatch",
            "reason": "flight_mismatch",
        })),
    )
}

fn throttled_flight_mismatch(engine: &LiveEngine, device_key: &str) -> axum::response::Response {
    if let Err(retry_after) = engine.allow_flight_mismatch(device_key) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_after.to_string())],
            Json(json!({ "error": "rate limited" })),
        )
            .into_response();
    }
    flight_mismatch().into_response()
}

/// Effective minimum device timestamp (ms) accepted on an open flight.
///
/// - Explicit `flight_id`/`flightId` binding (`omit_flight_id == false`): keeps the standard
///   `started_at - PRE_START_SKEW_MS` (120 s) skew tolerance unchanged.
/// - Omit-id path (`omit_flight_id == true`, including `/api/osmand` without `flight_id`):
///   fails closed against leftovers from a recently ended flight of the same device/pilot/account
///   (M-32). Any fix `<= previous_flight_ended_at` is dropped; when `previous_flight_ended_at`
///   falls inside the 120 s pre-start skew window (`>= started_at - PRE_START_SKEW_MS`), omit-id
///   fixes before `started_at` are also dropped (`max(started_at, previous_flight_ended_at + 1)`)
///   so inter-flight / prior-flight points cannot attach to the new flight.
pub(crate) fn effective_pre_start_min_t(
    started_at: i64,
    previous_flight_ended_at: Option<i64>,
    omit_flight_id: bool,
) -> i64 {
    let base = started_at.saturating_sub(PRE_START_SKEW_MS);
    if !omit_flight_id {
        return base;
    }
    match previous_flight_ended_at {
        Some(prev_end) if prev_end >= base => started_at.max(prev_end.saturating_add(1)),
        Some(prev_end) => base.max(prev_end.saturating_add(1)),
        None => base,
    }
}

fn validate_ingest_fix(engine: &LiveEngine, fix: &LiveFix) -> Result<(), &'static str> {
    engine.validate_fix(fix)?;
    for v in [fix.alt, fix.speed, fix.course, fix.accuracy]
        .into_iter()
        .flatten()
    {
        if !v.is_finite() {
            return Err("invalid fix field");
        }
    }
    Ok(())
}

fn validate_batch_seq(batch: &IngestBatch) -> Result<(), &'static str> {
    if let Some(seq) = batch.seq {
        if seq < 0 {
            return Err("invalid seq");
        }
    }
    for fix in &batch.fixes {
        if let Some(seq) = fix.seq {
            if seq < 0 {
                return Err("invalid seq");
            }
        }
    }
    Ok(())
}

fn collect_requested_flight_ids(batch: &IngestBatch) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(id) = &batch.flight_id {
        ids.push(id.clone());
    }
    for fix in &batch.fixes {
        if let Some(id) = &fix.flight_id {
            ids.push(id.clone());
        }
    }
    ids
}

fn flight_binding_matches(requested_ids: &[String], open_flight_id: &str) -> bool {
    requested_ids.iter().all(|id| id == open_flight_id)
}

/// Batch and single-fix position ingest (`POST /api/ingest`).
///
/// - Enforces Bearer device authentication before inspecting payload shape, batch limits, or
///   flight bindings.
/// - Accepts both batch (`{"fixes":[...], "flight_id"|"flightId"?: ..., "seq"?: ...}`) and
///   single-fix (`{"t":..., "lat":..., "lon":..., "flight_id"|"flightId"?: ..., "seq"?: ...}`)
///   JSON payloads. `flight_id`/`flightId` and `seq` (non-negative integer, validated and
///   ignored since dedup/ordering uses `(flight_id, device_id, t_ms)`) are accepted at both
///   envelope and per-fix level.
/// - Accepts batches of up to [`MAX_INGEST_BATCH_FIXES`] (100) fixes; larger batches return
///   `413 Payload Too Large` with `{"error":"batch too large","max":100}` atomically without
///   consuming rate-limit budget or writing any rows.
/// - Validates all `seq` values and fixes atomically (finite lat/lon, finite optional telemetry,
///   device timestamp within `[MIN_FIX_MS, now + MAX_FUTURE_MS]`) before rate-limit consumption
///   or writes.
/// - Flight binding (H-8): when any `flight_id`/`flightId` is present and does not equal the
///   device's currently open flight owned by the device's pilot/account (including when no flight
///   is open, or for closed/foreign/unknown IDs), rejects the whole batch with
///   `409 {"error":"flight_mismatch","reason":"flight_mismatch"}` without writing fixes,
///   consuming rate-limit hits, or broadcasting live events.
/// - Pre-start skew drop (H-8): drops fixes with device timestamp `< flight.started_at - PRE_START_SKEW_MS`
///   (`120_000` ms = 2 min). Dropped fixes do not consume the per-device 120/60s cap and are
///   reported in `"droppedPreStart"`; an all-dropped batch returns `200` with `accepted: 0`.
/// - Per-device sliding cap stays [`crate::live::engine::MAX_CLUB_INGEST_FIXES_PER_MINUTE`]
///   (120 fixes / 60s) with `429` + `Retry-After` and atomic rejection (no partial writes),
///   counted on non-dropped fixes.
/// - Deduplicates by `(flight_id, device_id, t_ms)` (`ON CONFLICT DO NOTHING`) and returns
///   `200 {"ok":true,"accepted":n,"duplicates":m,"droppedPreStart":d}` while keeping current
///   position monotonic.
/// - `/api/osmand` stays a single-fix Traccar/OsmAnd compatibility endpoint, applying the same
///   `409 flight_mismatch` rule when optional `flight_id`/`flightId` is passed and the same
///   [`PRE_START_SKEW_MS`] drop rule on DB-backed flights.
pub async fn post_ingest(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let token = match bearer_token(&headers) {
        Some(token) => token,
        None => return unauthorized().into_response(),
    };

    enum IngestCaller<'a> {
        DbDevice(String),
        LegacyDevice(&'a str),
    }

    let caller = if let Some(db) = &state.club {
        if let Some(device_id) = db.resolve_device_token(token).await {
            if db.device_account_suspended(&device_id).await {
                return unauthorized().into_response();
            }
            Some(IngestCaller::DbDevice(device_id))
        } else {
            None
        }
    } else {
        None
    };

    let caller = match caller {
        Some(c) => c,
        None => match state.device_auth.verify(token) {
            Some(name) => IngestCaller::LegacyDevice(name),
            None => return unauthorized().into_response(),
        },
    };

    let batch = match parse_ingest_payload(&body) {
        Ok(batch) => batch,
        Err(msg) => return bad_request(msg).into_response(),
    };

    if batch.fixes.len() > MAX_INGEST_BATCH_FIXES {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "error": "batch too large",
                "max": MAX_INGEST_BATCH_FIXES,
            })),
        )
            .into_response();
    }

    if let Err(msg) = validate_batch_seq(&batch) {
        return bad_request(msg).into_response();
    }

    let requested_flight_ids = collect_requested_flight_ids(&batch);

    let fixes: Vec<LiveFix> = batch
        .fixes
        .into_iter()
        .map(LiveEngine::input_to_fix)
        .collect();
    for fix in &fixes {
        if let Err(msg) = validate_ingest_fix(&state.live, fix) {
            return bad_request(msg).into_response();
        }
    }

    match caller {
        IngestCaller::DbDevice(device_id) => {
            let db = state.club.as_ref().expect("club db present");
            let mismatch_key = format!("db:{device_id}");
            let omit_flight_id = requested_flight_ids.is_empty();
            if let Some(open_flight) = db.open_flight_context_for_device(&device_id).await {
                if !flight_binding_matches(&requested_flight_ids, &open_flight.flight_id) {
                    return throttled_flight_mismatch(&state.live, &mismatch_key);
                }
                if let Some(club_id) = &open_flight.club_id {
                    if db.club_operational(club_id).await.is_err() {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(json!({ "error": "club unavailable" })),
                        )
                            .into_response();
                    }
                }
                let input_len = fixes.len();
                let min_allowed_t = effective_pre_start_min_t(
                    open_flight.started_at,
                    open_flight.previous_flight_ended_at,
                    omit_flight_id,
                );
                let mut kept = Vec::with_capacity(input_len);
                let mut dropped_pre_start = 0usize;
                for fix in fixes {
                    if fix.t < min_allowed_t {
                        dropped_pre_start += 1;
                    } else {
                        kept.push(fix);
                    }
                }
                if input_len > 0 && kept.is_empty() {
                    // All fixes were pre-start leftovers: drop without charging the 120/60s cap.
                    return (
                        StatusCode::OK,
                        Json(json!({
                            "ok": true,
                            "accepted": 0,
                            "duplicates": 0,
                            "droppedPreStart": dropped_pre_start,
                        })),
                    )
                        .into_response();
                }
                let rate_key = format!("club-flight:{device_id}");
                if let Err(retry_after) = state
                    .live
                    .allow_ingest_fixes(
                        &rate_key,
                        kept.len(),
                        crate::live::engine::MAX_CLUB_INGEST_FIXES_PER_MINUTE,
                    )
                    .await
                {
                    return (
                        StatusCode::TOO_MANY_REQUESTS,
                        [(header::RETRY_AFTER, retry_after.to_string())],
                        Json(json!({ "error": "rate limited" })),
                    )
                        .into_response();
                }
                if kept.is_empty() {
                    return (
                        StatusCode::OK,
                        Json(json!({
                            "ok": true,
                            "accepted": 0,
                            "duplicates": 0,
                            "droppedPreStart": dropped_pre_start,
                        })),
                    )
                        .into_response();
                }
                return match db
                    .append_fixes_for_open_flight(
                        &open_flight.flight_id,
                        &device_id,
                        &kept,
                        omit_flight_id,
                    )
                    .await
                {
                    Ok(outcome) => (
                        StatusCode::OK,
                        Json(json!({
                            "ok": true,
                            "accepted": outcome.accepted,
                            "duplicates": outcome.duplicates,
                            "droppedPreStart": dropped_pre_start + outcome.dropped_pre_start,
                        })),
                    )
                        .into_response(),
                    Err(AppendOpenFlightError::FlightMismatch) => {
                        if !omit_flight_id {
                            throttled_flight_mismatch(&state.live, &mismatch_key)
                        } else {
                            bad_request("no active flight").into_response()
                        }
                    }
                    Err(AppendOpenFlightError::Db) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": "db error" })),
                    )
                        .into_response(),
                };
            }
            if !omit_flight_id {
                return throttled_flight_mismatch(&state.live, &mismatch_key);
            }
            // Guest / unverified device with a live check-in: approval gates `cleared`, not sharing.
            if let Some(club_id) = db
                .club_id_for_device_checkin(&device_id, state.live.now_ms())
                .await
            {
                if db.club_operational(&club_id).await.is_err() {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(json!({ "error": "club unavailable" })),
                    )
                        .into_response();
                }
                let guest_key = format!("club:{club_id}:{device_id}");
                if !state.live.allow_ingest(&guest_key).await {
                    return (
                        StatusCode::TOO_MANY_REQUESTS,
                        Json(json!({ "error": "rate limited" })),
                    )
                        .into_response();
                }
                let total = fixes.len();
                return match state.live.ingest_fixes(&guest_key, fixes).await {
                    Ok(n) => (
                        StatusCode::OK,
                        Json(json!({
                            "ok": true,
                            "accepted": n,
                            "duplicates": total.saturating_sub(n),
                            "droppedPreStart": 0,
                        })),
                    )
                        .into_response(),
                    Err(msg) => bad_request(msg).into_response(),
                };
            }
            bad_request("no active flight").into_response()
        }
        IngestCaller::LegacyDevice(device) => {
            if !requested_flight_ids.is_empty() {
                return throttled_flight_mismatch(&state.live, &format!("legacy:{device}"));
            }
            if !state.live.allow_ingest(device).await {
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({ "error": "rate limited" })),
                )
                    .into_response();
            }
            let total = fixes.len();
            match state.live.ingest_fixes(device, fixes).await {
                Ok(n) => (
                    StatusCode::OK,
                    Json(json!({
                        "ok": true,
                        "accepted": n,
                        "duplicates": total.saturating_sub(n),
                        "droppedPreStart": 0,
                    })),
                )
                    .into_response(),
                Err(msg) => bad_request(msg).into_response(),
            }
        }
    }
}

#[derive(Debug, Deserialize, Default, Clone)]
pub struct OsmAndQuery {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub deviceid: Option<String>,
    #[serde(default)]
    pub lat: Option<String>,
    #[serde(default)]
    pub lon: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub speed: Option<String>,
    #[serde(default)]
    pub bearing: Option<String>,
    #[serde(default)]
    pub heading: Option<String>,
    #[serde(default)]
    pub altitude: Option<String>,
    #[serde(default)]
    pub accuracy: Option<String>,
    #[serde(default)]
    pub flight_id: Option<String>,
    #[serde(default, rename = "flightId")]
    pub flight_id_camel: Option<String>,
}

fn collect_osmand_flight_ids(q: &OsmAndQuery) -> Vec<String> {
    let mut ids = Vec::new();
    if let Some(id) = &q.flight_id {
        ids.push(id.clone());
    }
    if let Some(id) = &q.flight_id_camel {
        ids.push(id.clone());
    }
    ids
}

fn merge_osmand(mut base: OsmAndQuery, overlay: OsmAndQuery) -> OsmAndQuery {
    macro_rules! pick {
        ($field:ident) => {
            if overlay.$field.is_some() {
                base.$field = overlay.$field;
            }
        };
    }
    pick!(id);
    pick!(deviceid);
    pick!(lat);
    pick!(lon);
    pick!(timestamp);
    pick!(speed);
    pick!(bearing);
    pick!(heading);
    pick!(altitude);
    pick!(accuracy);
    pick!(flight_id);
    pick!(flight_id_camel);
    base
}

fn is_form_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| {
            ct.split(';')
                .next()
                .unwrap_or(ct)
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        })
        .unwrap_or(false)
}

fn bearer_device_token(headers: &HeaderMap) -> Option<String> {
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?;
    auth.strip_prefix("Bearer ")
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

pub fn parse_osmand_request(
    headers: &HeaderMap,
    query: OsmAndQuery,
    body: &[u8],
) -> Result<OsmAndQuery, &'static str> {
    let mut q = query;
    if q.id.is_none() && q.deviceid.is_none() {
        if let Some(token) = bearer_device_token(headers) {
            q.id = Some(token);
        }
    }
    if is_form_content_type(headers) || (!body.is_empty() && q.id.is_none() && q.lat.is_none()) {
        let from_body: OsmAndQuery =
            serde_urlencoded::from_bytes(body).map_err(|_| "invalid form body")?;
        q = merge_osmand(q, from_body);
    }
    Ok(q)
}

fn parse_f64(raw: Option<&String>) -> Option<f64> {
    raw.and_then(|s| s.parse::<f64>().ok())
}

fn parse_timestamp_ms(raw: Option<&String>) -> Option<i64> {
    let s = raw?;
    if let Ok(sec) = s.parse::<i64>() {
        if sec > 1_000_000_000_000 {
            return Some(sec);
        }
        return Some(sec * 1000);
    }
    None
}

pub fn osmand_fix_from_query(q: &OsmAndQuery) -> Result<(String, LiveFix), &'static str> {
    let token =
        q.id.as_deref()
            .or(q.deviceid.as_deref())
            .ok_or("missing id")?
            .trim();
    if token.is_empty() {
        return Err("missing id");
    }
    let lat = parse_f64(q.lat.as_ref()).ok_or("missing lat")?;
    let lon = parse_f64(q.lon.as_ref()).ok_or("missing lon")?;
    if !valid_lat_lon(lat, lon) {
        return Err("invalid lat/lon");
    }
    let t = parse_timestamp_ms(q.timestamp.as_ref()).unwrap_or(0);
    let speed_knots = parse_f64(q.speed.as_ref());
    let speed = speed_knots.map(knots_to_mps);
    let course = parse_f64(q.bearing.as_ref()).or_else(|| parse_f64(q.heading.as_ref()));
    let alt = parse_f64(q.altitude.as_ref());
    let accuracy = parse_f64(q.accuracy.as_ref());
    let fix = LiveFix {
        t,
        lat,
        lon,
        alt,
        alt_ref: alt.map(|_| ALT_REF_WGS84_ELLIPSOID.to_string()),
        speed,
        course,
        accuracy,
    };
    Ok((token.to_string(), fix))
}

/// Traccar Client (Flutter, [traccar-client-sdk]) POSTs `application/x-www-form-urlencoded`
/// with fields `id`, `lat`, `lon`, `timestamp` (unix s), `speed` (knots), `bearing`, `altitude`,
/// `accuracy`, `batt`, `charge`, `alarm` — see `HttpUploader.kt`. Legacy native Android builds
/// used query strings on POST instead ([traccar-client-android] `ProtocolFormatter.kt`).
pub async fn osmand_ingest(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<OsmAndQuery>,
    body: Bytes,
) -> impl IntoResponse {
    let q = match parse_osmand_request(&headers, query, &body) {
        Ok(q) => q,
        Err(msg) => return bad_request(msg).into_response(),
    };
    let (token, mut fix) = match osmand_fix_from_query(&q) {
        Ok(v) => v,
        Err(msg) => return bad_request(msg).into_response(),
    };
    if fix.t == 0 {
        fix.t = state.live.now_ms();
    }
    let requested_flight_ids = collect_osmand_flight_ids(&q);
    let omit_flight_id = requested_flight_ids.is_empty();
    if let Some(db) = &state.club {
        if let Some(device_id) = db.resolve_device_token(&token).await {
            if db.device_account_suspended(&device_id).await {
                return unauthorized().into_response();
            }
            let mismatch_key = format!("db:{device_id}");
            if let Some(open_flight) = db.open_flight_context_for_device(&device_id).await {
                if !flight_binding_matches(&requested_flight_ids, &open_flight.flight_id) {
                    return throttled_flight_mismatch(&state.live, &mismatch_key);
                }
                if let Some(club_id) = &open_flight.club_id {
                    if db.club_operational(club_id).await.is_err() {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(json!({ "error": "club unavailable" })),
                        )
                            .into_response();
                    }
                }
                let min_allowed_t = effective_pre_start_min_t(
                    open_flight.started_at,
                    open_flight.previous_flight_ended_at,
                    omit_flight_id,
                );
                if fix.t < min_allowed_t {
                    // Drop pre-start leftover without burning rate-limit budget or writing rows;
                    // return 200 OK so OsmAnd/Traccar clients discard stale buffered positions.
                    return (StatusCode::OK, "OK").into_response();
                }
                let rate_key = format!("club-flight:{device_id}");
                if let Err(retry_after) = state
                    .live
                    .allow_ingest_fixes(
                        &rate_key,
                        1,
                        crate::live::engine::MAX_CLUB_INGEST_FIXES_PER_MINUTE,
                    )
                    .await
                {
                    return (
                        StatusCode::TOO_MANY_REQUESTS,
                        [(header::RETRY_AFTER, retry_after.to_string())],
                        Json(json!({ "error": "rate limited" })),
                    )
                        .into_response();
                }
                return match db
                    .append_fixes_for_open_flight(
                        &open_flight.flight_id,
                        &device_id,
                        std::slice::from_ref(&fix),
                        omit_flight_id,
                    )
                    .await
                {
                    Ok(_) => (StatusCode::OK, "OK").into_response(),
                    Err(AppendOpenFlightError::FlightMismatch) => {
                        if !omit_flight_id {
                            throttled_flight_mismatch(&state.live, &mismatch_key)
                        } else {
                            bad_request("no active flight").into_response()
                        }
                    }
                    Err(AppendOpenFlightError::Db) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": "db error" })),
                    )
                        .into_response(),
                };
            }
            if !omit_flight_id {
                return throttled_flight_mismatch(&state.live, &mismatch_key);
            }
            // Guest / unverified device with a live check-in: approval gates `cleared`, not sharing.
            if let Some(club_id) = db
                .club_id_for_device_checkin(&device_id, state.live.now_ms())
                .await
            {
                if db.club_operational(&club_id).await.is_err() {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(json!({ "error": "club unavailable" })),
                    )
                        .into_response();
                }
                let guest_key = format!("club:{club_id}:{device_id}");
                if !state.live.allow_ingest(&guest_key).await {
                    return (
                        StatusCode::TOO_MANY_REQUESTS,
                        Json(json!({ "error": "rate limited" })),
                    )
                        .into_response();
                }
                return match state.live.ingest_fixes(&guest_key, vec![fix]).await {
                    Ok(_) => (StatusCode::OK, "OK").into_response(),
                    Err(msg) => bad_request(msg).into_response(),
                };
            }
            return bad_request("no active flight").into_response();
        }
    }
    let device = match state.device_auth.verify(&token) {
        Some(name) => name,
        None => return unauthorized().into_response(),
    };
    if !omit_flight_id {
        return throttled_flight_mismatch(&state.live, &format!("legacy:{device}"));
    }
    if !state.live.allow_ingest(device).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "rate limited" })),
        )
            .into_response();
    }
    match state.live.ingest_fixes(device, vec![fix]).await {
        Ok(_) => (StatusCode::OK, "OK").into_response(),
        Err(msg) => bad_request(msg).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn traccar_sdk_form_fixture() {
        let body = b"id=0123456789abcdef0123456789abcdef&lat=48.8566&lon=2.3522&timestamp=1609459200&speed=15&bearing=270&altitude=35&accuracy=10&batt=75";
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        let q = parse_osmand_request(&headers, OsmAndQuery::default(), body).unwrap();
        let (token, fix) = osmand_fix_from_query(&q).expect("parse");
        assert_eq!(token, "0123456789abcdef0123456789abcdef");
        assert_eq!(fix.t, 1_609_459_200_000);
        assert!((fix.speed.unwrap() - knots_to_mps(15.0)).abs() < 0.001);
    }

    #[test]
    fn legacy_query_string_fixture() {
        let q = OsmAndQuery {
            id: Some("0123456789abcdef0123456789abcdef".to_string()),
            lat: Some("48.8566".to_string()),
            lon: Some("2.3522".to_string()),
            timestamp: Some("1609459200".to_string()),
            speed: Some("15".to_string()),
            bearing: Some("270".to_string()),
            altitude: Some("35".to_string()),
            accuracy: Some("10".to_string()),
            ..Default::default()
        };
        let headers = HeaderMap::new();
        let merged = parse_osmand_request(&headers, q, b"").unwrap();
        let (_, fix) = osmand_fix_from_query(&merged).expect("parse");
        assert_eq!(fix.t, 1_609_459_200_000);
        assert_eq!(fix.alt, Some(35.0));
        assert_eq!(fix.alt_ref.as_deref(), Some(ALT_REF_WGS84_ELLIPSOID));
    }

    #[test]
    fn osmand_parses_optional_flight_id_params() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/x-www-form-urlencoded"),
        );
        let q1 = parse_osmand_request(
            &headers,
            OsmAndQuery::default(),
            b"id=tok123&lat=-33.4&lon=-70.6&timestamp=1700000000&flight_id=fl-snake",
        )
        .unwrap();
        assert_eq!(collect_osmand_flight_ids(&q1), vec!["fl-snake".to_string()]);

        let q2 = parse_osmand_request(
            &HeaderMap::new(),
            OsmAndQuery {
                id: Some("tok123".to_string()),
                lat: Some("-33.4".to_string()),
                lon: Some("-70.6".to_string()),
                flight_id_camel: Some("fl-camel".to_string()),
                ..Default::default()
            },
            b"",
        )
        .unwrap();
        assert_eq!(collect_osmand_flight_ids(&q2), vec!["fl-camel".to_string()]);
    }

    #[test]
    fn parse_ingest_payload_accepts_batch_single_flight_id_and_seq_and_rejects_malformed() {
        // Batch with snake_case flight_id + batch/fix seq
        let b1 = parse_ingest_payload(
            br#"{"flight_id":"f1","seq":0,"fixes":[{"t":1700000000000,"lat":-33.4,"lon":-70.6,"seq":1}]}"#,
        )
        .unwrap();
        assert_eq!(b1.flight_id.as_deref(), Some("f1"));
        assert_eq!(b1.seq, Some(0));
        assert_eq!(b1.fixes.len(), 1);
        assert_eq!(b1.fixes[0].seq, Some(1));
        assert!(validate_batch_seq(&b1).is_ok());
        assert_eq!(collect_requested_flight_ids(&b1), vec!["f1".to_string()]);

        // Batch with camelCase flightId on batch and per-fix
        let b2 = parse_ingest_payload(
            br#"{"flightId":"f2","fixes":[{"t":1700000000000,"lat":-33.4,"lon":-70.6,"flightId":"f2"}]}"#,
        )
        .unwrap();
        assert_eq!(
            collect_requested_flight_ids(&b2),
            vec!["f2".to_string(), "f2".to_string()]
        );

        // Single-fix form with flightId + seq
        let s1 = parse_ingest_payload(
            br#"{"t":1700000000000,"lat":-33.4,"lon":-70.6,"flightId":"f3","seq":7}"#,
        )
        .unwrap();
        assert_eq!(s1.fixes.len(), 1);
        assert_eq!(collect_requested_flight_ids(&s1), vec!["f3".to_string()]);
        assert!(validate_batch_seq(&s1).is_ok());

        // Negative seq rejected
        let neg = parse_ingest_payload(
            br#"{"seq":-1,"fixes":[{"t":1700000000000,"lat":-33.4,"lon":-70.6}]}"#,
        )
        .unwrap();
        assert_eq!(validate_batch_seq(&neg), Err("invalid seq"));

        // Malformed "fixes" key must NOT fall through to single-fix parser
        assert!(parse_ingest_payload(
            br#"{"fixes":"bad","t":1700000000000,"lat":-33.4,"lon":-70.6}"#,
        )
        .is_err());
        assert!(parse_ingest_payload(
            br#"{"fixes":null,"t":1700000000000,"lat":-33.4,"lon":-70.6}"#,
        )
        .is_err());
        // Duplicate flight_id + flightId rejected
        assert!(parse_ingest_payload(
            br#"{"flight_id":"a","flightId":"b","fixes":[{"t":1700000000000,"lat":-33.4,"lon":-70.6}]}"#,
        )
        .is_err());
        // Non-integer seq rejected
        assert!(parse_ingest_payload(
            br#"{"seq":1.5,"fixes":[{"t":1700000000000,"lat":-33.4,"lon":-70.6}]}"#,
        )
        .is_err());
        assert!(parse_ingest_payload(
            br#"{"seq":"1","fixes":[{"t":1700000000000,"lat":-33.4,"lon":-70.6}]}"#,
        )
        .is_err());
    }

    #[test]
    fn effective_pre_start_min_t_fails_closed_for_omit_id_and_preserves_explicit_id() {
        let prev_end = 1_700_000_000_000_i64;
        let start_t = prev_end + 30_000; // B starts 30s after A ended (inside 120s skew)

        // Omit-id with previous flight ended inside 120s skew: drops T-10s, T, AND T+5s!
        let min_omit = effective_pre_start_min_t(start_t, Some(prev_end), true);
        assert_eq!(min_omit, start_t);
        assert!(prev_end - 10_000 < min_omit);
        assert!(prev_end < min_omit);
        assert!(prev_end + 5_000 < min_omit);
        assert!(prev_end + 40_000 >= min_omit);

        // Explicit flight_id keeps normal 120s pre-start skew tolerance unchanged.
        let min_bound = effective_pre_start_min_t(start_t, Some(prev_end), false);
        assert_eq!(min_bound, start_t - PRE_START_SKEW_MS);
        assert!(prev_end - 10_000 >= min_bound);
        assert!(prev_end + 5_000 >= min_bound);

        // Omit-id with no previous flight or old previous flight (>120s ago): keeps normal skew.
        assert_eq!(
            effective_pre_start_min_t(start_t, None, true),
            start_t - PRE_START_SKEW_MS
        );
        assert_eq!(
            effective_pre_start_min_t(start_t, Some(start_t - PRE_START_SKEW_MS - 60_000), true),
            start_t - PRE_START_SKEW_MS
        );

        // Defensive: if prev_end > start_t (clock skew), still strictly > prev_end.
        assert_eq!(
            effective_pre_start_min_t(start_t, Some(start_t + 5_000), true),
            start_t + 5_001
        );
    }
}
