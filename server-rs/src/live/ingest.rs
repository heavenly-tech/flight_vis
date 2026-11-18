use crate::{
    live::{
        engine::LiveEngine,
        types::{knots_to_mps, valid_lat_lon, LiveFix, ALT_REF_WGS84_ELLIPSOID},
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

pub async fn post_ingest(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let token = match bearer_token(&headers) {
        Some(token) => token,
        None => return unauthorized().into_response(),
    };
    let batch: crate::live::types::IngestBatch = match serde_json::from_slice(&body) {
        Ok(batch) => batch,
        Err(_) => return bad_request("invalid json").into_response(),
    };
    let fixes: Vec<LiveFix> = batch
        .fixes
        .into_iter()
        .map(LiveEngine::input_to_fix)
        .collect();
    if let Some(db) = &state.club {
        if let Some(device_id) = db.resolve_device_token(token).await {
            if let Some(flight_id) = db.open_flight_for_device(&device_id).await {
                let accepted = db
                    .append_fixes(&flight_id, &device_id, &fixes)
                    .await
                    .unwrap_or(0);
                return (
                    StatusCode::OK,
                    Json(json!({ "ok": true, "accepted": accepted })),
                )
                    .into_response();
            }
            return bad_request("no active flight").into_response();
        }
    }
    let device = match state.device_auth.verify(token) {
        Some(name) => name,
        None => return unauthorized().into_response(),
    };
    if !state.live.allow_ingest(device).await {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "rate limited" })),
        )
            .into_response();
    }
    match state.live.ingest_fixes(device, fixes).await {
        Ok(n) => (StatusCode::OK, Json(json!({ "ok": true, "accepted": n }))).into_response(),
        Err(msg) => bad_request(msg).into_response(),
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
    if let Some(db) = &state.club {
        if let Some(device_id) = db.resolve_device_token(&token).await {
            if let Some(flight_id) = db.open_flight_for_device(&device_id).await {
                if db
                    .append_fixes(&flight_id, &device_id, std::slice::from_ref(&fix))
                    .await
                    .is_ok()
                {
                    return (StatusCode::OK, "OK").into_response();
                }
            }
            return bad_request("no active flight").into_response();
        }
    }
    let device = match state.device_auth.verify(&token) {
        Some(name) => name,
        None => return unauthorized().into_response(),
    };
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
}
