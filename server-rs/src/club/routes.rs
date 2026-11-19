use crate::club::{
    access_request::AccessRequestError,
    auth::{self, parse_display_cookie, parse_session_cookie},
    db::{
        now_ms, AdminPendingCounts, ApprovePilotCheckout, CheckinReject, CheckoutError, ClubDb,
        DeleteError, DeviceRegisterError, EndError, PatchError, RegisterError, StartFlightError,
        MAX_DEVICES_PER_PILOT,
    },
    map::TailLivePosition,
    visibility::{FlightVisibility, PilotStatus, Viewer},
};
use crate::{AppState, ClientIp};
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
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

#[allow(unused_imports)]
use axum::routing::patch;
use tokio_stream::StreamExt as _;

pub(crate) fn club_db(state: &AppState) -> Option<&ClubDb> {
    state.club.as_ref()
}

pub(crate) fn is_secure(headers: &HeaderMap) -> bool {
    let forwarded = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok());
    crate::auth::is_secure_request(forwarded)
}

pub(crate) async fn viewer(state: &AppState, headers: &HeaderMap) -> Viewer {
    let now = now_ms();
    if let Some(db) = club_db(state) {
        if let Some(acct_sess) = auth::parse_account_session_cookie(headers) {
            if let Some((_, _active)) = db.account_from_session(&acct_sess, now).await {
                if let Some((pilot_sess, _pilot)) =
                    db.pilot_session_for_account(&acct_sess, now).await
                {
                    return db.viewer_from_session(Some(&pilot_sess), now).await;
                }
            }
        }
    }
    let session = parse_session_cookie(headers);
    match state.club.as_ref() {
        Some(db) => db.viewer_from_session(session.as_deref(), now).await,
        None => Viewer::Anonymous,
    }
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

async fn require_club_admin_scope(
    db: &ClubDb,
    viewer: &Viewer,
) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    require_approved(viewer)?;
    let Viewer::Pilot { id, .. } = viewer else {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        ));
    };
    let club_id = db.pilot_club_id(id).await.map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
    })?;
    if db.require_club_admin(id, &club_id).await.is_err() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        ));
    }
    Ok(club_id)
}

async fn admin_may_act_on_pilot(
    db: &ClubDb,
    admin_id: &str,
    target_pilot_id: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let admin_club = db.pilot_club_id(admin_id).await.map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
    })?;
    if db.require_club_admin(admin_id, &admin_club).await.is_err() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        ));
    }
    let target_club = db
        .pilot_club_id(target_pilot_id)
        .await
        .map_err(|_| (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))))?;
    if target_club != admin_club {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))));
    }
    Ok(())
}

#[derive(Deserialize)]
struct RegisterBody {
    name: String,
    email: String,
    password: String,
    #[serde(default)]
    license_no: String,
    #[serde(default)]
    slug: Option<String>,
    #[serde(default)]
    website: Option<String>,
}

async fn register(
    State(state): State<AppState>,
    client_ip: ClientIp,
    Json(body): Json<RegisterBody>,
) -> impl IntoResponse {
    if body
        .website
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "register failed" })),
        )
            .into_response();
    }
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if !state
        .device_register_throttle
        .allow_up_to(&client_ip.0, now_secs, 20)
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "rate limited" })),
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
    let club_id = if let Some(slug) = body.slug.as_deref().filter(|s| !s.is_empty()) {
        let Some(row) = db.club_public_row_by_slug(slug).await else {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "register failed" })),
            )
                .into_response();
        };
        match ClubDb::register_gate(&row) {
            crate::club::db_club_lifecycle::ClubRegisterGate::Suspended => {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": "club suspended" })),
                )
                    .into_response();
            }
            crate::club::db_club_lifecycle::ClubRegisterGate::Closed => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "register failed" })),
                )
                    .into_response();
            }
            crate::club::db_club_lifecycle::ClubRegisterGate::Open => row.id,
        }
    } else {
        match db.default_club_id().await {
            Ok(id) => {
                if let Some(row) = db.club_public_row(&id).await {
                    if !ClubDb::is_public_verified(&row) {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({ "error": "register failed" })),
                        )
                            .into_response();
                    }
                    if row.suspended_at.is_some() {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(json!({ "error": "club suspended" })),
                        )
                            .into_response();
                    }
                }
                id
            }
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "register failed" })),
                )
                    .into_response();
            }
        }
    };
    let now = now_ms();
    match db
        .register(
            &club_id,
            body.name.trim(),
            body.email.trim(),
            body.license_no.trim(),
            &body.password,
        )
        .await
    {
        Ok(id) => {
            if let Ok(token) = db
                .issue_magic_email_verify(&id, body.email.trim(), now)
                .await
            {
                let url = format!(
                    "{}/api/club/auth/magic?token={token}",
                    state.mailer.public_url.trim_end_matches('/')
                );
                state
                    .mailer
                    .send_localized(
                        body.email.trim(),
                        "es",
                        "magicLink",
                        &[("minutes", "15"), ("url", &url)],
                    )
                    .await;
            }
            (
                StatusCode::CREATED,
                Json(json!({ "ok": true, "pilotId": id, "emailVerificationSent": true })),
            )
                .into_response()
        }
        Err(RegisterError::EmailExists) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "register failed" })),
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
    #[serde(rename = "installId", default)]
    install_id: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

async fn club_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    client_ip: ClientIp,
    Json(body): Json<LoginBody>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let club_id = db.default_club_id().await.unwrap_or_default();
    let now = now_ms();
    match db
        .login_password(
            &club_id,
            body.email.trim(),
            &body.password,
            now,
            &client_ip.0,
        )
        .await
    {
        Ok((session_id, pilot)) => {
            if let Some(install_id) = body.install_id.as_deref().filter(|s| !s.is_empty()) {
                let label = body
                    .label
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("Dispositivo");
                let _ = db
                    .register_device_for_install(
                        &pilot.id,
                        label,
                        body.model.as_deref(),
                        install_id,
                    )
                    .await;
            }
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
        Err(crate::club::db::LoginError::Throttled) => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "too many attempts" })),
        )
            .into_response(),
        Err(crate::club::db::LoginError::ClubUnavailable) => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "club unavailable" })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
struct LogoutBody {
    #[serde(rename = "installId", default)]
    install_id: Option<String>,
}

async fn club_logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<LogoutBody>>,
) -> impl IntoResponse {
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return (status, Json(json!({ "error": "csrf" }))).into_response();
    }
    if let (Some(db), Some(session)) = (club_db(&state), parse_session_cookie(&headers)) {
        let now = now_ms();
        if let Some(pilot) = db.pilot_from_session(&session, now).await {
            if let Some(Json(body)) = body {
                if let Some(install_id) = body.install_id.as_deref().filter(|s| !s.is_empty()) {
                    let _ = db.revoke_device_by_install(&pilot.id, install_id).await;
                }
            }
        }
        let _ = db.logout(&session).await;
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
    let (_viewer, pilot) = db.viewer_with_pilot(session.as_deref(), now).await;
    match pilot {
        Some(p) => {
            if db.club_operational(&p.club_id).await.is_err() {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": "club unavailable" })),
                )
                    .into_response();
            }
            let club_default = db
                .club_default_locale(&p.club_id)
                .await
                .unwrap_or_else(|_| "es".to_string());
            Json(json!({
                "pilot": {
                    "id": p.id,
                    "name": p.name,
                    "email": p.email,
                    "status": p.status,
                    "role": p.role,
                    "licenseNo": p.license_no,
                    "locale": p.locale,
                },
                "clubDefaultLocale": club_default,
            }))
        }
        .into_response(),
        None => Json(json!({ "pilot": null })).into_response(),
    }
}

#[derive(serde::Deserialize)]
struct LocaleBody {
    locale: String,
}

fn accept_locale(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok())
}

async fn club_i18n_context(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let accept = accept_locale(&headers);
    let cat = crate::i18n::require_global();
    let available_locales: Vec<_> = cat
        .locale_labels()
        .map(|(id, label, bcp47)| json!({ "id": id, "label": label, "bcp47": bcp47 }))
        .collect();
    let Some(db) = club_db(&state) else {
        let resolved = cat.resolve_locale(None, None, accept);
        return Json(json!({
            "userLocale": null,
            "clubDefaultLocale": null,
            "resolvedLocale": resolved,
            "availableLocales": available_locales,
        }))
        .into_response();
    };
    let session = parse_session_cookie(&headers);
    let now = now_ms();
    let (_viewer, pilot) = db.viewer_with_pilot(session.as_deref(), now).await;
    let (user_locale, club_default) = match pilot {
        Some(p) => (
            Some(p.locale),
            db.club_default_locale(&p.club_id)
                .await
                .ok()
                .or(Some("es".to_string())),
        ),
        None => (None, None),
    };
    let resolved = cat.resolve_locale(user_locale.as_deref(), club_default.as_deref(), accept);
    Json(json!({
        "userLocale": user_locale,
        "clubDefaultLocale": club_default,
        "resolvedLocale": resolved,
        "availableLocales": available_locales,
    }))
    .into_response()
}

async fn patch_me_locale(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LocaleBody>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let session = parse_session_cookie(&headers);
    let Some(session) = session else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let now = now_ms();
    let Some(pilot) = db.pilot_from_session(&session, now).await else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    match db.set_pilot_locale(&pilot.id, &body.locale).await {
        Ok(()) => Json(json!({ "locale": body.locale })).into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid locale" })),
        )
            .into_response(),
    }
}

async fn patch_admin_club_locale(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<LocaleBody>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let v = viewer(&state, &headers).await;
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let club_id = db.default_club_id().await.unwrap_or_default();
    if db.require_club_admin(&id, &club_id).await.is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    match db.set_club_default_locale(&club_id, &body.locale).await {
        Ok(()) => Json(json!({ "locale": body.locale })).into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid locale" })),
        )
            .into_response(),
    }
}

async fn me_tails(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
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
    match db.me_tails(&id, now_ms()).await {
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
    let v = viewer(&state, &headers).await;
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
    let device_id = match db.resolve_device_token(&token).await {
        Some(id) => id,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "invalid device token" })),
            )
                .into_response();
        }
    };
    if let Some(club_id) = db.club_id_for_aircraft_tail(parsed.tail.trim()).await {
        if db.club_operational(&club_id).await.is_err() {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "club unavailable" })),
            )
                .into_response();
        }
    }
    let visibility = match parse_visibility(parsed.visibility.as_deref()) {
        Ok(v) => v,
        Err(msg) => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response();
        }
    };
    let show_name = parsed.show_pilot_name_public.unwrap_or(false);
    match db
        .start_flight(
            &pilot_id,
            &device_id,
            parsed.tail.trim(),
            visibility,
            show_name,
            now_ms(),
        )
        .await
    {
        Ok(flight_id) => Json(json!({
            "id": flight_id,
            "visibility": visibility.as_str(),
            "showPilotNamePublic": show_name,
        }))
        .into_response(),
        Err(StartFlightError::NotCleared { tail, type_code }) => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": format!("not cleared for {tail} / {type_code}") })),
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
        Err(StartFlightError::DeviceMismatch) => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "device mismatch" })),
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
    let v = viewer(&state, &headers).await;
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
    match db
        .patch_flight(&id, &flight_id, visibility, body.show_pilot_name_public)
        .await
    {
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
    let v = viewer(&state, &headers).await;
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
    match db.end_flight(&id, &flight_id, now_ms()).await {
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
    let v = viewer(&state, &headers).await;
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
    if let Viewer::Pilot { id, .. } = &v {
        if let Ok(club_id) = db.pilot_club_id(id).await {
            if db.club_operational(&club_id).await.is_err() {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({ "error": "club unavailable" })),
                )
                    .into_response();
            }
        }
    }
    let positions: Vec<TailLivePosition> = db.club_map_for_viewer(&v).await;
    Json(json!({ "aircraft": positions })).into_response()
}

#[derive(Deserialize)]
struct PublicMapQuery {
    #[serde(default)]
    club: Option<String>,
}

async fn resolve_public_map_club(
    db: &ClubDb,
    slug: Option<&str>,
) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    let club_id = if let Some(slug) = slug.filter(|s| !s.is_empty()) {
        let Some(row) = db.club_public_row_by_slug(slug).await else {
            return Err((StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))));
        };
        if row.suspended_at.is_some() {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "club suspended" })),
            ));
        }
        if !ClubDb::is_public_verified(&row) {
            return Err((StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))));
        }
        row.id
    } else {
        db.default_club_id()
            .await
            .map_err(|_| (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))))?
    };
    if let Some(row) = db.club_public_row(&club_id).await {
        if row.suspended_at.is_some() {
            return Err((
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "club suspended" })),
            ));
        }
    }
    Ok(club_id)
}

async fn public_map(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<PublicMapQuery>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return Json(json!({ "aircraft": [] })).into_response();
    };
    let club_id = match resolve_public_map_club(db, q.club.as_deref()).await {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };
    let aircraft = db.public_map(&club_id).await;
    Json(json!({ "aircraft": aircraft, "club": q.club })).into_response()
}

async fn admin_map(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "aircraft": [] })).into_response();
    };
    let admin_id = match &v {
        Viewer::Pilot { id, .. } => id.clone(),
        _ => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "admin only" })),
            )
                .into_response();
        }
    };
    if require_club_admin_scope(db, &v).await.is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    }
    let aircraft = db.admin_map(&admin_id).await;
    Json(json!({ "aircraft": aircraft })).into_response()
}

#[derive(Deserialize)]
struct AdminTailsBody {
    tails: Vec<String>,
}

async fn get_admin_map_tails(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "tails": [] })).into_response();
    };
    let Ok(_club_id) = require_club_admin_scope(db, &v).await else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let tails = db.admin_map_tails(&id).await;
    Json(json!({ "tails": tails })).into_response()
}

async fn put_admin_map_tails(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AdminTailsBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let Ok(_club_id) = require_club_admin_scope(db, &v).await else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    match db.set_admin_map_tails(&id, &body.tails).await {
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
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    if db.flight_visible_to(&v, &flight_id).await.is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    }
    let fixes = db.fixes_for_flight(&flight_id).await;
    Json(json!({ "fixes": fixes })).into_response()
}

async fn club_flight_gpx(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(flight_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let Some(flight) = db.flight_visible_to(&v, &flight_id).await else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    };
    let fixes = db.fixes_for_flight(&flight_id).await;
    let gpx = fixes_to_gpx(&flight.tail, &fixes);
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/gpx+xml")],
        gpx,
    )
        .into_response()
}

fn xml_escape_attr(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn fixes_to_gpx(name: &str, fixes: &[crate::live::LiveFix]) -> String {
    let safe_name = xml_escape_attr(name);
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"flight_vis\"><trk><name>{safe_name}</name><trkseg>\n"
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
    client_ip: ClientIp,
    Path(flight_id): Path<String>,
) -> impl IntoResponse {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if !state
        .share_lookup_throttle
        .allow_up_to(&client_ip.0, now_secs, 30)
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "rate limited" })),
        )
            .into_response();
    }
    let Some(sse_guard) = state.sse_limiter.try_admit(&client_ip.0) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "too many streams" })),
        )
            .into_response();
    };
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    if db.flight_visible_to(&v, &flight_id).await.is_none() {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    }
    let db = state.club.clone().unwrap();
    let viewer = v;
    let flight_id_capture = flight_id.clone();
    let backfill: Vec<_> = db.fixes_for_flight(&flight_id).await;
    let idle_ms = super::sse_limit::SSE_IDLE_MS;
    let stream = stream::iter(backfill.into_iter().enumerate().map(move |(i, fix)| {
        Ok::<Event, Infallible>(Event::default().id(i.to_string()).json_data(fix).unwrap())
    }))
    .chain(stream::unfold(
        (
            db,
            flight_id_capture,
            viewer,
            0_i64,
            tokio::time::Instant::now(),
            sse_guard,
        ),
        #[allow(clippy::question_mark)]
        move |(db, fid, viewer, mut last_t, mut idle_since, _sse_guard)| async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if db.flight_visible_to(&viewer, &fid).await.is_none() {
                return None;
            }
            let fresh = db.fixes_for_flight_since(&fid, last_t).await;
            if fresh.is_empty() {
                if idle_since.elapsed() > Duration::from_millis(idle_ms) {
                    return None;
                }
            } else {
                idle_since = tokio::time::Instant::now();
                if let Some(t) = fresh.last().map(|f| f.t) {
                    last_t = t;
                }
            }
            let event = if fresh.is_empty() {
                Event::default().comment("ping")
            } else {
                Event::default()
                    .json_data(json!({ "fixes": fresh }))
                    .unwrap()
            };
            Some((Ok(event), (db, fid, viewer, last_t, idle_since, _sse_guard)))
        },
    ));
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn my_flights(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
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
    Json(json!({ "flights": db.list_my_flights(&id).await })).into_response()
}

async fn delete_my_flight(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(flight_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
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
    match db.delete_my_flight(&id, &flight_id).await {
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
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "types": [] })).into_response();
    };
    let club = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    Json(json!({ "types": db.list_types(&club).await })).into_response()
}

async fn admin_add_type(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TypeBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let club = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    match db.upsert_type(&club, body.code.trim()).await {
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
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "aircraft": [] })).into_response();
    };
    let club = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    Json(json!({ "aircraft": db.list_aircraft(&club).await })).into_response()
}

async fn admin_add_aircraft(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AircraftBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let club = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    match db
        .upsert_aircraft(&club, body.tail.trim(), body.type_code.trim(), body.active)
        .await
    {
        Ok(id) => Json(json!({ "id": id })).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn list_pilots_admin(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "pilots": [] })).into_response();
    };
    let club_id = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    Json(json!({ "pilots": db.list_all_pilots(&club_id).await })).into_response()
}

async fn admin_pending(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "pilots": [] })).into_response();
    };
    let club_id = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    Json(json!({ "pilots": db.list_pending_pilots(&club_id).await })).into_response()
}

#[derive(Deserialize, Default)]
struct ApprovePilotBody {
    #[serde(default)]
    checkout: Option<ApprovePilotCheckout>,
}

async fn admin_approve(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pilot_id): Path<String>,
    Json(body): Json<ApprovePilotBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Viewer::Pilot { id: admin_id, .. } = v else {
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
    if let Err(e) = admin_may_act_on_pilot(db, &admin_id, &pilot_id).await {
        return e.into_response();
    }
    let checkout = body.checkout;
    match db
        .approve_pilot_with_checkout(&admin_id, &pilot_id, checkout)
        .await
    {
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
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "checkouts": [] })).into_response();
    };
    let club_id = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    Json(json!({ "checkouts": db.list_checkouts(&club_id).await })).into_response()
}

async fn admin_checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CheckoutBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let Ok(_club_id) = require_club_admin_scope(db, &v).await else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let Viewer::Pilot { id: admin_id, .. } = v else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    if admin_may_act_on_pilot(db, &admin_id, &body.pilot_id)
        .await
        .is_err()
    {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))).into_response();
    }
    match db
        .add_checkout(
            &body.pilot_id,
            &body.type_code,
            body.tail.as_deref(),
            body.expires_at,
        )
        .await
    {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(CheckoutError::UnknownTail) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unknown tail; type is derived from the aircraft tail" })),
        )
            .into_response(),
        Err(CheckoutError::UnknownType) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unknown aircraft type" })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct AccessRequestBody {
    tail: String,
    #[serde(rename = "typeId")]
    type_id: Option<String>,
    #[serde(rename = "newTypeName")]
    new_type_name: Option<String>,
}

async fn me_access_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return Json(json!({ "requests": [] })).into_response();
    };
    Json(json!({ "requests": db.list_pilot_access_requests(&id).await })).into_response()
}

async fn me_access_requests_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state).cloned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let pilot_id = id;
    let stream = stream::unfold(0u64, move |_| {
        let db = db.clone();
        let pid = pilot_id.clone();
        async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let rows = db.list_pilot_access_requests(&pid).await;
            let payload = serde_json::to_string(&json!({ "requests": rows })).ok()?;
            Some((Ok::<_, Infallible>(Event::default().data(payload)), 0u64))
        }
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(2)))
        .into_response()
}

async fn post_access_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<AccessRequestBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
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
    match db
        .create_access_request(
            &id,
            &body.tail,
            body.type_id.as_deref(),
            body.new_type_name.as_deref(),
            now_ms(),
        )
        .await
    {
        Ok(req_id) => Json(json!({ "ok": true, "id": req_id })).into_response(),
        Err(err) => {
            let status = match err {
                AccessRequestError::NoCheckin => StatusCode::FORBIDDEN,
                AccessRequestError::InvalidTail | AccessRequestError::MissingType => {
                    StatusCode::BAD_REQUEST
                }
                _ => StatusCode::CONFLICT,
            };
            (status, Json(json!({ "error": err.message() }))).into_response()
        }
    }
}

async fn admin_pending_counts(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "pilots": 0, "accessRequests": 0 })).into_response();
    };
    let club_id = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let AdminPendingCounts {
        pilots,
        access_requests,
    } = db.admin_pending_counts(&club_id).await;
    Json(json!({ "pilots": pilots, "accessRequests": access_requests })).into_response()
}

async fn admin_list_access_requests(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<AccessRequestQuery>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "requests": [] })).into_response();
    };
    let club_id = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let requests = db
        .list_admin_access_requests(&club_id, q.status.as_deref())
        .await;
    Json(json!({ "requests": requests })).into_response()
}

#[derive(Deserialize, Default)]
struct AccessRequestQuery {
    status: Option<String>,
}

#[derive(Deserialize, Default)]
struct DecideAccessBody {
    #[serde(default)]
    expires_at: Option<i64>,
    #[serde(default)]
    reason: Option<String>,
}

async fn admin_approve_access(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Option<Json<DecideAccessBody>>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let Ok(_club_id) = require_club_admin_scope(db, &v).await else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let Viewer::Pilot { id: admin_id, .. } = v else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let expires = body.as_ref().and_then(|Json(b)| b.expires_at);
    match db
        .approve_access_request(&admin_id, &id, expires, now_ms())
        .await
    {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": err.message() })),
        )
            .into_response(),
    }
}

async fn admin_reject_access(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<DecideAccessBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let Ok(_club_id) = require_club_admin_scope(db, &v).await else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let Viewer::Pilot { id: admin_id, .. } = v else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let reason = body.reason.unwrap_or_else(|| "rejected".to_string());
    match db
        .reject_access_request(&admin_id, &id, &reason, now_ms())
        .await
    {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": err.message() })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct DeviceBody {
    #[serde(default)]
    label: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(rename = "installId", default)]
    install_id: Option<String>,
}

async fn create_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    client_ip: ClientIp,
    Json(body): Json<DeviceBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
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
    if !state
        .device_register_throttle
        .allow(&client_ip.0, crate::now_secs())
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "too many attempts" })),
        )
            .into_response();
    }
    if let Some(install_id) = body
        .install_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let label = body.label.trim();
        match db
            .register_device_for_install(&id, label, body.model.as_deref(), install_id)
            .await
        {
            Ok(reg) => return Json(reg).into_response(),
            Err(DeviceRegisterError::CapExceeded) => {
                return (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": format!("device cap ({MAX_DEVICES_PER_PILOT}) reached")
                    })),
                )
                    .into_response();
            }
            Err(DeviceRegisterError::InvalidInstallId) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "invalid installId" })),
                )
                    .into_response();
            }
            Err(DeviceRegisterError::Db) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "db error" })),
                )
                    .into_response();
            }
        }
    }
    match db.create_device(&id, body.label.trim()).await {
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

async fn delete_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(install_or_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
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
    let revoked = db
        .revoke_device_by_install(&id, &install_or_id)
        .await
        .unwrap_or(false)
        || db
            .revoke_device_by_id(&id, &install_or_id)
            .await
            .unwrap_or(false);
    if revoked {
        Json(json!({ "ok": true })).into_response()
    } else {
        (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
    }
}

async fn list_devices(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
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
    Json(json!({ "devices": db.list_my_devices(&id).await })).into_response()
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

async fn display_context(headers: &HeaderMap, db: &ClubDb) -> Option<(String, String)> {
    let raw = parse_display_cookie(headers)?;
    db.resolve_display_session(&raw)
        .await
        .map(|(_id, club_id, admin_id)| (club_id, admin_id))
}

type DisplayClubErr = (StatusCode, Json<serde_json::Value>);

async fn require_display_club(
    db: &ClubDb,
    headers: &HeaderMap,
) -> Result<(String, String), DisplayClubErr> {
    let Some((club_id, admin_id)) = display_context(headers, db).await else {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "display session required" })),
        ));
    };
    if db.club_operational(&club_id).await.is_err() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "club unavailable" })),
        ));
    }
    Ok((club_id, admin_id))
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
        if let Some(dev) = db.resolve_device_token(&bearer).await {
            device_id = Some(dev.clone());
            pilot_id = db.device_pilot_id(&dev).await;
        }
    }
    if pilot_id.is_none() {
        let v = viewer(&state, &headers).await;
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
    let token = body.token.trim();
    if let Some(target_club) = crate::club::checkin::parse_checkin_token_club_id(token) {
        if db.club_operational(&target_club).await.is_err() {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "club unavailable" })),
            )
                .into_response();
        }
    }
    if let Some(dev) = &device_id {
        if db.device_pilot_id(dev).await.as_deref() != Some(pilot_id.as_str()) {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "device mismatch" })),
            )
                .into_response();
        }
    }
    match db
        .submit_checkin(&pilot_id, device_id.as_deref(), token, now)
        .await
    {
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
    let Some(_ctx) = db.resolve_display_session(token).await else {
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
    let (club_id, _admin) = match require_display_club(db, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let now = now_ms();
    match db.mint_checkin_qr_token(&club_id, now).await {
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
    let (club_id, admin_id) = match require_display_club(db, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let rows = db.display_map(&club_id, &admin_id).await;
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
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let club_id = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let screen_url = db
        .club_slug(&club_id)
        .await
        .map(|slug| format!("/c/{slug}/screen"))
        .unwrap_or_else(|| "/club/screen".to_string());
    match db.create_display_session(&id, body.label.trim()).await {
        Ok((session_id, token)) => Json(json!({
            "id": session_id,
            "token": token,
            "screenUrl": screen_url,
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
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "sessions": [] })).into_response();
    };
    let _club_id = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    Json(json!({ "sessions": db.list_display_sessions(&id).await })).into_response()
}

async fn admin_revoke_display(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let _club_id = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    match db.revoke_display_session(&id, &session_id).await {
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
        Ok(html) => ([(html_csp_header())], Html(html)).into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            "Club screen page not built (npm run build)",
        )
            .into_response(),
    }
}

async fn club_slug_screen_page(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (StatusCode::SERVICE_UNAVAILABLE, "Club disabled").into_response();
    };
    if db.club_id_for_slug(&slug).await.is_none() {
        return (StatusCode::NOT_FOUND, "Club not found").into_response();
    }
    let path = state.dist_dir.join("club-screen.html");
    match tokio::fs::read_to_string(&path).await {
        Ok(html) => ([(html_csp_header())], Html(html)).into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            "Club screen page not built (npm run build)",
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
struct KioskLayoutBody {
    x: f64,
    y: f64,
    size: f64,
    #[serde(default)]
    aspect: Option<f64>,
}

fn layout_aspect(body: &KioskLayoutBody) -> f64 {
    body.aspect
        .filter(|a| *a > 0.05 && *a < 8.0)
        .unwrap_or(crate::club::kiosk_qr::DEFAULT_PREVIEW_ASPECT)
}

async fn admin_get_kiosk_layout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let club_id = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let _ = db.ensure_kiosk_qr_defaults(&club_id).await;
    match db.kiosk_qr_layout(&club_id).await {
        Ok(row) => Json(row).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn admin_patch_kiosk_layout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<KioskLayoutBody>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let club_id = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    if !body.size.is_finite() || !body.x.is_finite() || !body.y.is_finite() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid layout" })),
        )
            .into_response();
    }
    let aspect = layout_aspect(&body);
    match db
        .set_kiosk_qr_layout(&club_id, body.x, body.y, body.size, aspect)
        .await
    {
        Ok(row) => Json(row).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn admin_reset_kiosk_layout(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<KioskLayoutBody>>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let club_id = match require_club_admin_scope(db, &v).await {
        Ok(c) => c,
        Err(e) => return e.into_response(),
    };
    let aspect = body
        .as_ref()
        .map(|b| layout_aspect(b))
        .unwrap_or(crate::club::kiosk_qr::DEFAULT_PREVIEW_ASPECT);
    match db.reset_kiosk_qr_layout(&club_id, aspect).await {
        Ok(row) => Json(row).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn display_kiosk_layout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let (club_id, _admin) = match require_display_club(db, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let _ = db.ensure_kiosk_qr_defaults(&club_id).await;
    match db.kiosk_qr_layout(&club_id).await {
        Ok(row) => Json(row).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn display_kiosk_layout_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(db) = club_db(&state).cloned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let (club_id, _admin) = match require_display_club(&db, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let _ = db.ensure_kiosk_qr_defaults(&club_id).await;
    let club_id2 = club_id.clone();
    let stream = stream::unfold(
        (db, club_id2, 0i64),
        move |(db, club_id, mut last_rev)| async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if db.club_operational(&club_id).await.is_err() {
                return None;
            }
            let layout = match db.kiosk_qr_layout(&club_id).await {
                Ok(l) => l,
                Err(_) => return None,
            };
            if layout.rev == last_rev {
                return Some((
                    Ok::<_, Infallible>(Event::default().comment("ping")),
                    (db, club_id, last_rev),
                ));
            }
            last_rev = layout.rev;
            let payload = match serde_json::to_string(&layout) {
                Ok(p) => p,
                Err(_) => return None,
            };
            Some((
                Ok(Event::default().event("layout").data(payload)),
                (db, club_id, last_rev),
            ))
        },
    );
    Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}

async fn club_screen_qr_svg(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let (club_id, _admin) = match require_display_club(db, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let now = now_ms();
    let token = match db.mint_checkin_qr_token(&club_id, now).await {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let slug = db
        .club_slug(&club_id)
        .await
        .unwrap_or_else(|| "default".to_string());
    let path = crate::club::club_scope::checkin_qr_path(&slug, &token);
    let payload = crate::club::android_assetlinks::absolute_https_url(&headers, &path);
    let svg = match crate::club::qr::svg_for_payload(&payload, 280) {
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
    let v = viewer(&state, &headers).await;
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
    let Some(token) = db.device_pair_token_for_pilot(&device_id, &id).await else {
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

async fn postal_bounce_webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let sig = headers
        .get("x-postal-signature-256")
        .or_else(|| headers.get("X-Postal-Signature-256"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if sig.is_empty() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "missing signature" })),
        )
            .into_response();
    }
    if !state.mailer.verify_postal_signature(&body, sig) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid signature" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return (StatusCode::OK, Json(json!({ "ok": true }))).into_response();
    };
    let payload: serde_json::Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let email = payload
        .get("to")
        .or_else(|| payload.get("email"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !email.is_empty() {
        let _ = db.suppress_email(email, "postal_bounce").await;
    }
    Json(json!({ "ok": true })).into_response()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(crate::club::auth_routes::router())
        .merge(crate::club::account_routes::routes())
        .route("/api/webhooks/postal/bounce", post(postal_bounce_webhook))
        .route("/club/screen", get(club_screen_page))
        .route("/c/{slug}/screen", get(club_slug_screen_page))
        .route("/api/checkin", post(post_checkin))
        .route("/api/display/activate", post(display_activate))
        .route("/api/display/checkin-qr", get(display_checkin_qr))
        .route("/api/club/screen/qr.svg", get(club_screen_qr_svg))
        .route("/api/devices/pair/{id}/qr.svg", get(device_pair_qr_svg))
        .route("/api/display/map", get(display_map))
        .route("/api/display/kiosk-layout", get(display_kiosk_layout))
        .route(
            "/api/display/kiosk-layout/stream",
            get(display_kiosk_layout_stream),
        )
        .route(
            "/api/admin/kiosk-layout",
            get(admin_get_kiosk_layout).patch(admin_patch_kiosk_layout),
        )
        .route(
            "/api/admin/kiosk-layout/reset",
            post(admin_reset_kiosk_layout),
        )
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
        .route("/api/club/logout", post(club_logout))
        .route("/api/club/me", get(club_me))
        .route("/api/club/me/locale", axum::routing::patch(patch_me_locale))
        .route("/api/club/i18n/context", get(club_i18n_context))
        .route(
            "/api/admin/club/locale",
            axum::routing::patch(patch_admin_club_locale),
        )
        .route("/api/me/tails", get(me_tails))
        .route("/api/me/flights", get(my_flights))
        .route(
            "/api/me/flights/{id}",
            axum::routing::delete(delete_my_flight),
        )
        .route("/api/me/devices", get(list_devices).post(create_device))
        .route("/api/me/devices/{id}", axum::routing::delete(delete_device))
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
        .route(
            "/api/me/access-requests",
            get(me_access_requests).post(post_access_request),
        )
        .route(
            "/api/me/access-requests/stream",
            get(me_access_requests_stream),
        )
        .route("/api/admin/pending-counts", get(admin_pending_counts))
        .route(
            "/api/admin/access-requests",
            get(admin_list_access_requests),
        )
        .route(
            "/api/admin/access-requests/{id}/approve",
            post(admin_approve_access),
        )
        .route(
            "/api/admin/access-requests/{id}/reject",
            post(admin_reject_access),
        )
        .merge(crate::club::android_assetlinks::well_known_routes())
        .merge(crate::club::checkin_landing::routes())
        .merge(crate::club::signup_routes::routes())
        .merge(crate::club::superadmin_routes::routes())
        .merge(crate::club::share_routes::routes())
}
