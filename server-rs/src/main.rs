mod auth;
mod club;
mod device_auth;
mod flights;
mod html_csp;
mod i18n;
mod live;
mod static_files;
mod store;
mod video;

use auth::Auth;
use axum::{
    body::Bytes,
    extract::{ConnectInfo, DefaultBodyLimit, FromRequestParts, Query, State},
    http::{header, request::Parts, HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use device_auth::DeviceAuth;
use live::LiveEngine;
use serde_json::json;
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};
use store::Store;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) dist_dir: PathBuf,
    pub(crate) auth: Auth,
    pub(crate) device_auth: DeviceAuth,
    pub(crate) live: Arc<LiveEngine>,
    pub(crate) store: Store,
    pub(crate) club: Option<club::ClubDb>,
    pub(crate) video_queue: tokio::sync::mpsc::Sender<video::VideoJob>,
    pub(crate) login_throttle: LoginThrottle,
    pub(crate) device_register_throttle: LoginThrottle,
    pub(crate) share_lookup_throttle: LoginThrottle,
    pub(crate) account_password_throttle: LoginThrottle,
    pub(crate) account_password_account_throttle: LoginThrottle,
    pub(crate) mailer: club::mail::Mailer,
    pub(crate) webauthn: Option<club::webauthn::WebauthnState>,
    pub(crate) sse_limiter: std::sync::Arc<club::sse_limit::SseLimiter>,
    pub(crate) superadmin_emails: std::collections::HashSet<String>,
}

fn app_state(
    dist_dir: PathBuf,
    auth: Auth,
    device_auth: DeviceAuth,
    live: Arc<LiveEngine>,
    store: Store,
) -> (AppState, tokio::sync::mpsc::Receiver<video::VideoJob>) {
    let (video_queue, queue_rx) = tokio::sync::mpsc::channel(video::VIDEO_QUEUE_CAPACITY);
    (
        AppState {
            dist_dir,
            auth,
            device_auth,
            live,
            store,
            club: None,
            video_queue,
            login_throttle: LoginThrottle::default(),
            device_register_throttle: LoginThrottle::default(),
            share_lookup_throttle: LoginThrottle::default(),
            account_password_throttle: LoginThrottle::default(),
            account_password_account_throttle: LoginThrottle::default(),
            mailer: club::mail::Mailer::from_env(club::mail::MailOutbox::default()),
            webauthn: club::webauthn::WebauthnState::from_env(),
            sse_limiter: std::sync::Arc::new(club::sse_limit::SseLimiter::default()),
            superadmin_emails: club::superadmin::superadmin_emails_from_env(),
        },
        queue_rx,
    )
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

struct ServerConfig {
    dist_dir: PathBuf,
    data_dir: PathBuf,
    port: u16,
}

fn port_from_env() -> u16 {
    std::env::var("PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(8080)
}

fn config_from_env() -> ServerConfig {
    let data_dir = std::env::var("FLIGHT_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data"));
    let raw_dist = std::env::var("FLIGHT_VIS_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("dist"));
    // Normalize so prefix checks behave; fall back to the raw path when the
    // directory does not exist yet (all reads then 404/500).
    let dist_dir = std::fs::canonicalize(&raw_dist).unwrap_or(raw_dist);
    ServerConfig {
        dist_dir,
        data_dir,
        port: port_from_env(),
    }
}

fn session_token(headers: &HeaderMap) -> Option<String> {
    let cookie = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok());
    auth::parse_cookie(cookie)
}

fn is_secure(headers: &HeaderMap) -> bool {
    let forwarded = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok());
    auth::is_secure_request(forwarded)
}

fn wants_json(headers: &HeaderMap) -> bool {
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    auth::wants_json(accept, content_type)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

// Policy helpers
async fn signed_in(state: &AppState, headers: &HeaderMap) -> bool {
    // Legacy valid fv_session
    if let Some(token) = session_token(headers) {
        if state.auth.verify(&token, now_secs()) {
            return true;
        }
    }
    // Valid fv_account_session
    if let Some(db) = state.club.as_ref() {
        if let Some(sess) = club::parse_account_session_cookie(headers) {
            let now = club::now_ms();
            if db.account_from_session(&sess, now).await.is_some() {
                return true;
            }
        }
    }
    // Valid fv_club_session yielding a pilot
    if let Some(db) = state.club.as_ref() {
        if let Some(sess) = club::parse_session_cookie(headers) {
            let now = club::now_ms();
            let (_viewer, pilot) = db.viewer_with_pilot(Some(sess.as_str()), now).await;
            if pilot.is_some() {
                return true;
            }
        }
    }
    false
}

async fn can_manage_library(state: &AppState, headers: &HeaderMap) -> bool {
    // Legacy valid fv_session -> signed in AND can manage
    if let Some(token) = session_token(headers) {
        if state.auth.verify(&token, now_secs()) {
            return true;
        }
    }
    // Valid fv_account_session
    if let Some(db) = state.club.as_ref() {
        if let Some(sess) = club::parse_account_session_cookie(headers) {
            let now = club::now_ms();
            if let Some((account, _)) = db.account_from_session(&sess, now).await {
                // Can manage if account is platform superadmin OR email in superadmin allowlist
                if db.account_is_platform_superadmin(&account.id).await {
                    return true;
                }
                if club::superadmin::is_superadmin_email(&state.superadmin_emails, &account.email) {
                    return true;
                }
                return false;
            }
        }
    }
    // Valid fv_club_session with pilot
    if let Some(db) = state.club.as_ref() {
        if let Some(sess) = club::parse_session_cookie(headers) {
            let now = club::now_ms();
            let (_viewer, pilot) = db.viewer_with_pilot(Some(sess.as_str()), now).await;
            if let Some(pilot) = pilot {
                return db
                    .is_platform_superadmin(&pilot.id, &state.superadmin_emails)
                    .await;
            }
        }
    }
    false
}

async fn auth_me(State(state): State<AppState>, headers: HeaderMap) -> Json<serde_json::Value> {
    let authenticated = signed_in(&state, &headers).await;
    let can_manage = can_manage_library(&state, &headers).await;
    // Legacy session (fv_session) is the only one that POST /logout clears.
    let legacy_authenticated = session_token(&headers)
        .map(|token| state.auth.verify(&token, now_secs()))
        .unwrap_or(false);

    Json(json!({
        "authenticated": authenticated,
        "configured": state.auth.configured(),
        "canManageLibrary": can_manage,
        "legacyAuthenticated": legacy_authenticated,
    }))
}

async fn auth_legacy_gate(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Json<serde_json::Value> {
    let authenticated = session_token(&headers)
        .map(|token| state.auth.verify(&token, now_secs()))
        .unwrap_or(false);
    let mut allowed = authenticated;
    if !allowed {
        if let Some(db) = state.club.as_ref() {
            let session = club::parse_session_cookie(&headers);
            let now = club::now_ms();
            let (_viewer, pilot) = db.viewer_with_pilot(session.as_deref(), now).await;
            if let Some(pilot) = pilot {
                allowed = db
                    .is_platform_superadmin(&pilot.id, &state.superadmin_emails)
                    .await;
            }
        }
    }
    Json(json!({
        "allowed": allowed,
        "authenticated": authenticated,
        "configured": state.auth.configured(),
    }))
}

async fn club_admin_login_redirect(
    state: &AppState,
    headers: &HeaderMap,
    path: &str,
    query: Option<&str>,
) -> Option<axum::response::Response> {
    let normalized =
        static_files::normalize_url_path(path).unwrap_or_else(|| path.trim().to_string());
    if !normalized.starts_with("/club-admin-") || !normalized.ends_with(".html") {
        return None;
    }
    let db = state.club.as_ref()?;
    let session = club::parse_session_cookie(headers);
    let now = club::now_ms();
    let (_viewer, pilot) = db.viewer_with_pilot(session.as_deref(), now).await;
    if pilot
        .as_ref()
        .is_some_and(|p| p.role == "admin" || p.role == "staff")
    {
        return None;
    }
    let raw_next = match query {
        Some(q) if !q.is_empty() => format!("{normalized}?{q}"),
        _ => normalized.clone(),
    };
    let next = auth::safe_next(&raw_next);
    Some(redirect_found(&format!("/login?next={}", url_encode(&next))).into_response())
}

async fn fallback(
    State(state): State<AppState>,
    uri: Uri,
    headers: HeaderMap,
    method: axum::http::Method,
) -> Result<axum::response::Response, StatusCode> {
    if let Some(db) = state.club.as_ref() {
        if let Some(resp) = club::slug_redirect::maybe_club_slug_redirect(
            db,
            method.as_str(),
            uri.path(),
            uri.query(),
        )
        .await
        {
            return Ok(resp);
        }
    }
    if let Some(resp) = club_admin_login_redirect(&state, &headers, uri.path(), uri.query()).await {
        return Ok(resp);
    }
    static_files::serve(&state.dist_dir, uri.path(), &headers).await
}

const LOGIN_FALLBACK_HEAD: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>flight_vis · login</title>
    <style>
      :root { color-scheme: dark; --bg:#07090d; --panel:#10151d; --stroke:rgba(255,255,255,.12); --text:#f4f7fb; --muted:#9aa6b5; --cyan:#67e8f9; --bad:#f87171; }
      body { margin:0; min-height:100vh; display:flex; align-items:center; justify-content:center; background:var(--bg); color:var(--text); font:14px/1.5 Outfit,system-ui,sans-serif; }
      .sheet { width:min(400px,92vw); padding:28px 24px; border:1px solid var(--stroke); border-radius:16px; background:var(--panel); }
      h1 { margin:0; letter-spacing:.18em; text-transform:uppercase; font-size:11px; color:var(--cyan); }
      p { color:var(--muted); }
      label { display:block; font-size:12px; color:var(--muted); margin-bottom:6px; }
      input { width:100%; box-sizing:border-box; background:#0b0f16; color:var(--text); border:1px solid var(--stroke); border-radius:10px; padding:10px 12px; }
      button { margin-top:14px; width:100%; cursor:pointer; background:#ecebff; color:#15132a; border:0; border-radius:10px; padding:10px 12px; font-weight:650; }
      .err { color:var(--bad); min-height:1.2em; }
    </style>
  </head>
  <body>
    <form class="sheet" method="post" action="/login">
      <h1>flight_vis</h1>
      <p>heavenly.cl · password to save and upload tracks</p>
      "#;

const LOGIN_FALLBACK_LOCKED: &str =
    r#"<p class="lock">Upload is locked. Set <code>FLIGHT_VIS_PASSWORD</code> on the host.</p>"#;

const LOGIN_FALLBACK_FORM: &str = r#"<label for="password">Password</label>
      <input id="password" name="password" type="password" required autofocus />
      <input type="hidden" name="next" value="/" />
      <button type="submit">Log in</button>
      <p class="err"></p>"#;

const LOGIN_FALLBACK_TAIL: &str = r#"    </form>
  </body>
</html>"#;

fn login_fallback_html(configured: bool) -> String {
    let middle = if configured {
        LOGIN_FALLBACK_FORM
    } else {
        LOGIN_FALLBACK_LOCKED
    };
    format!("{LOGIN_FALLBACK_HEAD}{middle}{LOGIN_FALLBACK_TAIL}")
}

async fn read_login_page(dist_dir: &std::path::Path, configured: bool) -> String {
    let candidate = dist_dir.join("login.html");
    if let Ok(html) = tokio::fs::read_to_string(&candidate).await {
        return html;
    }
    login_fallback_html(configured)
}

fn login_redirect_query(query: &HashMap<String, String>, force_password_mode: bool) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    if let Some(next) = query.get("next") {
        let safe = auth::safe_next(next);
        if safe != "/" {
            pairs.push(("next", safe));
        }
    }
    if query.get("e") == Some(&"1".to_string()) {
        pairs.push(("e", "1".to_string()));
    }
    if force_password_mode || query.get("mode").map(String::as_str) == Some("password") {
        pairs.push(("mode", "password".to_string()));
    }
    if pairs.is_empty() {
        "/login".to_string()
    } else {
        let qs = pairs
            .iter()
            .map(|(k, v)| format!("{k}={}", url_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        format!("/login?{qs}")
    }
}

async fn club_login_html_redirect(
    Query(query): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    redirect_found(&login_redirect_query(&query, false))
}

async fn login_legacy_redirect(Query(query): Query<HashMap<String, String>>) -> impl IntoResponse {
    redirect_found(&login_redirect_query(&query, true))
}

fn apply_login_mode_html(html: &str, query: &HashMap<String, String>) -> String {
    if query.get("mode") != Some(&"password".to_string()) {
        return html.to_string();
    }
    html.replace(
        r#"id="login-lead" class="lead" data-i18n="entrar.magicLead""#,
        r#"id="login-lead" class="lead" data-i18n="entrar.passwordLead""#,
    )
    .replace(
        "Ingresa tu correo y te enviaremos un enlace para iniciar sesión.",
        "Inicia sesión con tu contraseña.",
    )
    .replace(
        r#"id="login-form" method="post" action="/api/account/magic-link""#,
        r#"id="login-form" method="post" action="/api/account/login""#,
    )
    .replace(r#"id="password-reveal" hidden"#, r#"id="password-reveal""#)
    .replace(
        r#"id="submit-magic" class="primary-btn" data-i18n="entrar.sendLink""#,
        r#"id="submit-magic" class="primary-btn" data-i18n="entrar.sendLink" hidden"#,
    )
    .replace(
        r#"id="submit-password" class="primary-btn" hidden data-i18n="club.login.submit""#,
        r#"id="submit-password" class="primary-btn" data-i18n="club.login.submit""#,
    )
    .replace(
        r#"id="use-password" class="muted-link""#,
        r#"id="use-password" class="muted-link" hidden"#,
    )
    .replace(
        r#"id="back-magic" class="muted-link" href="/login" hidden"#,
        r#"id="back-magic" class="muted-link" href="/login""#,
    )
    .replace(
        r#"id="legacy-details" class="muted" hidden"#,
        r#"id="legacy-details" class="muted""#,
    )
    .replace(
        r#"name="password" type="password" autocomplete="current-password""#,
        r#"name="password" type="password" autocomplete="current-password" required"#,
    )
}

fn apply_login_ssr_locale(html: &str, locale: &str) -> String {
    let cat = crate::i18n::require_global();
    let bcp47 = match locale {
        "en" => "en-US",
        _ => "es-CL",
    };
    let title = cat.t(locale, "entrar.title", &[]);
    let send_link = cat.t(locale, "entrar.sendLink", &[]);
    let magic_lead = cat.t(locale, "entrar.magicLead", &[]);
    let password_lead = cat.t(locale, "entrar.passwordLead", &[]);
    let lang_attr = format!(r#"lang="{bcp47}""#);
    let mut out = html.replace(r#"lang="es-CL""#, &lang_attr);
    for (key, fallback, localized) in [
        ("entrar.title", "Iniciar sesión", title.as_str()),
        ("entrar.sendLink", "Enviar enlace", send_link.as_str()),
        (
            "entrar.magicLead",
            "Ingresa tu correo y te enviaremos un enlace para iniciar sesión.",
            magic_lead.as_str(),
        ),
        (
            "entrar.passwordLead",
            "Inicia sesión con tu contraseña.",
            password_lead.as_str(),
        ),
    ] {
        let needle = format!(r#"data-i18n="{key}""#);
        if let Some(idx) = out.find(&needle) {
            if let Some(close) = out[idx..].find('>') {
                let start = idx + close + 1;
                if let Some(end) = out[start..].find('<') {
                    let text = &out[start..start + end];
                    if text.trim() == fallback
                        || matches!(
                            key,
                            "entrar.title"
                                | "entrar.sendLink"
                                | "entrar.magicLead"
                                | "entrar.passwordLead"
                        )
                    {
                        out.replace_range(start..start + end, localized);
                    }
                }
            }
        }
    }
    out
}

async fn login_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    if session_token(&headers)
        .map(|token| state.auth.verify(&token, now_secs()))
        .unwrap_or(false)
    {
        let next = query.get("next").map(String::as_str).unwrap_or("/");
        return redirect_found(&auth::safe_next(next)).into_response();
    }
    let raw = read_login_page(&state.dist_dir, state.auth.configured()).await;
    let html = apply_login_mode_html(&raw, &query);
    let accept = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let stored = crate::club::parse_fv_locale_cookie(&headers);
    let locale =
        crate::i18n::require_global().resolve_locale(None, None, accept, stored.as_deref());
    let html = apply_login_ssr_locale(&html, &locale);
    let mut res = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=UTF-8"),
            (
                header::CONTENT_SECURITY_POLICY,
                html_csp::csp_string(
                    html_csp::policy_for_route("/login").expect("/login in HTML_CSP_ROUTE_TABLE"),
                ),
            ),
            (header::REFERRER_POLICY, static_files::HTML_REFERRER_POLICY),
        ],
        html,
    )
        .into_response();
    if let Ok(value) = header::HeaderValue::from_str(&locale) {
        res.headers_mut().insert(header::CONTENT_LANGUAGE, value);
    }
    res
}

async fn login_html_redirect(Query(query): Query<HashMap<String, String>>) -> impl IntoResponse {
    match query.get("next") {
        Some(next) => redirect_found(&format!("/login?next={}", url_encode(next))).into_response(),
        None => redirect_found("/login").into_response(),
    }
}

fn redirect_found(location: &str) -> impl IntoResponse {
    (
        StatusCode::FOUND,
        [(header::LOCATION, location.to_string())],
        "",
    )
}

fn url_encode(input: &str) -> String {
    let mut out = String::new();
    for b in input.bytes() {
        if b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
        {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[derive(Default)]
struct LoginFields {
    password: String,
    next: String,
}

fn login_fields(headers: &HeaderMap, body: &[u8]) -> LoginFields {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if auth::media_type_is_json(content_type) {
        let parsed: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
        return LoginFields {
            password: parsed
                .get("password")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            next: auth::safe_next(parsed.get("next").and_then(|v| v.as_str()).unwrap_or("")),
        };
    }
    let parsed: HashMap<String, String> = serde_urlencoded::from_bytes(body).unwrap_or_default();
    LoginFields {
        password: parsed.get("password").cloned().unwrap_or_default(),
        next: auth::safe_next(parsed.get("next").map(String::as_str).unwrap_or("")),
    }
}

/// Failed and successful `POST /login` attempts share one sliding window per
/// client address. Forwarding headers are trusted only when the socket peer is
/// inside `TRUSTED_PROXY_CIDRS`; otherwise the socket address is used. When the
/// peer is trusted, `X-Real-IP` wins, then the last `X-Forwarded-For` entry
/// (the one appended by our own proxy).
pub(crate) const LOGIN_MAX_ATTEMPTS: usize = 8;
pub(crate) const SHARE_LOOKUP_MAX_ATTEMPTS: usize = 60;
pub(crate) const ACCOUNT_PASSWORD_MAX_ATTEMPTS: usize = 10;
pub(crate) const ACCOUNT_PASSWORD_PER_ACCOUNT_MAX_ATTEMPTS: usize = 5;
const LOGIN_WINDOW_SECS: u64 = 60;

#[derive(Clone, Default)]
pub(crate) struct LoginThrottle {
    hits: Arc<Mutex<HashMap<String, Vec<u64>>>>,
}

impl LoginThrottle {
    pub(crate) fn allow_up_to(&self, ip: &str, now: u64, max_attempts: usize) -> bool {
        let mut hits = self.hits.lock().unwrap_or_else(|err| err.into_inner());
        let cutoff = now.saturating_sub(LOGIN_WINDOW_SECS);
        let allowed = {
            let entry = hits.entry(ip.to_string()).or_default();
            entry.retain(|stamp| *stamp > cutoff);
            if entry.len() >= max_attempts {
                false
            } else {
                entry.push(now);
                true
            }
        };
        if hits.len() > 4096 {
            hits.retain(|_, times| times.last().is_some_and(|stamp| *stamp > cutoff));
        }
        allowed
    }

    fn allow(&self, ip: &str, now: u64) -> bool {
        let mut hits = self.hits.lock().unwrap_or_else(|err| err.into_inner());
        let cutoff = now.saturating_sub(LOGIN_WINDOW_SECS);
        let allowed = {
            let entry = hits.entry(ip.to_string()).or_default();
            entry.retain(|stamp| *stamp > cutoff);
            if entry.len() >= LOGIN_MAX_ATTEMPTS {
                false
            } else {
                entry.push(now);
                true
            }
        };
        if hits.len() > 4096 {
            hits.retain(|_, times| times.last().is_some_and(|stamp| *stamp > cutoff));
        }
        allowed
    }
}

pub(crate) struct ClientIp(pub String);

impl<S> FromRequestParts<S> for ClientIp
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(ClientIp(client_ip(parts)))
    }
}

fn client_ip(parts: &Parts) -> String {
    let peer = parts
        .extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());
    client_ip_with(peer, &parts.headers, trusted_proxy_cidrs())
}

/// Resolve the throttle key from a socket peer and forwarding headers. A client
/// that connects directly (peer outside every trusted network) cannot influence
/// the result with spoofed `X-Forwarded-For` / `X-Real-IP` headers.
fn client_ip_with(peer: Option<IpAddr>, headers: &HeaderMap, trusted: &[Cidr]) -> String {
    let peer_trusted = peer.is_some_and(|ip| trusted.iter().any(|cidr| cidr.contains(&ip)));
    if peer_trusted {
        if let Some(ip) = parse_ip_header(headers.get("x-real-ip"), false) {
            return ip;
        }
        if let Some(ip) = parse_ip_header(headers.get("x-forwarded-for"), true) {
            return ip;
        }
    }
    match peer {
        Some(ip) => ip.to_string(),
        None => "unknown".to_string(),
    }
}

/// Parse a single IP out of a forwarding header. `last_hop` selects the final
/// comma-separated `X-Forwarded-For` entry (the nearest proxy to us). Values
/// that do not parse as an `IpAddr` are ignored.
fn parse_ip_header(value: Option<&axum::http::HeaderValue>, last_hop: bool) -> Option<String> {
    let raw = value?.to_str().ok()?;
    let token = if last_hop {
        raw.rsplit(',').next().unwrap_or("").trim()
    } else {
        raw.trim()
    };
    token.parse::<IpAddr>().ok().map(|ip| ip.to_string())
}

/// Default expands to the docker `proxy_network` plus loopback.
const DEFAULT_TRUSTED_PROXY_CIDRS: &str = "172.18.0.0/16,127.0.0.1/32,::1/128";

#[derive(Clone, Copy, Eq, PartialEq, Debug)]
struct Cidr {
    base: IpAddr,
    prefix: u8,
}

impl Cidr {
    fn parse(raw: &str) -> Option<Cidr> {
        let raw = raw.trim();
        let (addr_part, prefix_part) = match raw.split_once('/') {
            Some((addr, prefix)) => (addr.trim(), Some(prefix.trim())),
            None => (raw, None),
        };
        let base: IpAddr = addr_part.parse().ok()?;
        let max = match base {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        let prefix = match prefix_part {
            Some(prefix) => prefix.parse::<u8>().ok()?,
            None => max,
        };
        if prefix > max {
            return None;
        }
        Some(Cidr { base, prefix })
    }

    fn contains(&self, ip: &IpAddr) -> bool {
        match (self.base, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = prefix_mask32(self.prefix);
                (u32::from(net) & mask) == (u32::from(*ip) & mask)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = prefix_mask128(self.prefix);
                (u128::from(net) & mask) == (u128::from(*ip) & mask)
            }
            _ => false,
        }
    }
}

fn prefix_mask32(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    }
}

fn prefix_mask128(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    }
}

fn parse_trusted_cidrs(raw: &str) -> Vec<Cidr> {
    raw.split(',').filter_map(Cidr::parse).collect()
}

/// Parse `TRUSTED_PROXY_CIDRS` once. A missing, empty, or unparseable value
/// falls back to [`DEFAULT_TRUSTED_PROXY_CIDRS`].
fn trusted_proxy_cidrs() -> &'static [Cidr] {
    static CIDRS: OnceLock<Vec<Cidr>> = OnceLock::new();
    CIDRS.get_or_init(|| {
        let raw = std::env::var("TRUSTED_PROXY_CIDRS").unwrap_or_default();
        let parsed = parse_trusted_cidrs(&raw);
        if parsed.is_empty() {
            parse_trusted_cidrs(DEFAULT_TRUSTED_PROXY_CIDRS)
        } else {
            parsed
        }
    })
}

fn too_many_logins(json: bool) -> axum::response::Response {
    if json {
        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "too many attempts" })),
        )
            .into_response()
    } else {
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "too many attempts",
        )
            .into_response()
    }
}

async fn login_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    client_ip: ClientIp,
    body: Bytes,
) -> impl IntoResponse {
    if let Err(status) = club::csrf::cookie_post_allowed(&headers) {
        return status.into_response();
    }
    let json = wants_json(&headers);
    if !state.login_throttle.allow(&client_ip.0, now_secs()) {
        return too_many_logins(json);
    }
    if !state.auth.configured() {
        if json {
            return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "locked" }))).into_response();
        }
        return redirect_found("/login?mode=password&locked=1").into_response();
    }
    let fields = login_fields(&headers, &body);
    if !state.auth.check_password(&fields.password) {
        if json {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "unauthorized" })),
            )
                .into_response();
        }
        let mut q = HashMap::new();
        q.insert("e".to_string(), "1".to_string());
        if fields.next != "/" {
            q.insert("next".to_string(), fields.next.clone());
        }
        return redirect_found(&login_redirect_query(&q, true)).into_response();
    }
    let secure = is_secure(&headers);
    let cookie = auth::set_cookie_header(&state.auth.mint(now_secs()), secure);
    if json {
        return (
            StatusCode::OK,
            [(header::SET_COOKIE, cookie)],
            Json(json!({ "ok": true, "next": fields.next })),
        )
            .into_response();
    }
    ([(header::SET_COOKIE, cookie)], redirect_found(&fields.next)).into_response()
}

async fn logout(State(_state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let secure = is_secure(&headers);
    let cookie = auth::clear_cookie_header(secure);
    if wants_json(&headers) {
        return (
            StatusCode::OK,
            [(header::SET_COOKIE, cookie)],
            Json(json!({ "ok": true })),
        )
            .into_response();
    }
    ([(header::SET_COOKIE, cookie)], redirect_found("/login")).into_response()
}

async fn require_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), Box<axum::response::Response>> {
    if can_manage_library(state, headers).await {
        return Ok(());
    }
    Err(Box::new(auth_denied(state)))
}

/// Unauthorized/locked body shared by every disk-library write and the list.
fn auth_denied(state: &AppState) -> axum::response::Response {
    let error = if state.auth.configured() {
        "unauthorized"
    } else {
        "locked"
    };
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": error }))).into_response()
}

fn build_router(state: AppState) -> Router {
    let mut app = Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/me", get(auth_me))
        .route("/api/auth/legacy-gate", get(auth_legacy_gate))
        .route("/login", get(login_page).post(login_post))
        .route("/login.html", get(login_html_redirect))
        .route("/login/legacy", get(login_legacy_redirect))
        .route("/club-login.html", get(club_login_html_redirect))
        .route("/logout", get(logout).post(logout));
    if state.club.is_some() {
        app = app.merge(club::router()).merge(club::contract_router());
    }
    app.merge(flights::router().layer(DefaultBodyLimit::max(64 * 1024 * 1024)))
        .merge(video::router())
        .merge(live::router().layer(DefaultBodyLimit::max(256 * 1024)))
        .fallback(fallback)
        .with_state(state)
}

/// Container healthcheck. The runtime image is `FROM scratch`, so there is no
/// wget/curl/busybox to probe `/api/health`; the binary does it itself over
/// loopback, honouring the same `PORT` as the server.
const HEALTHCHECK_ARG: &str = "healthcheck";
const CHECK_I18N_ARG: &str = "check-i18n";
const SEED_ACCOUNT_ARG: &str = "seed-account";
const HEALTHCHECK_HOST: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
const HEALTHCHECK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

fn health_response_ok(raw: &[u8]) -> bool {
    let split = match raw.windows(4).position(|window| window == b"\r\n\r\n") {
        Some(index) => index,
        None => return false,
    };
    let (head, body) = (&raw[..split], &raw[split + 4..]);
    let head = match std::str::from_utf8(head) {
        Ok(head) => head,
        Err(_) => return false,
    };
    let status_ok = head
        .lines()
        .next()
        .map(|line| {
            let mut parts = line.split_whitespace();
            matches!(parts.next(), Some(version) if version.starts_with("HTTP/1."))
                && parts.next() == Some("200")
        })
        .unwrap_or(false);
    status_ok
        && serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|value| value.get("ok").and_then(serde_json::Value::as_bool))
            == Some(true)
}

fn healthcheck_at(addr: SocketAddr) -> bool {
    use std::io::{Read, Write};

    let mut stream = match std::net::TcpStream::connect_timeout(&addr, HEALTHCHECK_TIMEOUT) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("healthcheck: connect {addr}: {error}");
            return false;
        }
    };
    let _ = stream.set_read_timeout(Some(HEALTHCHECK_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HEALTHCHECK_TIMEOUT));
    let request = format!("GET /api/health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if let Err(error) = stream.write_all(request.as_bytes()) {
        eprintln!("healthcheck: write {addr}: {error}");
        return false;
    }
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => {
                raw.extend_from_slice(&buf[..read]);
                if raw.len() > 16 * 1024 {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                eprintln!("healthcheck: read {addr}: {error}");
                return false;
            }
        }
    }
    if health_response_ok(&raw) {
        true
    } else {
        eprintln!("healthcheck: unhealthy response from {addr}");
        false
    }
}

fn run_healthcheck() -> bool {
    healthcheck_at(SocketAddr::new(HEALTHCHECK_HOST, port_from_env()))
}

fn run_check_i18n() -> bool {
    let config = config_from_env();
    let dir = i18n::Catalogs::resolve_dir(&config.dist_dir);
    match i18n::Catalogs::load_from_dir(&dir) {
        Ok(cat) => {
            if cat.canonical != "es" {
                eprintln!("check-i18n: expected canonical locale es");
                return false;
            }
            let has_es_cl = cat
                .locale_labels()
                .any(|(id, _, bcp)| id == "es" && bcp == "es-CL");
            if !has_es_cl {
                eprintln!("check-i18n: es locale must be es-CL");
                return false;
            }
            let sample = cat.t("es", "club.login.title", &[]);
            if sample == "club.login.title" || sample.is_empty() {
                eprintln!("check-i18n: missing Spanish catalog strings");
                return false;
            }
            true
        }
        Err(error) => {
            eprintln!("check-i18n: {error}");
            false
        }
    }
}

fn main() {
    // Any other argument (or none) still starts the server, as before.
    // `args_os` so a non-UTF-8 argument cannot panic the process.
    let arg = std::env::args_os().nth(1);
    if arg.as_deref() == Some(std::ffi::OsStr::new(HEALTHCHECK_ARG)) {
        std::process::exit(if run_healthcheck() { 0 } else { 1 });
    }
    if arg.as_deref() == Some(std::ffi::OsStr::new(CHECK_I18N_ARG)) {
        std::process::exit(if run_check_i18n() { 0 } else { 1 });
    }
    if arg.as_deref() == Some(std::ffi::OsStr::new(SEED_ACCOUNT_ARG)) {
        std::process::exit(run_seed_account_cli());
    }
    run_server();
}

fn run_seed_account_cli() -> i32 {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let mut email = String::new();
    let mut club = String::from("cas");
    let mut role = String::from("admin");
    let mut dry_run = true;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--email" if i + 1 < args.len() => {
                email = args[i + 1].clone();
                i += 2;
            }
            "--club" if i + 1 < args.len() => {
                club = args[i + 1].clone();
                i += 2;
            }
            "--role" if i + 1 < args.len() => {
                role = args[i + 1].clone();
                i += 2;
            }
            "--dry-run" => {
                dry_run = true;
                i += 1;
            }
            "--apply" => {
                dry_run = false;
                i += 1;
            }
            _ => i += 1,
        }
    }
    if email.is_empty() {
        eprintln!("usage: flight_vis seed-account --email EMAIL [--club SLUG] [--role admin] [--dry-run|--apply]");
        return 2;
    }
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        match club::ClubDb::connect().await {
            Ok(db) => match db.seed_account_cli(&email, &club, &role, dry_run).await {
                Ok(v) => {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    0
                }
                Err(e) => {
                    eprintln!("seed-account error: {e}");
                    1
                }
            },
            Err(e) => {
                eprintln!("database: {e}");
                1
            }
        }
    })
}

#[tokio::main]
async fn run_server() {
    let config = config_from_env();
    match i18n::init_global(&i18n::Catalogs::resolve_dir(&config.dist_dir)) {
        Ok(_) => {}
        Err(error) => {
            eprintln!("error: i18n catalogs: {error}");
            std::process::exit(1);
        }
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let live = Arc::new(LiveEngine::new(&config.data_dir));
    if let Err(error) = live.load_from_disk().await {
        eprintln!("warn: could not load live sessions: {error}");
    }
    let live_task = Arc::clone(&live);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            live_task.tick_idle_and_prune().await;
        }
    });
    let club = match club::ClubDb::connect().await {
        Ok(db) => {
            let now = club::now_ms();
            if let Err(error) = db.purge_stale_pending_clubs(now).await {
                eprintln!("warn: pending club purge failed: {error}");
            }
            if std::env::var("FLIGHT_CLUB_SEED_DEMO").as_deref() == Ok("1") {
                if let Err(error) = db.seed_demo_if_empty().await {
                    eprintln!("warn: demo seed failed: {error}");
                }
            }
            if let Err(error) = db.backfill_accounts_from_pilots().await {
                eprintln!("warn: pilot account backfill failed: {error}");
            }
            Some(db)
        }
        Err(error) => {
            eprintln!("warn: club database unavailable: {error}");
            None
        }
    };
    let mut state_bundle = app_state(
        config.dist_dir,
        Auth::from_env(),
        DeviceAuth::from_env().unwrap_or_else(|error| {
            eprintln!("device token config error: {error}");
            std::process::exit(1);
        }),
        live,
        Store::new(&config.data_dir),
    );
    state_bundle.0.club = club.clone();
    let (state, queue_rx) = state_bundle;
    if let Some(db) = club.clone() {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                let now = club::now_ms();
                let _ = db.auto_end_idle(10 * 60 * 1000, now).await;
            }
        });
    }
    state
        .store
        .seed(
            vec![
                root.join("../samples/vuelo5.gpx"),
                root.join("../public/samples/vuelo5.gpx"),
                root.join("../dist/samples/vuelo5.gpx"),
            ],
            None,
        )
        .await
        .expect("seed sample flight");
    let worker_store = state.store.clone();
    video::spawn_transcode_worker(worker_store, queue_rx);
    video::requeue_transcoding(&state.store, &state.video_queue).await;
    let app = build_router(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    eprintln!("flight_vis (rust) listening on {}", config.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind listener");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("serve app");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderName, HeaderValue};

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn cidr(raw: &str) -> Cidr {
        Cidr::parse(raw).unwrap()
    }

    fn ip(raw: &str) -> IpAddr {
        raw.parse().unwrap()
    }

    #[test]
    fn cidr_matching_v4_and_v6() {
        let net = cidr("172.18.0.0/16");
        assert!(net.contains(&ip("172.18.5.5")));
        assert!(!net.contains(&ip("172.19.0.1")));

        let loopback = cidr("127.0.0.1/32");
        assert!(loopback.contains(&ip("127.0.0.1")));
        assert!(!loopback.contains(&ip("127.0.0.2")));

        let v6 = cidr("::1/128");
        assert!(v6.contains(&ip("::1")));
        assert!(!v6.contains(&ip("::2")));

        let v6net = cidr("2001:db8::/32");
        assert!(v6net.contains(&ip("2001:db8::1")));
        assert!(!v6net.contains(&ip("2001:db9::1")));

        // Families never cross-match.
        assert!(!net.contains(&ip("::1")));
    }

    #[test]
    fn cidr_parse_rejects_bad_input() {
        assert_eq!(Cidr::parse(""), None);
        assert_eq!(Cidr::parse("not-an-ip"), None);
        assert_eq!(Cidr::parse("172.18.0.0/33"), None);
        assert_eq!(Cidr::parse("::1/129"), None);
        assert_eq!(Cidr::parse("172.18.0.0/abc"), None);
        // A bare address is a host route.
        assert_eq!(
            cidr("10.0.0.1"),
            Cidr {
                base: ip("10.0.0.1"),
                prefix: 32
            }
        );
    }

    #[test]
    fn untrusted_peer_ignores_spoofed_forwarding_headers() {
        let trusted = [cidr("172.18.0.0/16")];
        let peer = Some(ip("198.51.100.9"));

        let spoofed = headers(&[("x-forwarded-for", "1.2.3.4")]);
        assert_eq!(client_ip_with(peer, &spoofed, &trusted), "198.51.100.9");

        let real = headers(&[("x-real-ip", "1.2.3.4")]);
        assert_eq!(client_ip_with(peer, &real, &trusted), "198.51.100.9");
    }

    #[test]
    fn trusted_peer_prefers_x_real_ip() {
        let trusted = [cidr("172.18.0.0/16")];
        let hdrs = headers(&[
            ("x-real-ip", "9.9.9.9"),
            ("x-forwarded-for", "1.2.3.4, 5.6.7.8"),
        ]);
        let peer = Some(ip("172.18.0.5"));
        assert_eq!(client_ip_with(peer, &hdrs, &trusted), "9.9.9.9");
    }

    #[test]
    fn trusted_peer_uses_last_forwarded_for_entry() {
        let trusted = [cidr("172.18.0.0/16")];
        let hdrs = headers(&[("x-forwarded-for", "1.2.3.4, 5.6.7.8")]);
        let peer = Some(ip("172.18.0.5"));
        assert_eq!(client_ip_with(peer, &hdrs, &trusted), "5.6.7.8");
    }

    #[test]
    fn trusted_peer_with_invalid_header_falls_back_to_peer() {
        let trusted = [cidr("172.18.0.0/16")];
        let hdrs = headers(&[("x-forwarded-for", "not-an-ip")]);
        let peer = Some(ip("172.18.0.5"));
        assert_eq!(client_ip_with(peer, &hdrs, &trusted), "172.18.0.5");
    }

    #[test]
    fn missing_connect_info_is_unknown() {
        let trusted = [cidr("127.0.0.1/32")];
        assert_eq!(client_ip_with(None, &HeaderMap::new(), &trusted), "unknown");
        // A spoofed header cannot rescue a missing peer.
        let hdrs = headers(&[("x-forwarded-for", "1.2.3.4")]);
        assert_eq!(client_ip_with(None, &hdrs, &trusted), "unknown");
    }

    #[test]
    fn default_cidrs_cover_docker_proxy_and_loopback() {
        let cidrs = parse_trusted_cidrs(DEFAULT_TRUSTED_PROXY_CIDRS);
        assert!(cidrs.iter().any(|c| c.contains(&ip("172.18.0.9"))));
        assert!(cidrs.iter().any(|c| c.contains(&ip("127.0.0.1"))));
        assert!(cidrs.iter().any(|c| c.contains(&ip("::1"))));
    }

    #[test]
    fn healthcheck_accepts_ok_response() {
        assert!(health_response_ok(
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\n\r\n{\"ok\":true}"
        ));
    }

    #[test]
    fn healthcheck_rejects_unhealthy_responses() {
        assert!(!health_response_ok(b""));
        // No header terminator: the response is incomplete.
        assert!(!health_response_ok(b"HTTP/1.1 200 OK\r\n{\"ok\":true}"));
        assert!(!health_response_ok(
            b"HTTP/1.1 503 Service Unavailable\r\n\r\n{\"ok\":false}"
        ));
        assert!(!health_response_ok(b"HTTP/1.1 200 OK\r\n\r\nnot json"));
        assert!(!health_response_ok(
            b"HTTP/1.1 200 OK\r\n\r\n{\"ok\":false}"
        ));
    }

    #[test]
    fn healthcheck_probes_loopback_and_fails_when_closed() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = [0u8; 1024];
            let read = stream.read(&mut request).expect("read request");
            assert!(String::from_utf8_lossy(&request[..read]).starts_with("GET /api/health "));
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 11\r\n\r\n{\"ok\":true}",
                )
                .expect("write response");
        });
        assert!(healthcheck_at(addr));
        server.join().expect("server thread");

        // A listener that was dropped refuses connections.
        let gone = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let gone_addr = gone.local_addr().expect("addr");
        drop(gone);
        assert!(!healthcheck_at(gone_addr));
    }

    #[test]
    fn login_ssr_password_mode_updates_lead_and_keeps_legacy_collapsed() {
        let locales = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../locales");
        let _ = crate::i18n::init_global(&locales);
        let raw = include_str!("../../login.html");

        // Default magic-link mode: magic lead intact, legacy details hidden, secondary pilot CTA.
        let default_html = apply_login_mode_html(raw, &HashMap::new());
        assert!(
            default_html.contains(r#"id="login-lead" class="lead" data-i18n="entrar.magicLead""#)
        );
        assert!(default_html
            .contains("Ingresa tu correo y te enviaremos un enlace para iniciar sesión."));
        assert!(default_html.contains(r#"id="legacy-details" class="muted" hidden"#));
        assert!(default_html.contains(
            r#"id="register-pilot"
          class="secondary""#
        ));
        assert!(!default_html.contains(
            r#"id="register-pilot"
          class="primary""#
        ));

        let default_en = apply_login_ssr_locale(&default_html, "en");
        assert!(default_en.contains("Enter your email and we will send you a sign-in link."));

        // Password mode: password lead swapped, legacy details un-hidden WITHOUT open attribute.
        let mut pw_query = HashMap::new();
        pw_query.insert("mode".to_string(), "password".to_string());
        let pw_html = apply_login_mode_html(raw, &pw_query);
        assert!(pw_html.contains(r#"id="login-lead" class="lead" data-i18n="entrar.passwordLead""#));
        assert!(pw_html.contains("Inicia sesión con tu contraseña."));
        assert!(
            !pw_html.contains("Ingresa tu correo y te enviaremos un enlace para iniciar sesión.")
        );
        assert!(pw_html.contains(r#"<details id="legacy-details" class="muted">"#));
        assert!(!pw_html.contains(r#"id="legacy-details" class="muted" open"#));
        assert!(!pw_html.contains(r#"id="legacy-details" class="muted" hidden"#));
        assert!(pw_html.contains(r#"id="login-form" method="post" action="/api/account/login""#));

        let pw_es = apply_login_ssr_locale(&pw_html, "es");
        assert!(pw_es.contains("Inicia sesión con tu contraseña."));

        let pw_en = apply_login_ssr_locale(&pw_html, "en");
        assert!(pw_en.contains("Sign in with your password."));
        assert!(!pw_en.contains("Inicia sesión con tu contraseña."));
    }
}
