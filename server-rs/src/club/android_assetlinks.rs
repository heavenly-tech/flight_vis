//! Android App Links: `/.well-known/assetlinks.json` (cacheable, no auth).
#![allow(dead_code)]
use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Mutex;

static ANDROID_TEST_ENV_LOCK: Mutex<()> = Mutex::new(());

/// Serializes tests that mutate Android deep-link environment variables.
#[cfg(test)]
pub(crate) fn android_test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    ANDROID_TEST_ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

pub const DEFAULT_ANDROID_PACKAGE: &str = "cl.heavenly.flightvis";

/// `FLIGHT_ANDROID_APP_PACKAGE` overrides the default `cl.heavenly.flightvis`.
pub fn android_package_name() -> String {
    android_package_name_from_env().unwrap_or_else(|| DEFAULT_ANDROID_PACKAGE.to_string())
}

fn android_package_name_from_env() -> Option<String> {
    for key in ["FLIGHT_ANDROID_APP_PACKAGE", "ANDROID_PACKAGE"] {
        if let Ok(raw) = std::env::var(key) {
            let s = raw.trim();
            if s.is_empty() {
                continue;
            }
            if is_valid_java_package(s) {
                return Some(s.to_string());
            }
            eprintln!("flight_vis: invalid {key} `{s}` (not a Java package name), using default");
        }
    }
    None
}

fn is_valid_java_package(name: &str) -> bool {
    let mut parts = name.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    if !is_valid_java_identifier(first) {
        return false;
    }
    for part in parts {
        if !is_valid_java_identifier(part) {
            return false;
        }
    }
    name.contains('.')
}

fn is_valid_java_identifier(segment: &str) -> bool {
    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// SHA-256 cert fingerprints (`FLIGHT_ANDROID_CERT_SHA256` / file, or legacy `ANDROID_*`).
pub fn android_cert_sha256_list() -> Vec<String> {
    if let Some(list) = certs_from_env("FLIGHT_ANDROID_CERT_SHA256") {
        return list;
    }
    if let Some(path) = std::env::var("FLIGHT_ANDROID_CERT_SHA256_FILE")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        if let Ok(list) = certs_from_file(&path) {
            if !list.is_empty() {
                return list;
            }
        }
    }
    if let Some(list) = certs_from_env("ANDROID_CERT_SHA256") {
        return list;
    }
    if let Ok(path) = std::env::var("ANDROID_CERT_SHA256_FILE") {
        if let Ok(list) = certs_from_file(&path) {
            if !list.is_empty() {
                return list;
            }
        }
    }
    Vec::new()
}

fn certs_from_env(key: &str) -> Option<Vec<String>> {
    let raw = std::env::var(key).ok()?;
    let list: Vec<String> = raw
        .split(',')
        .filter_map(|s| normalize_sha256(s.trim()))
        .collect();
    if list.is_empty() {
        None
    } else {
        Some(list)
    }
}

fn certs_from_file(path: &str) -> std::io::Result<Vec<String>> {
    let body = std::fs::read_to_string(path)?;
    Ok(body
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter_map(normalize_sha256)
        .collect())
}

fn normalize_sha256(s: &str) -> Option<String> {
    let s = s
        .trim()
        .trim_start_matches("sha256:")
        .trim_start_matches("SHA256:");
    if s.is_empty() {
        return None;
    }
    if is_colon_sha256(s) {
        return Some(s.to_uppercase());
    }
    if is_bare_sha256(s) {
        let upper = s.to_uppercase();
        let mut out = String::with_capacity(95);
        for (i, chunk) in upper.as_bytes().chunks(2).enumerate() {
            if i > 0 {
                out.push(':');
            }
            out.push(chunk[0] as char);
            out.push(chunk[1] as char);
        }
        return Some(out);
    }
    eprintln!("flight_vis: invalid Android cert fingerprint, dropped: {s}");
    None
}

fn is_colon_sha256(s: &str) -> bool {
    let parts: Vec<&str> = s.split(':').collect();
    parts.len() == 32
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn is_bare_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn android_deep_link_configured() -> bool {
    !android_cert_sha256_list().is_empty()
}

pub fn assetlinks_json(package: &str, certs: &[String]) -> Value {
    json!([{
        "relation": ["delegate_permission/common.handle_all_urls"],
        "target": {
            "namespace": "android_app",
            "package_name": package,
            "sha256_cert_fingerprints": certs
        }
    }])
}

/// Placeholder for future iOS Universal Links (not required for this phase).
pub fn apple_app_site_association_placeholder() -> Value {
    json!({
        "applinks": {
            "apps": [],
            "details": []
        },
        "note": "Universal Links configuration pending"
    })
}

async fn assetlinks_handler() -> impl IntoResponse {
    let certs = android_cert_sha256_list();
    if certs.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let package = android_package_name();
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        Json(assetlinks_json(&package, &certs)),
    )
        .into_response()
}

async fn aasa_placeholder() -> impl IntoResponse {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        Json(apple_app_site_association_placeholder()),
    )
}

async fn app_install_link(
    State(state): State<crate::AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let release = read_apk_path(&state.dist_dir).await;
    let apk_abs = club_public_https_url(&state, &headers, &release.apk_path);
    let https_app = club_public_https_url(&state, &headers, "/app.html");
    let intent_url = android_deep_link_configured()
        .then(|| launcher_intent_url(&android_package_name(), &apk_abs));
    (
        StatusCode::OK,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::CONTENT_TYPE, "application/json"),
        ],
        Json(json!({
            "httpsUrl": https_app,
            "apkUrl": apk_abs,
            "intentUrl": intent_url,
        })),
    )
}

struct ApkPath {
    apk_path: String,
}

async fn read_apk_path(dist: &Path) -> ApkPath {
    let path = dist.join("app/app-release.json");
    let default_apk = "/app/flightvis.apk".to_string();
    let Ok(body) = tokio::fs::read_to_string(&path).await else {
        return ApkPath {
            apk_path: default_apk,
        };
    };
    let v: Value = serde_json::from_str(&body).unwrap_or(json!({}));
    let apk_url = v
        .get("apkUrl")
        .and_then(|s| s.as_str())
        .unwrap_or("/app/flightvis.apk")
        .to_string();
    ApkPath { apk_path: apk_url }
}

pub fn well_known_routes() -> Router<crate::AppState> {
    Router::new()
        .route("/.well-known/assetlinks.json", get(assetlinks_handler))
        .route(
            "/.well-known/apple-app-site-association",
            get(aasa_placeholder),
        )
        .route("/api/app/install-link", get(app_install_link))
}

pub fn absolute_https_url(headers: &HeaderMap, path: &str) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .filter(|p| *p == "https")
        .unwrap_or(if host.contains("localhost") || host.starts_with("127.") {
            "http"
        } else {
            "https"
        });
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("{proto}://{host}{path}")
}

/// Prefer `FLIGHT_CLUB_PUBLIC_URL` (via mailer) over the request `Host` header.
pub fn club_public_https_url(state: &crate::AppState, headers: &HeaderMap, path: &str) -> String {
    let base = state.mailer.public_url.trim_end_matches('/');
    if base.starts_with("http://") || base.starts_with("https://") {
        let path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        return format!("{base}{path}");
    }
    absolute_https_url(headers, path)
}

fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Chrome custom-tab intent: launch the installed APK, else download via `browser_fallback_url`.
pub fn launcher_intent_url(package: &str, browser_fallback_url: &str) -> String {
    let package_enc = pct_encode(package);
    let fallback = pct_encode(browser_fallback_url);
    format!(
        "intent:#Intent;action=android.intent.action.MAIN;category=android.intent.category.LAUNCHER;package={package_enc};S.browser_fallback_url={fallback};end"
    )
}

/// Payload encoded in club check-in QR symbols: always a plain HTTPS App Link URL (never `intent://`).
pub fn checkin_qr_https_payload(
    state: &crate::AppState,
    headers: &HeaderMap,
    https_path: &str,
) -> String {
    club_public_https_url(state, headers, https_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use serial_test::serial;
    use tower::ServiceExt;

    const VALID_FP: &str =
        "00:01:02:03:04:05:06:07:08:09:0A:0B:0C:0D:0E:0F:10:11:12:13:14:15:16:17:18:19:1A:1B:1C:1D:1E:1F";

    #[test]
    fn normalize_accepts_colon_and_bare_hex() {
        let bare = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
        let norm = normalize_sha256(bare).unwrap();
        assert_eq!(norm.len(), 95);
        assert!(norm.contains(':'));
        assert_eq!(normalize_sha256(VALID_FP).unwrap(), VALID_FP.to_string());
    }

    #[test]
    fn normalize_rejects_garbage() {
        assert!(normalize_sha256("not-a-fingerprint").is_none());
        assert!(normalize_sha256("AA:BB:CC").is_none());
    }

    #[test]
    fn java_package_validation() {
        assert!(is_valid_java_package("cl.heavenly.flightvis"));
        assert!(!is_valid_java_package("cl..heavenly"));
        assert!(!is_valid_java_package("1bad"));
    }

    #[test]
    fn assetlinks_shape_with_fingerprint() {
        let j = assetlinks_json("cl.heavenly.flightvis", &[VALID_FP.into()]);
        let arr = j.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let target = &arr[0]["target"];
        assert_eq!(target["package_name"], "cl.heavenly.flightvis");
        let fps = target["sha256_cert_fingerprints"].as_array().unwrap();
        assert_eq!(fps[0], VALID_FP);
    }

    #[test]
    fn launcher_intent_exact_format() {
        let intent = launcher_intent_url(
            "cl.heavenly.flightvis",
            "https://flight.heavenly.cl/app/flightvis.apk",
        );
        assert!(intent.starts_with(
            "intent:#Intent;action=android.intent.action.MAIN;category=android.intent.category.LAUNCHER;"
        ));
        assert!(intent.contains("package=cl.heavenly.flightvis"));
        assert!(intent.contains(
            "S.browser_fallback_url=https%3A%2F%2Fflight.heavenly.cl%2Fapp%2Fflightvis.apk"
        ));
        assert!(intent.ends_with(";end"));
    }

    #[test]
    fn checkin_qr_payload_uses_public_url_not_host() {
        let _guard = super::android_test_env_lock();
        std::env::set_var("FLIGHT_CLUB_PUBLIC_URL", "https://flight.heavenly.cl");
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "evil.example.com".parse().unwrap());
        let dir = tempfile::tempdir().unwrap();
        let state = crate::AppState {
            dist_dir: dir.path().to_path_buf(),
            auth: crate::auth::Auth::with_password("x"),
            device_auth: crate::device_auth::DeviceAuth::empty(),
            live: std::sync::Arc::new(crate::live::LiveEngine::new(dir.path())),
            store: crate::store::Store::new(dir.path()),
            club: None,
            video_queue: tokio::sync::mpsc::channel(1).0,
            login_throttle: crate::LoginThrottle::default(),
            device_register_throttle: crate::LoginThrottle::default(),
            share_lookup_throttle: crate::LoginThrottle::default(),
            account_password_throttle: crate::LoginThrottle::default(),
            account_password_account_throttle: crate::LoginThrottle::default(),
            mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
            webauthn: None,
            sse_limiter: std::sync::Arc::new(crate::club::sse_limit::SseLimiter::default()),
            superadmin_emails: Default::default(),
        };
        let payload = checkin_qr_https_payload(&state, &headers, "/c/demo/q/v1.demo.1.sig");
        assert!(payload.starts_with("https://flight.heavenly.cl/c/demo/q/"));
        assert!(!payload.contains("intent:"));
        std::env::remove_var("FLIGHT_CLUB_PUBLIC_URL");
    }

    #[test]
    fn reads_flight_android_cert_from_env() {
        let _guard = super::android_test_env_lock();
        std::env::remove_var("FLIGHT_ANDROID_CERT_SHA256");
        std::env::remove_var("ANDROID_CERT_SHA256");
        let fp =
            "00:01:02:03:04:05:06:07:08:09:0A:0B:0C:0D:0E:0F:10:11:12:13:14:15:16:17:18:19:1A:1B:1C:1D:1E:1F";
        let fp2 =
            "10:11:12:13:14:15:16:17:18:19:1A:1B:1C:1D:1E:1F:20:21:22:23:24:25:26:27:28:29:2A:2B:2C:2D:2E:2F";
        std::env::set_var("FLIGHT_ANDROID_CERT_SHA256", format!("{fp}, {fp2}"));
        let list = android_cert_sha256_list();
        assert_eq!(list, vec![fp.to_string(), fp2.to_string()]);
        std::env::remove_var("FLIGHT_ANDROID_CERT_SHA256");
    }

    #[test]
    fn default_package_without_env() {
        let _guard = super::android_test_env_lock();
        std::env::remove_var("FLIGHT_ANDROID_APP_PACKAGE");
        std::env::remove_var("ANDROID_PACKAGE");
        assert_eq!(android_package_name(), DEFAULT_ANDROID_PACKAGE);
    }

    #[tokio::test]
    #[serial]
    #[allow(clippy::await_holding_lock)]
    async fn assetlinks_http_404_when_no_valid_fingerprints() {
        let _guard = super::android_test_env_lock();
        std::env::remove_var("FLIGHT_ANDROID_CERT_SHA256");
        std::env::remove_var("ANDROID_CERT_SHA256");
        let app = Router::new().route("/.well-known/assetlinks.json", get(assetlinks_handler));
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/assetlinks.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    #[serial]
    #[allow(clippy::await_holding_lock)]
    async fn assetlinks_http_json_when_fingerprints_configured() {
        let _guard = super::android_test_env_lock();
        std::env::set_var(
            "FLIGHT_ANDROID_CERT_SHA256",
            "00:01:02:03:04:05:06:07:08:09:0A:0B:0C:0D:0E:0F:10:11:12:13:14:15:16:17:18:19:1A:1B:1C:1D:1E:1F",
        );
        let app = Router::new().route("/.well-known/assetlinks.json", get(assetlinks_handler));
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/.well-known/assetlinks.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok()),
            Some("application/json")
        );
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let j: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(j[0]["target"]["package_name"], DEFAULT_ANDROID_PACKAGE);
        std::env::remove_var("FLIGHT_ANDROID_CERT_SHA256");
    }
}
