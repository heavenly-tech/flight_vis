use crate::club::{
    auth::{self, set_session_cookie},
    db::{now_ms, GuestClaimError, MagicConsumeError, PilotRow},
    webauthn::WebauthnRouteError,
};
use crate::{AppState, ClientIp};
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::post,
    Router,
};
use serde::Deserialize;
use serde_json::json;
use webauthn_rs::prelude::{PublicKeyCredential, RegisterPublicKeyCredential};

use super::routes::{club_db, is_secure};

#[derive(Deserialize)]
pub(crate) struct MagicLinkBody {
    email: String,
    #[serde(default)]
    locale: Option<String>,
}

pub async fn post_magic_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    client_ip: ClientIp,
    Json(body): Json<MagicLinkBody>,
) -> impl IntoResponse {
    let email = body.email.trim();
    let now = now_ms();
    let ip = client_ip.0;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "ok": true })).into_response();
    };
    let club_id = db.default_club_id().await.unwrap_or_default();
    if db.magic_send_allowed(email, &ip, now).await && state.mailer.allow_send(email, &ip) {
        if let Ok(Some(token)) = db.issue_magic_login(&club_id, email, now).await {
            let _ = db.log_magic_send(email, &ip, now).await;
            let url = format!(
                "{}/api/club/auth/magic?token={token}",
                state.mailer.public_url.trim_end_matches('/')
            );
            let loc = body
                .locale
                .as_deref()
                .map(str::to_string)
                .or_else(|| auth::parse_display_cookie(&headers))
                .unwrap_or_else(|| "es".to_string());
            state
                .mailer
                .send_localized(
                    email,
                    &loc,
                    "magicLink",
                    &[("minutes", "15"), ("url", &url)],
                )
                .await;
        }
    }
    Json(json!({ "ok": true })).into_response()
}

#[derive(Deserialize)]
pub(crate) struct MagicQuery {
    token: String,
}

pub async fn get_magic_consume(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<MagicQuery>,
) -> impl IntoResponse {
    if wants_html(&headers) {
        let token = html_escape(&q.token);
        return Html(format!(
            r#"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><title>Sign in</title></head>
<body><h1>Sign in</h1><form method="post" action="/api/club/auth/magic">
<input type="hidden" name="token" value="{token}">
<button type="submit">Continue</button></form></body></html>"#
        ))
        .into_response();
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let now = now_ms();
    if db.try_consume_magic_verify(&q.token, now).await.is_ok() {
        return Json(json!({ "ok": true, "verified": true })).into_response();
    }
    let club_id = db.default_club_id().await.unwrap_or_default();
    match db.consume_magic_login(&q.token, &club_id, now).await {
        Ok((session_id, pilot)) => {
            let cookie = set_session_cookie(&session_id, is_secure(&headers));
            if wants_html(&headers) {
                return (
                    StatusCode::FOUND,
                    [
                        (header::SET_COOKIE, cookie),
                        (header::LOCATION, "/".to_string()),
                    ],
                )
                    .into_response();
            }
            pilot_ok_response(&cookie, &pilot)
        }
        Err(MagicConsumeError::Expired) => {
            (StatusCode::GONE, Json(json!({ "error": "expired" }))).into_response()
        }
        Err(_) => (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid" }))).into_response(),
    }
}

#[derive(Deserialize)]
struct MagicConsumeForm {
    token: String,
}

async fn post_magic_consume_form(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Form(body): axum::Form<MagicConsumeForm>,
) -> impl IntoResponse {
    magic_consume_post(state, headers, body.token).await
}

async fn post_magic_consume_json(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<MagicQuery>,
) -> impl IntoResponse {
    magic_consume_post(state, headers, body.token).await
}

async fn magic_consume_post(
    state: AppState,
    headers: HeaderMap,
    token: String,
) -> axum::response::Response {
    if auth::parse_session_cookie(&headers).is_some() {
        if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
            return (status, Json(json!({ "error": "csrf" }))).into_response();
        }
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let now = now_ms();
    if db.try_consume_magic_verify(&token, now).await.is_ok() {
        return Json(json!({ "ok": true, "verified": true })).into_response();
    }
    let club_id = db.default_club_id().await.unwrap_or_default();
    match db.consume_magic_login(&token, &club_id, now).await {
        Ok((session_id, pilot)) => {
            let cookie = set_session_cookie(&session_id, is_secure(&headers));
            pilot_ok_response(&cookie, &pilot)
        }
        Err(MagicConsumeError::Expired) => {
            (StatusCode::GONE, Json(json!({ "error": "expired" }))).into_response()
        }
        Err(_) => (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid" }))).into_response(),
    }
}

fn html_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn wants_html(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|a| a.contains("text/html"))
}

fn pilot_ok_response(cookie: &str, pilot: &PilotRow) -> axum::response::Response {
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie.to_string())],
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

pub async fn webauthn_register_begin(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Some(webauthn) = &state.webauthn else {
        return webauthn_disabled();
    };
    let Some(db) = club_db(&state) else {
        return club_disabled();
    };
    let now = now_ms();
    let session = auth::parse_session_cookie(&headers);
    let (_viewer, pilot) = db.viewer_with_pilot(session.as_deref(), now).await;
    let Some(pilot) = pilot else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    if pilot.status != "approved" {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "approved pilot required" })),
        )
            .into_response();
    }
    match webauthn.register_begin(&pilot) {
        Ok((public_key, nonce)) => {
            Json(json!({ "publicKey": public_key, "nonce": nonce })).into_response()
        }
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "webauthn begin failed" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct WebauthnFinishBody {
    nonce: String,
    credential: RegisterPublicKeyCredential,
}

pub async fn webauthn_register_finish(
    State(state): State<AppState>,
    Json(body): Json<WebauthnFinishBody>,
) -> impl IntoResponse {
    let Some(webauthn) = &state.webauthn else {
        return webauthn_disabled();
    };
    let Some(db) = club_db(&state) else {
        return club_disabled();
    };
    match webauthn
        .register_finish(db, &body.nonce, body.credential)
        .await
    {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(WebauthnRouteError::InvalidSession) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid nonce" })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "webauthn failed" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct WebauthnLoginBeginBody {
    email: String,
}

pub async fn webauthn_login_begin(
    State(state): State<AppState>,
    Json(body): Json<WebauthnLoginBeginBody>,
) -> impl IntoResponse {
    let Some(webauthn) = &state.webauthn else {
        return webauthn_disabled();
    };
    let Some(db) = club_db(&state) else {
        return club_disabled();
    };
    let club_id = db.default_club_id().await.unwrap_or_default();
    match webauthn.login_begin(db, &body.email, &club_id).await {
        Ok((public_key, nonce)) => {
            Json(json!({ "publicKey": public_key, "nonce": nonce })).into_response()
        }
        Err(WebauthnRouteError::NotFound) => Json(json!({ "ok": true })).into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "webauthn begin failed" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct WebauthnLoginFinishBody {
    nonce: String,
    credential: PublicKeyCredential,
}

pub async fn webauthn_login_finish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<WebauthnLoginFinishBody>,
) -> impl IntoResponse {
    let Some(webauthn) = &state.webauthn else {
        return webauthn_disabled();
    };
    let Some(db) = club_db(&state) else {
        return club_disabled();
    };
    let club_id = db.default_club_id().await.unwrap_or_default();
    match webauthn
        .login_finish(db, &club_id, &body.nonce, body.credential)
        .await
    {
        Ok((session_id, pilot)) => {
            let cookie = set_session_cookie(&session_id, is_secure(&headers));
            pilot_ok_response(&cookie, &pilot)
        }
        Err(WebauthnRouteError::InvalidSession) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid nonce" })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "webauthn failed" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct GuestBody {
    #[serde(default)]
    name: Option<String>,
}

pub async fn post_guest(
    State(state): State<AppState>,
    headers: HeaderMap,
    client_ip: ClientIp,
    body: Option<Json<GuestBody>>,
) -> impl IntoResponse {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if !state
        .device_register_throttle
        .allow_up_to(&client_ip.0, now_secs, 30)
    {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "rate limited" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return club_disabled();
    };
    let club_id = db.default_club_id().await.unwrap_or_default();
    let now = now_ms();
    let name = body
        .as_ref()
        .and_then(|b| b.name.as_deref())
        .unwrap_or("Guest");
    match db.create_guest_session(&club_id, name, now).await {
        Ok((session_id, _guest_sess, pilot)) => {
            let cookie = set_session_cookie(&session_id, is_secure(&headers));
            pilot_ok_response(&cookie, &pilot)
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "guest failed" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub(crate) struct ClaimBody {
    name: String,
    email: String,
    #[serde(default)]
    license_no: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

pub async fn post_claim(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ClaimBody>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return club_disabled();
    };
    let now = now_ms();
    let session = auth::parse_session_cookie(&headers);
    let Some(session_id) = session else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(pilot) = db.pilot_from_session(&session_id, now).await else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(guest_session_id) = db.guest_session_for_pilot(&pilot.id, now).await else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "not a guest session" })),
        )
            .into_response();
    };
    let license = body.license_no.as_deref().unwrap_or("");
    match db
        .claim_guest(
            &guest_session_id,
            &body.name,
            &body.email,
            license,
            body.password.as_deref(),
            now,
        )
        .await
    {
        Ok(pilot) => {
            if let Ok(token) = db
                .issue_magic_email_verify(&pilot.id, &pilot.email, now)
                .await
            {
                let url = format!(
                    "{}/api/club/auth/magic?token={token}",
                    state.mailer.public_url.trim_end_matches('/')
                );
                state
                    .mailer
                    .send_localized(
                        &pilot.email,
                        "es",
                        "magicLink",
                        &[("minutes", "15"), ("url", &url)],
                    )
                    .await;
            }
            Json(json!({
                "ok": true,
                "emailVerificationSent": true,
                "pilot": {
                    "id": pilot.id,
                    "name": pilot.name,
                    "email": pilot.email,
                    "status": pilot.status,
                    "role": pilot.role,
                }
            }))
            .into_response()
        }
        Err(GuestClaimError::EmailExists) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "email exists" })),
        )
            .into_response(),
        Err(GuestClaimError::WeakPassword) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "weak password" })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "claim failed" })),
        )
            .into_response(),
    }
}

fn club_disabled() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "club disabled" })),
    )
        .into_response()
}

fn webauthn_disabled() -> axum::response::Response {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(json!({ "error": "webauthn disabled" })),
    )
        .into_response()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/club/auth/magic-link", post(post_magic_link))
        .route(
            "/api/club/auth/magic",
            axum::routing::get(get_magic_consume).post(post_magic_consume_form),
        )
        .route(
            "/api/club/auth/magic/consume",
            post(post_magic_consume_json),
        )
        .route(
            "/api/club/auth/webauthn/register/begin",
            post(webauthn_register_begin),
        )
        .route(
            "/api/club/auth/webauthn/register/finish",
            post(webauthn_register_finish),
        )
        .route(
            "/api/club/auth/webauthn/login/begin",
            post(webauthn_login_begin),
        )
        .route(
            "/api/club/auth/webauthn/login/finish",
            post(webauthn_login_finish),
        )
        .route("/api/club/auth/guest", post(post_guest))
        .route("/api/club/auth/claim", post(post_claim))
}
