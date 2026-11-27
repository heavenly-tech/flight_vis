use super::routes::is_secure;
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

async fn signup_available() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "ok": true, "signup": true })))
}

#[derive(Deserialize)]
struct AccountSignupBody {
    email: String,
    #[serde(default)]
    password: Option<String>,
    #[serde(default, rename = "clubSlug", alias = "club_slug")]
    club_slug: Option<String>,
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
    if !db.account_signup_throttle.allow(
        &client_ip.0,
        now_secs,
        crate::club::db::ACCOUNT_SIGNUP_MAX_PER_IP,
        crate::club::db::ACCOUNT_SIGNUP_WINDOW_SECS,
    ) {
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
    if body
        .password
        .as_deref()
        .is_some_and(|p| !p.is_empty() && p.len() < 8)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "weak password" })),
        )
            .into_response();
    }
    let requested_club_id = match body.club_slug.as_deref() {
        None => None,
        Some(raw) => {
            let trimmed = raw.trim();
            let normalized = crate::club::slug::normalize_slug(trimmed);
            let valid_chars = !normalized.is_empty()
                && normalized.len() <= 64
                && normalized
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                && !normalized.starts_with('-')
                && !normalized.ends_with('-')
                && !normalized.contains("--");
            if !valid_chars {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "club not found" })),
                )
                    .into_response();
            }
            let Some(row) = db.club_public_row_by_slug(&normalized).await else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "club not found" })),
                )
                    .into_response();
            };
            // Never leak unverified/pending clubs even if suspended.
            if row.status != "live" || row.email_verified_at.is_none() {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "club not found" })),
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
            Some(row.id)
        }
    };
    let signup_nonce = auth::new_session_id();
    match db.ensure_account_for_email(email, None, now).await {
        Ok(_) => {
            if db.magic_send_allowed(email, &client_ip.0, now).await {
                let _ = db.log_magic_send(email, &client_ip.0, now).await;
                let token = db
                    .issue_account_magic_signup_with_club(
                        email,
                        body.password.as_deref(),
                        Some(&signup_nonce),
                        requested_club_id.as_deref(),
                        now,
                    )
                    .await
                    .ok();
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
            let nonce_cookie = auth::set_signup_nonce_cookie(&signup_nonce, is_secure(&headers));
            (
                StatusCode::CREATED,
                [(header::SET_COOKIE, nonce_cookie)],
                Json(json!({ "ok": true })),
            )
                .into_response()
        }
        Err(AccountSignupError::WeakPassword) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "weak password" })),
        )
            .into_response(),
        Err(AccountSignupError::EmailExists) | Err(AccountSignupError::Db) => {
            (StatusCode::CREATED, Json(json!({ "ok": true }))).into_response()
        }
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "signup failed" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
struct AccountLoginBody {
    email: String,
    password: String,
    #[serde(default)]
    next: Option<String>,
    #[serde(rename = "installId", default)]
    install_id: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

struct AccountLoginFields {
    email: String,
    password: String,
    next: String,
    install_id: Option<String>,
    label: Option<String>,
    model: Option<String>,
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
            install_id: body.install_id,
            label: body.label,
            model: body.model,
        };
    }
    let parsed: HashMap<String, String> = serde_urlencoded::from_bytes(body).unwrap_or_default();
    AccountLoginFields {
        email: parsed.get("email").cloned().unwrap_or_default(),
        password: parsed.get("password").cloned().unwrap_or_default(),
        next: crate::auth::safe_next(parsed.get("next").map(String::as_str).unwrap_or("/")),
        install_id: None,
        label: None,
        model: None,
    }
}

async fn account_auth_payload(
    db: &ClubDb,
    account: &crate::club::db_account::AccountRow,
    active_membership_id: Option<&str>,
    device: Option<crate::club::db_devices_pg::DeviceRegisterResponse>,
) -> serde_json::Value {
    let memberships = db.list_account_memberships(&account.id).await;
    let mut body = json!({
        "ok": true,
        "account": {
            "id": account.id,
            "email": account.email,
            "emailVerified": account.email_verified_at.is_some(),
            "emailVerifiedAt": account.email_verified_at,
        },
        "activeMembershipId": active_membership_id,
        "memberships": memberships.iter().map(|m| json!({
            "id": m.id,
            "clubId": m.club_id,
            "slug": m.club_slug,
            "name": m.club_name,
            "role": m.role,
            "status": m.status,
        })).collect::<Vec<_>>(),
    });
    if let Some(dev) = device {
        body["device"] = json!(dev);
    }
    body
}

async fn maybe_register_account_login_device(
    state: &AppState,
    db: &ClubDb,
    client_ip: &str,
    pilot_id: &str,
    install_id: Option<&str>,
    label: Option<&str>,
    model: Option<&str>,
) -> Option<crate::club::db_devices_pg::DeviceRegisterResponse> {
    let iid = install_id.map(str::trim).filter(|s| !s.is_empty())?;
    if !state
        .device_register_throttle
        .allow(client_ip, crate::now_secs())
    {
        return None;
    }
    let label = label
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Dispositivo");
    db.register_device_for_install(pilot_id, label, model, iid)
        .await
        .ok()
}

fn account_login_redirect(loc: &str) -> impl IntoResponse {
    (StatusCode::SEE_OTHER, [(header::LOCATION, loc.to_string())])
}

#[derive(Deserialize, Default)]
struct AccountMagicBody {
    email: String,
    #[serde(default)]
    locale: Option<String>,
}

struct AccountMagicFields {
    email: String,
    locale: Option<String>,
    next: String,
}

fn account_magic_fields(headers: &HeaderMap, body: &[u8]) -> AccountMagicFields {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if crate::auth::media_type_is_json(content_type) {
        let body: AccountMagicBody = serde_json::from_slice(body).unwrap_or_default();
        return AccountMagicFields {
            email: body.email,
            locale: body.locale,
            next: "/".to_string(),
        };
    }
    let parsed: HashMap<String, String> = serde_urlencoded::from_bytes(body).unwrap_or_default();
    AccountMagicFields {
        email: parsed.get("email").cloned().unwrap_or_default(),
        locale: parsed.get("locale").cloned(),
        next: crate::auth::safe_next(parsed.get("next").map(String::as_str).unwrap_or("/")),
    }
}

async fn send_account_magic_link(
    state: &AppState,
    headers: &HeaderMap,
    email: &str,
    locale: Option<&str>,
    ip: &str,
) {
    let _ = auth::verify_password(email, auth::DUMMY_PASSWORD_HASH);
    let Some(db) = club_db(state) else {
        return;
    };
    let email = email.trim().to_string();
    let ip = ip.to_string();
    let mailer = state.mailer.clone();
    let public_url = state.mailer.public_url.trim_end_matches('/').to_string();
    let loc = locale
        .map(str::to_string)
        .or_else(|| auth::parse_fv_locale_cookie(headers))
        .unwrap_or_else(|| "es".to_string());
    let db = db.clone();
    tokio::spawn(async move {
        let now = now_ms();
        if !db.magic_send_allowed(&email, &ip, now).await || !mailer.allow_send(&email, &ip) {
            return;
        }
        let Ok(Some(token)) = db.issue_account_magic_login(&email, now).await else {
            return;
        };
        let _ = db.log_magic_send(&email, &ip, now).await;
        let url = format!("{public_url}/api/account/auth/magic?token={token}");
        let _ = mailer
            .send_localized(
                &email,
                &loc,
                "magicLink",
                &[("minutes", "15"), ("url", &url)],
            )
            .await;
    });
}

async fn account_magic_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    client_ip: crate::ClientIp,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return (status, Json(json!({ "error": "csrf" }))).into_response();
    }
    let json = account_wants_json(&headers);
    let fields = account_magic_fields(&headers, &body);
    let email = fields.email.trim();
    send_account_magic_link(
        &state,
        &headers,
        email,
        fields.locale.as_deref(),
        &client_ip.0,
    )
    .await;
    if json {
        Json(json!({ "ok": true })).into_response()
    } else {
        let next_q: String = url::form_urlencoded::byte_serialize(fields.next.as_bytes()).collect();
        account_login_redirect(&format!("/login?magic=1&next={next_q}")).into_response()
    }
}

async fn account_login(
    State(state): State<AppState>,
    headers: HeaderMap,
    client_ip: crate::ClientIp,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return status.into_response();
    }
    let json = account_wants_json(&headers);
    let fields = account_login_fields(&headers, &body);
    let next_q: String = url::form_urlencoded::byte_serialize(fields.next.as_bytes()).collect();
    let success_loc = format!("/account-clubs.html?next={next_q}");
    let fail_loc = format!("/login?e=1&mode=password&next={next_q}");
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
        Ok((session_id, account)) => {
            let cookie = auth::set_account_session_cookie(&session_id, is_secure(&headers));
            if json {
                let device = if fields
                    .install_id
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|s| !s.is_empty())
                {
                    if let Ok(personal_pid) = db.ensure_personal_pilot(&account.id, now).await {
                        maybe_register_account_login_device(
                            &state,
                            db,
                            &client_ip.0,
                            &personal_pid,
                            fields.install_id.as_deref(),
                            fields.label.as_deref(),
                            fields.model.as_deref(),
                        )
                        .await
                    } else {
                        None
                    }
                } else {
                    None
                };
                let payload = account_auth_payload(db, &account, None, device).await;
                (
                    StatusCode::OK,
                    [(header::SET_COOKIE, cookie)],
                    Json(payload),
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
        Err(crate::club::db::LoginError::Throttled) => {
            if json {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({ "error": "too many attempts" })),
                )
                    .into_response()
            } else {
                account_login_redirect(&fail_loc).into_response()
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

async fn account_logout(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return (status, Json(json!({ "error": "csrf" }))).into_response();
    }
    if let Some(db) = club_db(&state) {
        if let Some(account_sess) = auth::parse_account_session_cookie(&headers) {
            let _ = db.logout_account_session(&account_sess).await;
        }
        if let Some(club_sess) = auth::parse_session_cookie(&headers) {
            let _ = db.logout(&club_sess).await;
        }
    }
    let secure = is_secure(&headers);
    let clear_account = auth::clear_account_session_cookie(secure);
    let clear_club = auth::clear_session_cookie(secure);
    let clear_legacy = crate::auth::clear_cookie_header(secure);
    let mut res = (StatusCode::OK, Json(json!({ "ok": true }))).into_response();
    res.headers_mut()
        .append(header::SET_COOKIE, clear_account.parse().expect("cookie"));
    res.headers_mut()
        .append(header::SET_COOKIE, clear_club.parse().expect("cookie"));
    res.headers_mut()
        .append(header::SET_COOKIE, clear_legacy.parse().expect("cookie"));
    res
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
    let skip_current_password = db.account_session_magic_fresh(&sess, now).await;
    (
        StatusCode::OK,
        Json(json!({
            "account": {
                "id": account.id,
                "email": account.email,
                "emailVerifiedAt": account.email_verified_at,
            },
            "email_verified": account.email_verified_at.is_some(),
            "skipCurrentPassword": skip_current_password,
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
            let club_slug = db.club_slug(&pilot.club_id).await.unwrap_or_default();
            let club_name = db.club_name(&pilot.club_id).await.unwrap_or_default();
            let mut resp = (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "pilotId": pilot.id,
                    "clubId": pilot.club_id,
                    "activeMembershipId": body.membership_id,
                    "club": {
                        "id": pilot.club_id,
                        "slug": club_slug,
                        "name": club_name,
                    },
                })),
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
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let Some(token) = q.get("token") else {
        return (StatusCode::BAD_REQUEST, "missing token").into_response();
    };
    let purpose = if let Some(db) = club_db(&state) {
        db.peek_magic_token_purpose(token.trim())
            .await
            .or_else(|| q.get("purpose").cloned())
            .unwrap_or_else(|| "account_signup".to_string())
    } else {
        q.get("purpose")
            .cloned()
            .unwrap_or_else(|| "account_signup".to_string())
    };
    let token_e = account_magic_html_escape(token);
    let purpose_e = account_magic_html_escape(&purpose);
    (
        [
            (
                header::CONTENT_SECURITY_POLICY,
                crate::html_csp::HTML_STRICT_CSP,
            ),
            (
                header::REFERRER_POLICY,
                crate::html_csp::SENSITIVE_REFERRER_POLICY,
            ),
        ],
        Html(format!(
            r#"<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><title>Confirm</title></head>
<body><h1>Continue sign-in</h1>
<form method="post" action="/api/account/auth/magic">
<input type="hidden" name="token" value="{token_e}">
<input type="hidden" name="purpose" value="{purpose_e}">
<button type="submit">Continue</button>
</form></body></html>"#
        )),
    )
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
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return (status, Json(json!({ "error": "csrf" }))).into_response();
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
    let purpose = if let Some(db) = club_db(&state) {
        db.peek_magic_token_purpose(token.trim())
            .await
            .or(purpose)
            .unwrap_or_else(|| "account_signup".to_string())
    } else {
        purpose.unwrap_or_else(|| "account_signup".to_string())
    };
    account_magic_consume(state, headers, token, &purpose).await
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
    if matches!(
        purpose,
        crate::club::db_memberships_admin::ADMIN_INVITE_PURPOSE
            | crate::club::db_memberships_admin::PILOT_INVITE_PURPOSE
    ) {
        match db.accept_membership_invite(&token, now).await {
            Ok(accept) => {
                let session_id = db
                    .create_account_session_auth(
                        &accept.account_id,
                        Some(&accept.membership_id),
                        now,
                        crate::club::db_account::ACCOUNT_SESSION_AUTH_MAGIC,
                    )
                    .await
                    .unwrap_or_default();
                let cookie = auth::set_account_session_cookie(&session_id, is_secure(&headers));
                let location =
                    if accept.purpose == crate::club::db_memberships_admin::ADMIN_INVITE_PURPOSE {
                        "/club-admin-pilots.html"
                    } else {
                        "/account-clubs.html"
                    };
                (
                    StatusCode::FOUND,
                    [
                        (axum::http::header::SET_COOKIE, cookie),
                        (axum::http::header::LOCATION, location.to_string()),
                    ],
                )
                    .into_response()
            }
            Err(_) => (StatusCode::BAD_REQUEST, "invalid token").into_response(),
        }
    } else {
        let signup_nonce = auth::parse_signup_nonce_cookie(&headers);
        match db
            .consume_account_magic(&token, signup_nonce.as_deref(), now)
            .await
        {
            Ok(account) => {
                let session_id = db
                    .create_account_session_auth(
                        &account.id,
                        None,
                        now,
                        crate::club::db_account::ACCOUNT_SESSION_AUTH_MAGIC,
                    )
                    .await
                    .unwrap_or_default();
                let secure = is_secure(&headers);
                let cookie = auth::set_account_session_cookie(&session_id, secure);
                let clear_nonce = auth::clear_signup_nonce_cookie(secure);
                let mut res = (
                    StatusCode::FOUND,
                    [(
                        axum::http::header::LOCATION,
                        "/account-clubs.html".to_string(),
                    )],
                )
                    .into_response();
                res.headers_mut()
                    .append(axum::http::header::SET_COOKIE, cookie.parse().unwrap());
                res.headers_mut()
                    .append(axum::http::header::SET_COOKIE, clear_nonce.parse().unwrap());
                res
            }
            Err(_) => (StatusCode::BAD_REQUEST, "invalid token").into_response(),
        }
    }
}

#[derive(Deserialize)]
struct AccountSetPasswordBody {
    password: String,
    #[serde(default)]
    current_password: Option<String>,
}

async fn account_set_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    client_ip: crate::ClientIp,
    Json(body): Json<AccountSetPasswordBody>,
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
    let Some(sess) = auth::parse_account_session_cookie(&headers) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let now = now_ms();
    let now_sec = now / 1000;
    if !state.account_password_throttle.allow_up_to(
        &client_ip.0,
        now_sec as u64,
        crate::ACCOUNT_PASSWORD_MAX_ATTEMPTS,
    ) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "rate limit" })),
        )
            .into_response();
    }
    let Some((acct, _)) = db.account_from_session(&sess, now).await else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    if !state.account_password_account_throttle.allow_up_to(
        &acct.id,
        now_sec as u64,
        crate::ACCOUNT_PASSWORD_PER_ACCOUNT_MAX_ATTEMPTS,
    ) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "rate limit" })),
        )
            .into_response();
    }
    let current = body.current_password.as_deref();
    match db
        .set_account_password(&acct.id, &body.password, now, current, Some(&sess))
        .await
    {
        Ok(()) => {
            let locale = acct.locale.as_deref().unwrap_or("es");
            let login_url = format!("{}/login", state.mailer.public_url.trim_end_matches('/'));
            let _ = state
                .mailer
                .send_localized(
                    &acct.email,
                    locale,
                    "passwordChanged",
                    &[("loginUrl", &login_url)],
                )
                .await;
            (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
        }
        Err(AccountSignupError::WeakPassword) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "weak password" })),
        )
            .into_response(),
        Err(AccountSignupError::TooLong) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "password too long" })),
        )
            .into_response(),
        Err(AccountSignupError::WrongPassword) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "wrong password" })),
        )
            .into_response(),
        Err(AccountSignupError::AuthRequired) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "auth required" })),
        )
            .into_response(),
        Err(_) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "set password failed" })),
        )
            .into_response(),
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

#[derive(Deserialize, Default)]
struct AccountMagicCompleteBody {
    token: String,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(rename = "installId", default)]
    install_id: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

async fn account_magic_complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    client_ip: crate::ClientIp,
    Json(body): Json<AccountMagicCompleteBody>,
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
    let token = body.token.trim();
    if token.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "missing token" })),
        )
            .into_response();
    }
    let purpose = db
        .peek_magic_token_purpose(token)
        .await
        .or(body.purpose)
        .unwrap_or_else(|| "account_signup".to_string());
    let now = now_ms();
    let secure = is_secure(&headers);
    if matches!(
        purpose.as_str(),
        crate::club::db_memberships_admin::ADMIN_INVITE_PURPOSE
            | crate::club::db_memberships_admin::PILOT_INVITE_PURPOSE
    ) {
        match db.accept_membership_invite(token, now).await {
            Ok(accept) => {
                let Ok(session_id) = db
                    .create_account_session_auth(
                        &accept.account_id,
                        Some(&accept.membership_id),
                        now,
                        crate::club::db_account::ACCOUNT_SESSION_AUTH_MAGIC,
                    )
                    .await
                else {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": "session failed" })),
                    )
                        .into_response();
                };
                let Some(account) = db.account_by_id(&accept.account_id).await else {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": "account missing" })),
                    )
                        .into_response();
                };
                let device = if body
                    .install_id
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|s| !s.is_empty())
                {
                    let target_pid = if let Some(p) =
                        db.pilot_for_active_membership(&accept.membership_id).await
                    {
                        Some(p.id)
                    } else {
                        db.ensure_personal_pilot(&account.id, now).await.ok()
                    };
                    if let Some(pid) = target_pid {
                        maybe_register_account_login_device(
                            &state,
                            db,
                            &client_ip.0,
                            &pid,
                            body.install_id.as_deref(),
                            body.label.as_deref(),
                            body.model.as_deref(),
                        )
                        .await
                    } else {
                        None
                    }
                } else {
                    None
                };
                let cookie = auth::set_account_session_cookie(&session_id, secure);
                let payload =
                    account_auth_payload(db, &account, Some(&accept.membership_id), device).await;
                (
                    StatusCode::OK,
                    [(header::SET_COOKIE, cookie)],
                    Json(payload),
                )
                    .into_response()
            }
            Err(
                crate::club::db_memberships_admin::MembershipAdminError::ClubSuspended
                | crate::club::db_memberships_admin::MembershipAdminError::ClubClosed,
            ) => (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "club unavailable" })),
            )
                .into_response(),
            Err(_) => {
                (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid" }))).into_response()
            }
        }
    } else {
        let signup_nonce = auth::parse_signup_nonce_cookie(&headers);
        match db
            .consume_account_magic(token, signup_nonce.as_deref(), now)
            .await
        {
            Ok(account) => {
                let Ok(session_id) = db
                    .create_account_session_auth(
                        &account.id,
                        None,
                        now,
                        crate::club::db_account::ACCOUNT_SESSION_AUTH_MAGIC,
                    )
                    .await
                else {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({ "error": "session failed" })),
                    )
                        .into_response();
                };
                let device = if body
                    .install_id
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|s| !s.is_empty())
                {
                    if let Ok(personal_pid) = db.ensure_personal_pilot(&account.id, now).await {
                        maybe_register_account_login_device(
                            &state,
                            db,
                            &client_ip.0,
                            &personal_pid,
                            body.install_id.as_deref(),
                            body.label.as_deref(),
                            body.model.as_deref(),
                        )
                        .await
                    } else {
                        None
                    }
                } else {
                    None
                };
                let cookie = auth::set_account_session_cookie(&session_id, secure);
                let clear_nonce = auth::clear_signup_nonce_cookie(secure);
                let payload = account_auth_payload(db, &account, None, device).await;
                let mut res = (StatusCode::OK, Json(payload)).into_response();
                res.headers_mut()
                    .append(header::SET_COOKIE, cookie.parse().expect("cookie"));
                res.headers_mut()
                    .append(header::SET_COOKIE, clear_nonce.parse().expect("cookie"));
                res
            }
            Err(crate::club::db::MagicConsumeError::Expired) => {
                (StatusCode::GONE, Json(json!({ "error": "expired" }))).into_response()
            }
            Err(crate::club::db::MagicConsumeError::ClubUnavailable) => (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "club unavailable" })),
            )
                .into_response(),
            Err(
                crate::club::db::MagicConsumeError::Used
                | crate::club::db::MagicConsumeError::Invalid,
            ) => (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid" }))).into_response(),
        }
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/signup", get(serve_signup_page))
        .route("/api/account/signup-available", get(signup_available))
        .route("/api/account/signup", post(account_signup))
        .route("/api/account/login", post(account_login))
        .route("/api/account/logout", post(account_logout))
        .route("/api/account/password", post(account_set_password))
        .route("/api/account/magic-link", post(account_magic_link))
        .route("/api/account/me", get(account_me))
        .route("/api/account/switch-club", post(switch_club))
        .route("/api/account/join", post(join_club))
        .route(
            "/api/account/devices",
            get(super::routes::list_account_devices).post(super::routes::create_account_device),
        )
        .route(
            "/api/account/auth/magic",
            get(get_account_magic).post(post_account_magic),
        )
        .route(
            "/api/account/auth/magic/complete",
            post(account_magic_complete),
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
                    crate::html_csp::HTML_STRICT_CSP,
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
