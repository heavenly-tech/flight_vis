mod auth;
mod flights;
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
use serde_json::json;
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use store::Store;

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) dist_dir: PathBuf,
    pub(crate) auth: Auth,
    pub(crate) store: Store,
    pub(crate) video_queue: tokio::sync::mpsc::Sender<video::VideoJob>,
    pub(crate) login_throttle: LoginThrottle,
}

fn app_state(
    dist_dir: PathBuf,
    auth: Auth,
    store: Store,
) -> (AppState, tokio::sync::mpsc::Receiver<video::VideoJob>) {
    let (video_queue, queue_rx) = tokio::sync::mpsc::channel(video::VIDEO_QUEUE_CAPACITY);
    (
        AppState {
            dist_dir,
            auth,
            store,
            video_queue,
            login_throttle: LoginThrottle::default(),
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
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(8080);
    ServerConfig {
        dist_dir,
        data_dir,
        port,
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

async fn auth_me(State(state): State<AppState>, headers: HeaderMap) -> Json<serde_json::Value> {
    let authenticated = session_token(&headers)
        .map(|token| state.auth.verify(&token, now_secs()))
        .unwrap_or(false);
    Json(json!({
        "authenticated": authenticated,
        "configured": state.auth.configured(),
    }))
}

async fn fallback(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<axum::response::Response, StatusCode> {
    static_files::serve(&state.dist_dir, uri.path()).await
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
    let html = read_login_page(&state.dist_dir, state.auth.configured()).await;
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=UTF-8")],
        html,
    )
        .into_response()
}

async fn login_html_redirect(Query(query): Query<HashMap<String, String>>) -> impl IntoResponse {
    match query.get("next") {
        Some(next) => {
            redirect_found(&format!("/login?next={}", url_encode(next))).into_response()
        }
        None => redirect_found("/login").into_response(),
    }
}

fn redirect_found(location: &str) -> impl IntoResponse {
    (StatusCode::FOUND, [(header::LOCATION, location.to_string())], "")
}

fn url_encode(input: &str) -> String {
    let mut out = String::new();
    for b in input.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')') {
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
    if content_type.contains("application/json") {
        let parsed: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
        return LoginFields {
            password: parsed
                .get("password")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            next: auth::safe_next(
                parsed.get("next").and_then(|v| v.as_str()).unwrap_or(""),
            ),
        };
    }
    let parsed: HashMap<String, String> = serde_urlencoded::from_bytes(body).unwrap_or_default();
    LoginFields {
        password: parsed.get("password").cloned().unwrap_or_default(),
        next: auth::safe_next(parsed.get("next").map(String::as_str).unwrap_or("")),
    }
}

/// Failed and successful `POST /login` attempts share one sliding window per
/// client address. Direct peers use the socket address; behind the compose
/// proxy, `X-Forwarded-For` (then `X-Real-IP`) is the client address.
pub(crate) const LOGIN_MAX_ATTEMPTS: usize = 8;
const LOGIN_WINDOW_SECS: u64 = 60;

#[derive(Clone, Default)]
pub(crate) struct LoginThrottle {
    hits: Arc<Mutex<HashMap<String, Vec<u64>>>>,
}

impl LoginThrottle {
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
    if let Some(ip) = header_ip(parts.headers.get("x-forwarded-for"), true) {
        return ip;
    }
    if let Some(ip) = header_ip(parts.headers.get("x-real-ip"), false) {
        return ip;
    }
    if let Some(ConnectInfo(addr)) = parts.extensions.get::<ConnectInfo<SocketAddr>>() {
        return addr.ip().to_string();
    }
    "unknown".to_string()
}

fn header_ip(value: Option<&axum::http::HeaderValue>, first_hop: bool) -> Option<String> {
    let raw = value?.to_str().ok()?;
    let token = if first_hop {
        raw.split(',').next().unwrap_or("").trim()
    } else {
        raw.trim()
    };
    if token.is_empty() || token.len() > 128 {
        return None;
    }
    if token
        .bytes()
        .any(|b| b.is_ascii_whitespace() || matches!(b, b'/' | b'\\' | b'\0'))
    {
        return None;
    }
    Some(token.to_string())
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
    let json = wants_json(&headers);
    if !state.login_throttle.allow(&client_ip.0, now_secs()) {
        return too_many_logins(json);
    }
    if !state.auth.configured() {
        if json {
            return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "locked" }))).into_response();
        }
        return redirect_found("/login?locked=1").into_response();
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
        return redirect_found("/login?e=1").into_response();
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
    (
        [(header::SET_COOKIE, cookie)],
        redirect_found(&fields.next),
    )
        .into_response()
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
    (
        [(header::SET_COOKIE, cookie)],
        redirect_found("/login"),
    )
        .into_response()
}

fn require_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), Box<axum::response::Response>> {
    let ok = session_token(headers)
        .map(|token| state.auth.verify(&token, now_secs()))
        .unwrap_or(false);
    if ok {
        return Ok(());
    }
    let error = if state.auth.configured() {
        "unauthorized"
    } else {
        "locked"
    };
    Err(Box::new(
        (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": error })),
        )
            .into_response(),
    ))
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/me", get(auth_me))
        .route("/login", get(login_page).post(login_post))
        .route("/login.html", get(login_html_redirect))
        .route("/logout", get(logout).post(logout))
        .merge(flights::router().layer(DefaultBodyLimit::max(64 * 1024 * 1024)))
        .merge(video::router())
        .fallback(fallback)
        .with_state(state)
}

#[tokio::main]
async fn main() {
    let config = config_from_env();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let (state, queue_rx) = app_state(
        config.dist_dir,
        Auth::from_env(),
        Store::new(&config.data_dir),
    );
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
