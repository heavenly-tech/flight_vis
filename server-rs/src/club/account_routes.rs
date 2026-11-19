use crate::club::{
    auth,
    db::{now_ms, ClubDb},
    db_account::{AccountJoinError, AccountSignupError},
};
use crate::AppState;
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;

fn club_db(state: &AppState) -> Option<&ClubDb> {
    state.club.as_ref()
}

fn is_secure(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|p| p.eq_ignore_ascii_case("https"))
}

async fn signup_available() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "ok": true, "signup": true })))
}

#[derive(Deserialize)]
struct AccountSignupBody {
    email: String,
    #[serde(default)]
    password: Option<String>,
}

async fn account_signup(
    State(state): State<AppState>,
    headers: HeaderMap,
    client_ip: crate::ClientIp,
    Json(body): Json<AccountSignupBody>,
) -> impl IntoResponse {
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return (status, Json(json!({ "error": "csrf" }))).into_response();
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
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
    let now = now_ms();
    let email = body.email.trim();
    if email.is_empty() || !email.contains('@') {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "signup failed" })),
        )
            .into_response();
    }
    match db
        .ensure_account_for_email(email, body.password.as_deref(), now)
        .await
    {
        Ok(_) => {
            if db.magic_send_allowed(email, &client_ip.0, now).await {
                let _ = db.log_magic_send(email, &client_ip.0, now).await;
                let token = if body.password.as_deref().is_some_and(|p| !p.is_empty()) {
                    db.issue_account_magic_login(email, now)
                        .await
                        .ok()
                        .flatten()
                } else {
                    db.issue_account_magic_signup(email, now).await.ok()
                };
                if let Some(token) = token {
                    let url = format!(
                        "{}/api/account/auth/magic?token={token}",
                        state.mailer.public_url.trim_end_matches('/')
                    );
                    let _ = state
                        .mailer
                        .send_localized(
                            email,
                            "es",
                            "magicLink",
                            &[("minutes", "15"), ("url", &url)],
                        )
                        .await;
                }
            }
            (StatusCode::CREATED, Json(json!({ "ok": true }))).into_response()
        }
        Err(AccountSignupError::WeakPassword) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "weak password" })),
        )
            .into_response(),
        Err(AccountSignupError::EmailExists) | Err(AccountSignupError::Db) => {
            (StatusCode::CREATED, Json(json!({ "ok": true }))).into_response()
        }
    }
}

#[derive(Deserialize, Default)]
struct AccountLoginBody {
    email: String,
    password: String,
    #[serde(default)]
    next: Option<String>,
}

struct AccountLoginFields {
    email: String,
    password: String,
    next: String,
}

fn account_wants_json(headers: &HeaderMap) -> bool {
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    crate::auth::wants_json(accept, content_type)
}

fn account_login_fields(headers: &HeaderMap, body: &[u8]) -> AccountLoginFields {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if crate::auth::media_type_is_json(content_type) {
        let body: AccountLoginBody = serde_json::from_slice(body).unwrap_or_default();
        return AccountLoginFields {
            email: body.email,
            password: body.password,
            next: crate::auth::safe_next(body.next.as_deref().unwrap_or("/")),
        };
    }
    let parsed: HashMap<String, String> = serde_urlencoded::from_bytes(body).unwrap_or_default();
    AccountLoginFields {
        email: parsed.get("email").cloned().unwrap_or_default(),
        password: parsed.get("password").cloned().unwrap_or_default(),
        next: crate::auth::safe_next(parsed.get("next").map(String::as_str).unwrap_or("/")),
    }
}

fn account_login_redirect(loc: &str) -> impl IntoResponse {
    (StatusCode::SEE_OTHER, [(header::LOCATION, loc.to_string())])
}

#[derive(Deserialize)]
struct AccountMagicBody {
    email: String,
    #[serde(default)]
    locale: Option<String>,
}

async fn account_magic_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    client_ip: crate::ClientIp,
    Json(body): Json<AccountMagicBody>,
) -> impl IntoResponse {
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return (status, Json(json!({ "error": "csrf" }))).into_response();
    }
    let email = body.email.trim();
    let now = now_ms();
    let ip = client_ip.0;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "ok": true })).into_response();
    };
    if db.magic_send_allowed(email, &ip, now).await && state.mailer.allow_send(email, &ip) {
        if let Ok(Some(token)) = db.issue_account_magic_login(email, now).await {
            let _ = db.log_magic_send(email, &ip, now).await;
            let url = format!(
                "{}/api/account/auth/magic?token={token}",
                state.mailer.public_url.trim_end_matches('/')
            );
            let loc = body
                .locale
                .as_deref()
                .map(str::to_string)
                .or_else(|| auth::parse_display_cookie(&headers))
                .unwrap_or_else(|| "es".to_string());
            let _ = state
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

async fn account_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    client_ip: crate::ClientIp,
    body: Bytes,
) -> impl IntoResponse {
    if !account_wants_json(&headers) {
        if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
            return status.into_response();
        }
    }
    let json = account_wants_json(&headers);
    let fields = account_login_fields(&headers, &body);
    let next_q: String = url::form_urlencoded::byte_serialize(fields.next.as_bytes()).collect();
    let success_loc = format!("/account-clubs.html?next={next_q}");
    let fail_loc = format!("/login?e=1&next={next_q}");
    let Some(db) = club_db(&state) else {
        if json {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "club disabled" })),
            )
                .into_response();
        }
        return account_login_redirect(&fail_loc).into_response();
    };
    let now = now_ms();
    match db
        .account_login_password(fields.email.trim(), &fields.password, now, &client_ip.0)
        .await
    {
        Ok((session_id, _account)) => {
            let cookie = auth::set_account_session_cookie(&session_id, is_secure(&headers));
            if json {
                (
                    StatusCode::OK,
                    [(header::SET_COOKIE, cookie)],
                    Json(json!({ "ok": true })),
                )
                    .into_response()
            } else {
                (
                    StatusCode::SEE_OTHER,
                    [
                        (header::SET_COOKIE, cookie),
                        (header::LOCATION, success_loc),
                    ],
                )
                    .into_response()
            }
        }
        Err(_) => {
            if json {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({ "error": "unauthorized" })),
                )
                    .into_response()
            } else {
                account_login_redirect(&fail_loc).into_response()
            }
        }
    }
}

async fn account_me(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let now = now_ms();
    let Some(sess) = auth::parse_account_session_cookie(&headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some((account, active)) = db.account_from_session(&sess, now).await else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let memberships = db.list_account_memberships(&account.id).await;
    (
        StatusCode::OK,
        Json(json!({
            "account": {
                "id": account.id,
                "email": account.email,
                "emailVerifiedAt": account.email_verified_at,
            },
            "activeMembershipId": active,
            "memberships": memberships.iter().map(|m| json!({
                "id": m.id,
                "clubId": m.club_id,
                "slug": m.club_slug,
                "name": m.club_name,
                "role": m.role,
                "status": m.status,
            })).collect::<Vec<_>>(),
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
struct SwitchClubBody {
    #[serde(rename = "membershipId")]
    membership_id: String,
}

async fn switch_club(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SwitchClubBody>,
) -> impl IntoResponse {
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return (status, Json(json!({ "error": "csrf" }))).into_response();
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let now = now_ms();
    let Some(sess) = auth::parse_account_session_cookie(&headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let club_id = match db.membership_club_id(&body.membership_id).await {
        Ok(Some(cid)) => cid,
        _ => {
            return (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))).into_response();
        }
    };
    if db.club_operational(&club_id).await.is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "club unavailable" })),
        )
            .into_response();
    }
    match db
        .set_active_membership(&sess, &body.membership_id, now)
        .await
    {
        Ok(pilot) => {
            let mut resp = (
                StatusCode::OK,
                Json(json!({ "ok": true, "pilotId": pilot.id, "clubId": pilot.club_id })),
            )
                .into_response();
            if let Ok(ps) = db.create_pilot_session(&pilot.id, now).await {
                let cookie = auth::set_session_cookie(&ps, is_secure(&headers));
                resp.headers_mut().append(
                    axum::http::header::SET_COOKIE,
                    cookie.parse().expect("cookie"),
                );
            }
            resp
        }
        Err(_) => (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))).into_response(),
    }
}

#[derive(Deserialize)]
struct JoinClubBody {
    slug: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    license_no: String,
}

async fn join_club(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<JoinClubBody>,
) -> impl IntoResponse {
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return (status, Json(json!({ "error": "csrf" }))).into_response();
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let now = now_ms();
    let Some(sess) = auth::parse_account_session_cookie(&headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some((account, _)) = db.account_from_session(&sess, now).await else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(ctx) = db.club_id_for_slug(&body.slug).await else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "club not found" })),
        )
            .into_response();
    };
    match db
        .join_club_with_account(&account.id, &ctx, &body.name, &body.license_no, now)
        .await
    {
        Ok(mid) => (
            StatusCode::CREATED,
            Json(json!({ "ok": true, "membershipId": mid, "status": "pending" })),
        )
            .into_response(),
        Err(AccountJoinError::EmailExists) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "already member" })),
        )
            .into_response(),
        Err(AccountJoinError::ClubSuspended) => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "club suspended" })),
        )
            .into_response(),
        Err(AccountJoinError::ClubClosed) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "club closed" })),
        )
            .into_response(),
        Err(AccountJoinError::AccountSuspended) => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "account suspended" })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "join failed" })),
        )
            .into_response(),
    }
}

fn account_magic_html_escape(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

async fn get_account_magic(
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(token) = q.get("token") else {
        return (StatusCode::BAD_REQUEST, "missing token").into_response();
    };
    let purpose = q
        .get("purpose")
        .map(String::as_str)
        .unwrap_or("account_signup");
    let token_e = account_magic_html_escape(token);
    let purpose_e = account_magic_html_escape(purpose);
    Html(format!(
        r#"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><title>Confirm</title></head>
<body><h1>Continue sign-in</h1>
<form method="post" action="/api/account/auth/magic">
<input type="hidden" name="token" value="{token_e}">
<input type="hidden" name="purpose" value="{purpose_e}">
<button type="submit">Continue</button>
</form></body></html>"#
    ))
    .into_response()
}

#[derive(Deserialize)]
struct AccountMagicConsumeForm {
    token: String,
    #[serde(default)]
    purpose: Option<String>,
}

async fn post_account_magic(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    if auth::parse_account_session_cookie(&headers).is_some()
        || auth::parse_session_cookie(&headers).is_some()
    {
        if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
            return (status, Json(json!({ "error": "csrf" }))).into_response();
        }
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let (token, purpose) = if crate::auth::media_type_is_json(content_type) {
        let parsed: AccountMagicConsumeJson =
            serde_json::from_slice(&body).unwrap_or(AccountMagicConsumeJson {
                token: String::new(),
                purpose: None,
            });
        (parsed.token, parsed.purpose)
    } else {
        let parsed: AccountMagicConsumeForm =
            serde_urlencoded::from_bytes(&body).unwrap_or(AccountMagicConsumeForm {
                token: String::new(),
                purpose: None,
            });
        (parsed.token, parsed.purpose)
    };
    if token.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "missing token").into_response();
    }
    let purpose = purpose.as_deref().unwrap_or("account_signup");
    account_magic_consume(state, headers, token, purpose).await
}

#[derive(Deserialize, Default)]
struct AccountMagicConsumeJson {
    token: String,
    #[serde(default)]
    purpose: Option<String>,
}

async fn account_magic_consume(
    state: AppState,
    headers: HeaderMap,
    token: String,
    purpose: &str,
) -> axum::response::Response {
    let Some(db) = club_db(&state) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let now = now_ms();
    if purpose == crate::club::db_memberships_admin::ADMIN_INVITE_PURPOSE {
        match db.accept_admin_invite(&token, now).await {
            Ok(accept) => {
                let session_id = db
                    .create_account_session(&accept.account_id, Some(&accept.membership_id), now)
                    .await
                    .unwrap_or_default();
                let cookie = auth::set_account_session_cookie(&session_id, is_secure(&headers));
                (
                    StatusCode::FOUND,
                    [
                        (axum::http::header::SET_COOKIE, cookie),
                        (
                            axum::http::header::LOCATION,
                            "/club-admin-pilots.html".to_string(),
                        ),
                    ],
                )
                    .into_response()
            }
            Err(_) => (StatusCode::BAD_REQUEST, "invalid token").into_response(),
        }
    } else {
        match db.consume_account_magic(&token, purpose, now).await {
            Ok(account) => {
                let session_id = db
                    .create_account_session(&account.id, None, now)
                    .await
                    .unwrap_or_default();
                let cookie = auth::set_account_session_cookie(&session_id, is_secure(&headers));
                (
                    StatusCode::FOUND,
                    [
                        (axum::http::header::SET_COOKIE, cookie),
                        (
                            axum::http::header::LOCATION,
                            "/account-clubs.html".to_string(),
                        ),
                    ],
                )
                    .into_response()
            }
            Err(_) => (StatusCode::BAD_REQUEST, "invalid token").into_response(),
        }
    }
}

async fn approve_membership(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return (status, Json(json!({ "error": "csrf" }))).into_response();
    }
    let Some(db) = club_db(&state) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let now = now_ms();
    let sess = auth::parse_session_cookie(&headers);
    let Some(pilot) = db
        .pilot_from_session(sess.as_deref().unwrap_or_default(), now)
        .await
    else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let club_id: Option<String> = db.membership_club_id(&id).await.ok().flatten();
    let Some(club_id) = club_id else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    };
    if db.require_club_admin(&pilot.id, &club_id).await.is_err() {
        return (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))).into_response();
    }
    if db.club_operational(&club_id).await.is_err() {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "club unavailable" })),
        )
            .into_response();
    }
    match db.approve_membership(&id, now).await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "status": "active" })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "not pending" })),
        )
            .into_response(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/signup", get(serve_signup_page))
        .route("/api/account/signup-available", get(signup_available))
        .route("/api/account/signup", post(account_signup))
        .route("/api/account/login", post(account_login))
        .route("/api/account/magic-link", post(account_magic_link))
        .route("/api/account/me", get(account_me))
        .route("/api/account/switch-club", post(switch_club))
        .route("/api/account/join", post(join_club))
        .route(
            "/api/account/auth/magic",
            get(get_account_magic).post(post_account_magic),
        )
        .route(
            "/api/account/memberships/{id}/approve",
            post(approve_membership),
        )
}

async fn serve_signup_page(State(state): State<AppState>) -> impl IntoResponse {
    let path = state.dist_dir.join("signup.html");
    match tokio::fs::read_to_string(&path).await {
        Ok(html) => (
            StatusCode::OK,
            [
                (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (
                    axum::http::header::CONTENT_SECURITY_POLICY,
                    crate::static_files::HTML_CSP,
                ),
                (
                    axum::http::header::REFERRER_POLICY,
                    crate::static_files::HTML_REFERRER_POLICY,
                ),
            ],
            html,
        )
            .into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            "Signup page not built (npm run build)",
        )
            .into_response(),
    }
}
