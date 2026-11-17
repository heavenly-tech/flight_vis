use crate::club::{
    auth::{self, parse_display_cookie, parse_session_cookie},
    db::{
        now_ms, CheckinReject, ClubDb, DeleteError, EndError, PatchError, RegisterError,
        StartFlightError,
    },
    map::TailLivePosition,
    visibility::{FlightVisibility, PilotRole, PilotStatus, Viewer},
};
use crate::AppState;
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse, Json,
    },
    routing::{get, post},
    Router,
};
use futures_util::stream;
use serde::Deserialize;
use serde_json::json;
use std::{convert::Infallible, time::Duration};
use tokio_stream::StreamExt as _;

fn club_db(state: &AppState) -> Option<&ClubDb> {
    state.club.as_ref()
}

fn is_secure(headers: &HeaderMap) -> bool {
    let forwarded = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok());
    crate::auth::is_secure_request(forwarded)
}

fn viewer(state: &AppState, headers: &HeaderMap) -> Viewer {
    let session = parse_session_cookie(headers);
    let now = now_ms();
    state
        .club
        .as_ref()
        .map(|db| db.viewer_from_session(session.as_deref(), now))
        .unwrap_or(Viewer::Anonymous)
}

fn require_approved(viewer: &Viewer) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    match viewer {
        Viewer::Pilot {
            status: PilotStatus::Approved,
            ..
        } => Ok(()),
        Viewer::Pilot {
            status: PilotStatus::Pending,
            ..
        } => Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "pending approval" })),
        )),
        Viewer::Anonymous => Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )),
    }
}

fn require_admin(viewer: &Viewer) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    require_approved(viewer)?;
    match viewer {
        Viewer::Pilot {
            role: PilotRole::Admin,
            ..
        } => Ok(()),
        _ => Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )),
    }
}

#[derive(Deserialize)]
struct RegisterBody {
    name: String,
    email: String,
    password: String,
    #[serde(default)]
    license_no: String,
}

async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterBody>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let club_id = match db.default_club_id() {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "club missing" })),
            )
                .into_response();
        }
    };
    match db.register(
        &club_id,
        body.name.trim(),
        body.email.trim(),
        body.license_no.trim(),
        &body.password,
    ) {
        Ok(id) => (
            StatusCode::CREATED,
            Json(json!({ "ok": true, "pilotId": id })),
        )
            .into_response(),
        Err(RegisterError::EmailExists) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "email exists" })),
        )
            .into_response(),
        Err(RegisterError::WeakPassword) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "weak password" })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "register failed" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct LoginBody {
    email: String,
    password: String,
}

async fn club_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let club_id = db.default_club_id().unwrap_or_default();
    let now = now_ms();
    match db.login(&club_id, body.email.trim(), &body.password, now) {
        Ok((session_id, pilot)) => {
            let cookie = auth::set_session_cookie(&session_id, is_secure(&headers));
            (
                StatusCode::OK,
                [(header::SET_COOKIE, cookie)],
                Json(json!({
                    "ok": true,
                    "pilot": {
                        "id": pilot.id,
                        "name": pilot.name,
                        "email": pilot.email,
                        "status": pilot.status,
                        "role": pilot.role,
                    }
                })),
            )
                .into_response()
        }
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response(),
    }
}

async fn club_logout(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let (Some(db), Some(session)) = (club_db(&state), parse_session_cookie(&headers)) {
        let _ = db.logout(&session);
    }
    let cookie = auth::clear_session_cookie(is_secure(&headers));
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "ok": true })),
    )
        .into_response()
}

async fn club_me(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return Json(json!({ "pilot": null })).into_response();
    };
    let session = parse_session_cookie(&headers);
    let now = now_ms();
    let (_viewer, pilot) = db.viewer_with_pilot(session.as_deref(), now);
    match pilot {
        Some(p) => Json(json!({
            "pilot": {
                "id": p.id,
                "name": p.name,
                "email": p.email,
                "status": p.status,
                "role": p.role,
                "licenseNo": p.license_no,
            }
        }))
        .into_response(),
        None => Json(json!({ "pilot": null })).into_response(),
    }
}

async fn me_tails(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Viewer::Pilot {
        id,
        status: PilotStatus::Approved,
        ..
    } = v
    else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return Json(json!({ "tails": [] })).into_response();
    };
    match db.me_tails(&id, now_ms()) {
        Ok(tails) => Json(json!({ "tails": tails })).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct StartFlightBody {
    tail: String,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    show_pilot_name_public: Option<bool>,
}

fn parse_visibility(raw: Option<&str>) -> Result<FlightVisibility, &'static str> {
    match raw {
        None => Ok(FlightVisibility::default_start()),
        Some(s) => FlightVisibility::parse(s).ok_or("invalid visibility"),
    }
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

pub async fn post_club_flight_start(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let parsed: StartFlightBody = match serde_json::from_slice(&body) {
        Ok(p) => p,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid JSON" })),
            )
                .into_response();
        }
    };
    let v = viewer(&state, &headers);
    let Viewer::Pilot {
        id: pilot_id,
        status: PilotStatus::Approved,
        ..
    } = v
    else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let token = match bearer_token(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "device token required" })),
            )
                .into_response();
        }
    };
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let device_id = match db.resolve_device_token(&token) {
        Some(id) => id,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "invalid device token" })),
            )
                .into_response();
        }
    };
    let visibility = match parse_visibility(parsed.visibility.as_deref()) {
        Ok(v) => v,
        Err(msg) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response();
        }
    };
    let show_name = parsed.show_pilot_name_public.unwrap_or(false);
    match db.start_flight(
        &pilot_id,
        &device_id,
        parsed.tail.trim(),
        visibility,
        show_name,
        now_ms(),
    ) {
        Ok(flight_id) => Json(json!({
            "id": flight_id,
            "visibility": visibility.as_str(),
            "showPilotNamePublic": show_name,
        }))
        .into_response(),
        Err(StartFlightError::NoCheckout) => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "no checkout" })),
        )
            .into_response(),
        Err(StartFlightError::NoTail) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unknown or inactive tail" })),
        )
            .into_response(),
        Err(StartFlightError::AlreadyActive) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "active flight exists" })),
        )
            .into_response(),
        Err(StartFlightError::NoCheckin) => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "check-in required" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
pub struct PatchFlightBody {
    visibility: Option<String>,
    show_pilot_name_public: Option<bool>,
}

pub async fn patch_club_flight_by_id(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(flight_id): Path<String>,
    Json(body): Json<PatchFlightBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let visibility = match &body.visibility {
        None => None,
        Some(s) => match parse_visibility(Some(s.as_str())) {
            Ok(v) => Some(v),
            Err(msg) => {
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response();
            }
        },
    };
    match db.patch_flight(&id, &flight_id, visibility, body.show_pilot_name_public) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(PatchError::NotFound) => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
        }
        Err(PatchError::Forbidden) => {
            (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))).into_response()
        }
    }
}

async fn end_club_flight(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(flight_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    match db.end_flight(&id, &flight_id, now_ms()) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(EndError::NotFound) => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
        }
        Err(EndError::Forbidden) => {
            (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))).into_response()
        }
        Err(EndError::NotActive) => {
            (StatusCode::CONFLICT, Json(json!({ "error": "not active" }))).into_response()
        }
    }
}

async fn club_map(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    if require_approved(&v).is_err() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return Json(json!({ "aircraft": [] })).into_response();
    };
    let positions: Vec<TailLivePosition> = db.club_map_for_viewer(&v);
    Json(json!({ "aircraft": positions })).into_response()
}

async fn public_map(State(state): State<AppState>) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return Json(json!({ "aircraft": [] })).into_response();
    };
    Json(json!({ "aircraft": db.public_map() })).into_response()
}

async fn admin_map(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    if require_admin(&v).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return Json(json!({ "aircraft": [] })).into_response();
    };
    Json(json!({ "aircraft": db.admin_map(&id) })).into_response()
}

#[derive(Deserialize)]
struct AdminTailsBody {
    tails: Vec<String>,
}

async fn get_admin_map_tails(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    if require_admin(&v).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return Json(json!({ "tails": [] })).into_response();
    };
    Json(json!({ "tails": db.admin_map_tails(&id) })).into_response()
}

async fn put_admin_map_tails(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AdminTailsBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    if require_admin(&v).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    match db.set_admin_map_tails(&id, &body.tails) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn club_flight_fixes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(flight_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    if db.flight_visible_to(&v, &flight_id).is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    }
    let fixes = db.fixes_for_flight(&flight_id);
    Json(json!({ "fixes": fixes })).into_response()
}

async fn club_flight_gpx(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(flight_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let Some(flight) = db.flight_visible_to(&v, &flight_id) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    };
    let fixes = db.fixes_for_flight(&flight_id);
    let gpx = fixes_to_gpx(&flight.tail, &fixes);
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/gpx+xml")],
        gpx,
    )
        .into_response()
}

fn fixes_to_gpx(name: &str, fixes: &[crate::live::LiveFix]) -> String {
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"flight_vis\"><trk><name>{name}</name><trkseg>\n"
    );
    for f in fixes {
        out.push_str(&format!("<trkpt lat=\"{}\" lon=\"{}\">", f.lat, f.lon));
        if let Some(alt) = f.alt {
            out.push_str(&format!("<ele>{alt}</ele>"));
        }
        out.push_str(&format!(
            "<time>{}</time></trkpt>\n",
            unix_ms_to_rfc3339(f.t)
        ));
    }
    out.push_str("</trkseg></trk></gpx>");
    out
}

fn unix_ms_to_rfc3339(ms: i64) -> String {
    let sec = ms.div_euclid(1000);
    match time::OffsetDateTime::from_unix_timestamp(sec) {
        Ok(dt) => dt
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string()),
        Err(_) => "1970-01-01T00:00:00Z".to_string(),
    }
}

async fn club_flight_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(flight_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    if db.flight_visible_to(&v, &flight_id).is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    }
    let db = state.club.clone().unwrap();
    let viewer = v;
    let flight_id_capture = flight_id.clone();
    let backfill: Vec<_> = db.fixes_for_flight(&flight_id);
    let stream = stream::iter(backfill.into_iter().enumerate().map(move |(i, fix)| {
        Ok::<Event, Infallible>(Event::default().id(i.to_string()).json_data(fix).unwrap())
    }))
    .chain(stream::unfold(
        (db, flight_id_capture, viewer, 0_i64),
        #[allow(clippy::question_mark)]
        |(db, fid, viewer, mut last_t)| async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if db.flight_visible_to(&viewer, &fid).is_none() {
                return None;
            }
            let fixes = db.fixes_for_flight(&fid);
            let fresh: Vec<_> = fixes.into_iter().filter(|f| f.t > last_t).collect();
            if let Some(t) = fresh.last().map(|f| f.t) {
                last_t = t;
            }
            let event = if fresh.is_empty() {
                Event::default().comment("ping")
            } else {
                Event::default()
                    .json_data(json!({ "fixes": fresh }))
                    .unwrap()
            };
            Some((Ok(event), (db, fid, viewer, last_t)))
        },
    ));
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn my_flights(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return Json(json!({ "flights": [] })).into_response();
    };
    Json(json!({ "flights": db.list_my_flights(&id) })).into_response()
}

async fn delete_my_flight(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(flight_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    match db.delete_my_flight(&id, &flight_id) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(DeleteError::NotFound) => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
        }
        Err(DeleteError::Forbidden) => {
            (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))).into_response()
        }
        Err(DeleteError::Db(_)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

// --- admin ---
#[derive(Deserialize)]
struct TypeBody {
    code: String,
}

async fn list_types(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    if require_admin(&v).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return Json(json!({ "types": [] })).into_response();
    };
    let club = db.default_club_id().unwrap_or_default();
    Json(json!({ "types": db.list_types(&club) })).into_response()
}

async fn admin_add_type(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TypeBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    if require_admin(&v).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let club = db.default_club_id().unwrap_or_default();
    match db.upsert_type(&club, body.code.trim()) {
        Ok(id) => Json(json!({ "id": id })).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct AircraftBody {
    tail: String,
    #[serde(rename = "type")]
    type_code: String,
    #[serde(default = "default_true")]
    active: bool,
}

fn default_true() -> bool {
    true
}

async fn list_aircraft(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    if require_admin(&v).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return Json(json!({ "aircraft": [] })).into_response();
    };
    let club = db.default_club_id().unwrap_or_default();
    Json(json!({ "aircraft": db.list_aircraft(&club) })).into_response()
}

async fn admin_add_aircraft(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AircraftBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    if require_admin(&v).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let club = db.default_club_id().unwrap_or_default();
    match db.upsert_aircraft(&club, body.tail.trim(), body.type_code.trim(), body.active) {
        Ok(id) => Json(json!({ "id": id })).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn list_pilots_admin(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    if require_admin(&v).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return Json(json!({ "pilots": [] })).into_response();
    };
    Json(json!({ "pilots": db.list_all_pilots() })).into_response()
}

async fn admin_pending(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    if require_admin(&v).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return Json(json!({ "pilots": [] })).into_response();
    };
    Json(json!({ "pilots": db.list_pending_pilots() })).into_response()
}

async fn admin_approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pilot_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    if require_admin(&v).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    match db.approve_pilot(&pilot_id) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct CheckoutBody {
    pilot_id: String,
    #[serde(rename = "type")]
    type_code: String,
    tail: Option<String>,
    expires_at: Option<i64>,
}

async fn list_checkouts_admin(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    if require_admin(&v).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return Json(json!({ "checkouts": [] })).into_response();
    };
    Json(json!({ "checkouts": db.list_checkouts() })).into_response()
}

async fn admin_checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CheckoutBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    if require_admin(&v).is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    match db.add_checkout(
        &body.pilot_id,
        &body.type_code,
        body.tail.as_deref(),
        body.expires_at,
    ) {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct DeviceBody {
    #[serde(default)]
    label: String,
}

async fn create_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DeviceBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Viewer::Pilot {
        id,
        status: PilotStatus::Approved,
        ..
    } = v
    else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    match db.create_device(&id, body.label.trim()) {
        Ok((device_id, _token)) => Json(json!({
            "id": device_id,
            "label": body.label,
        }))
        .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn list_devices(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return Json(json!({ "devices": [] })).into_response();
    };
    Json(json!({ "devices": db.list_my_devices(&id) })).into_response()
}

fn expires_at_iso_utc(ms: i64) -> Option<String> {
    let secs = ms.div_euclid(1000);
    let nanos = (ms.rem_euclid(1000) * 1_000_000) as u32;
    let dt = time::OffsetDateTime::from_unix_timestamp(secs).ok()?;
    let dt = if nanos == 0 {
        dt
    } else {
        dt.replace_nanosecond(nanos).ok()?
    };
    dt.format(&time::format_description::well_known::Rfc3339)
        .ok()
}

fn display_context(headers: &HeaderMap, db: &ClubDb) -> Option<(String, String)> {
    let raw = parse_display_cookie(headers)?;
    db.resolve_display_session(&raw)
        .map(|(_id, club_id, admin_id)| (club_id, admin_id))
}

#[derive(Deserialize)]
struct CheckinBody {
    token: String,
}

async fn post_checkin(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CheckinBody>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let now = now_ms();
    let mut pilot_id: Option<String> = None;
    let mut device_id: Option<String> = None;
    if let Some(bearer) = bearer_token(&headers) {
        if let Some(dev) = db.resolve_device_token(&bearer) {
            device_id = Some(dev.clone());
            pilot_id = db.device_pilot_id(&dev);
        }
    }
    if pilot_id.is_none() {
        let v = viewer(&state, &headers);
        if let Viewer::Pilot {
            id,
            status: PilotStatus::Approved,
            ..
        } = v
        {
            pilot_id = Some(id);
        }
    }
    let Some(pilot_id) = pilot_id else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    if let Some(dev) = &device_id {
        if db.device_pilot_id(dev).as_deref() != Some(pilot_id.as_str()) {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "device mismatch" })),
            )
                .into_response();
        }
    }
    match db.submit_checkin(&pilot_id, device_id.as_deref(), body.token.trim(), now) {
        Ok(record) => {
            let Some(expires_at) = expires_at_iso_utc(record.expires_at) else {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "timestamp format" })),
                )
                    .into_response();
            };
            Json(json!({
                "ok": true,
                "id": record.id,
                "expiresAt": expires_at,
            }))
            .into_response()
        }
        Err(CheckinReject::Format) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid token", "reason": "format" })),
        )
            .into_response(),
        Err(CheckinReject::ForgedHmac) => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "invalid token", "reason": "forged_hmac" })),
        )
            .into_response(),
        Err(CheckinReject::OldStep) => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "invalid token", "reason": "old_step" })),
        )
            .into_response(),
        Err(CheckinReject::WrongClub) => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "invalid token", "reason": "wrong_club" })),
        )
            .into_response(),
        Err(CheckinReject::Replay) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "token already used", "reason": "replay" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct DisplayActivateBody {
    token: String,
}

async fn display_activate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DisplayActivateBody>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let token = body.token.trim();
    let Some(_ctx) = db.resolve_display_session(token) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid display token" })),
        )
            .into_response();
    };
    let cookie = auth::set_display_cookie(token, is_secure(&headers));
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "ok": true })),
    )
        .into_response()
}

async fn display_checkin_qr(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return Json(json!({ "error": "club disabled" })).into_response();
    };
    let Some((club_id, _admin)) = display_context(&headers, db) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "display session required" })),
        )
            .into_response();
    };
    let now = now_ms();
    match db.mint_checkin_qr_token(&club_id, now) {
        Ok(_token) => {
            let step = crate::club::checkin::current_step(now);
            Json(json!({
                "step": step,
                "stepSeconds": crate::club::checkin::CHECKIN_STEP_SEC,
                "serverTimeMs": now,
            }))
            .into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn display_map(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return Json(json!({ "aircraft": [] })).into_response();
    };
    let Some((_club_id, admin_id)) = display_context(&headers, db) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "display session required" })),
        )
            .into_response();
    };
    let rows = db.display_map(&admin_id);
    Json(json!({ "aircraft": rows })).into_response()
}

#[derive(Deserialize, Default)]
struct DisplaySessionBody {
    label: String,
}

async fn admin_create_display(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DisplaySessionBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Viewer::Pilot {
        id,
        role: PilotRole::Admin,
        ..
    } = v
    else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin required" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    match db.create_display_session(&id, body.label.trim()) {
        Ok((session_id, token)) => Json(json!({
            "id": session_id,
            "token": token,
            "screenUrl": "/club/screen",
        }))
        .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn admin_list_display(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Viewer::Pilot {
        id,
        role: PilotRole::Admin,
        ..
    } = v
    else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin required" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return Json(json!({ "sessions": [] })).into_response();
    };
    Json(json!({ "sessions": db.list_display_sessions(&id) })).into_response()
}

async fn admin_revoke_display(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Viewer::Pilot {
        id,
        role: PilotRole::Admin,
        ..
    } = v
    else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin required" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    match db.revoke_display_session(&id, &session_id) {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

fn html_csp_header() -> (header::HeaderName, &'static str) {
    (
        header::CONTENT_SECURITY_POLICY,
        crate::static_files::HTML_CSP,
    )
}

async fn club_screen_page(State(state): State<AppState>) -> impl IntoResponse {
    let path = state.dist_dir.join("club-screen.html");
    match tokio::fs::read_to_string(&path).await {
        Ok(html) => (
            [(html_csp_header())],
            Html(html),
        )
            .into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            "Club screen page not built (npm run build)",
        )
            .into_response(),
    }
}

async fn club_screen_qr_svg(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Some((club_id, _admin)) = display_context(&headers, db) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let now = now_ms();
    let token = match db.mint_checkin_qr_token(&club_id, now) {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let svg = match crate::club::qr::svg_for_payload(&token, 280) {
        Ok(s) => s,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    (
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_TYPE, "image/svg+xml; charset=utf-8"),
        ],
        svg,
    )
        .into_response()
}

async fn device_pair_qr_svg(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers);
    let Viewer::Pilot {
        id,
        status: PilotStatus::Approved,
        ..
    } = v
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(db) = club_db(&state) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Some(token) = db.device_pair_token_for_pilot(&device_id, &id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let svg = match crate::club::qr::svg_for_payload(&token, 180) {
        Ok(s) => s,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    (
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_TYPE, "image/svg+xml; charset=utf-8"),
        ],
        svg,
    )
        .into_response()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/club/screen", get(club_screen_page))
        .route("/api/checkin", post(post_checkin))
        .route("/api/display/activate", post(display_activate))
        .route("/api/display/checkin-qr", get(display_checkin_qr))
        .route("/api/club/screen/qr.svg", get(club_screen_qr_svg))
        .route("/api/devices/pair/{id}/qr.svg", get(device_pair_qr_svg))
        .route("/api/display/map", get(display_map))
        .route(
            "/api/admin/display-sessions",
            get(admin_list_display).post(admin_create_display),
        )
        .route(
            "/api/admin/display-sessions/{id}/revoke",
            post(admin_revoke_display),
        )
        .route("/api/club/register", post(register))
        .route("/api/club/login", post(club_login))
        .route("/api/club/logout", post(club_logout).get(club_logout))
        .route("/api/club/me", get(club_me))
        .route("/api/me/tails", get(me_tails))
        .route("/api/me/flights", get(my_flights))
        .route(
            "/api/me/flights/{id}",
            axum::routing::delete(delete_my_flight),
        )
        .route("/api/me/devices", get(list_devices).post(create_device))
        .route("/api/flights/{id}/end", post(end_club_flight))
        .route("/api/flights/{id}/fixes", get(club_flight_fixes))
        .route("/api/flights/{id}/stream", get(club_flight_stream))
        .route("/api/flights/{id}/gpx", get(club_flight_gpx))
        .route("/api/club/map", get(club_map))
        .route("/api/club/map/public", get(public_map))
        .route("/api/admin/map", get(admin_map))
        .route(
            "/api/admin/map/tails",
            get(get_admin_map_tails).post(put_admin_map_tails),
        )
        .route(
            "/api/admin/aircraft-types",
            get(list_types).post(admin_add_type),
        )
        .route(
            "/api/admin/aircraft",
            get(list_aircraft).post(admin_add_aircraft),
        )
        .route("/api/admin/pilots", get(list_pilots_admin))
        .route("/api/admin/pilots/pending", get(admin_pending))
        .route(
            "/api/admin/checkouts",
            get(list_checkouts_admin).post(admin_checkout),
        )
        .route("/api/admin/pilots/{id}/approve", post(admin_approve))
}
