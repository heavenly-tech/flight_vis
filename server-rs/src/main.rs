mod auth;
mod static_files;

use auth::Auth;
use axum::{
    body::Bytes,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde_json::json;
use std::{collections::HashMap, net::SocketAddr, path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

#[derive(Clone)]
struct AppState {
    dist_dir: PathBuf,
    auth: Auth,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn config_from_env() -> (AppState, u16) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let raw_dist = std::env::var("FLIGHT_VIS_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("../dist"));
    // Normalize (resolves the `..`) so prefix checks behave; fall back to the
    // raw path when the directory does not exist yet (all reads then 404/500).
    let dist_dir = std::fs::canonicalize(&raw_dist).unwrap_or(raw_dist);
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(8080);
    (
        AppState {
            dist_dir,
            auth: Auth::from_env(),
        },
        port,
    )
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
    for candidate in [
        dist_dir.join("login.html"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist/login.html"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../login.html"),
    ] {
        if let Ok(html) = tokio::fs::read_to_string(&candidate).await {
            return html;
        }
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

async fn login_post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let json = wants_json(&headers);
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
) -> Result<(), axum::response::Response> {
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
    Err((
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": error })),
    )
        .into_response())
}

#[tokio::main]
async fn main() {
    let (state, port) = config_from_env();
    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/me", get(auth_me))
        .route("/login", get(login_page).post(login_post))
        .route("/login.html", get(login_html_redirect))
        .route("/logout", get(logout).post(logout))
        .fallback(fallback)
        .with_state(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    eprintln!("flight_vis (rust) listening on {port}");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind listener");
    axum::serve(listener, app).await.expect("serve app");
}
