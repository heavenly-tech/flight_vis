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

/// `FLIGHT_ANDROID_APP_PACKAGE` (unset unless configured; no server default package name).
pub fn android_package_name() -> Option<String> {
    package_from_env("FLIGHT_ANDROID_APP_PACKAGE").or_else(|| package_from_env("ANDROID_PACKAGE"))
}

fn package_from_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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
        .map(|s| normalize_sha256(s.trim()))
        .filter(|s| !s.is_empty())
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
        .map(normalize_sha256)
        .filter(|s| !s.is_empty())
        .collect())
}

fn normalize_sha256(s: &str) -> String {
    s.trim()
        .trim_start_matches("sha256:")
        .trim_start_matches("SHA256:")
        .to_uppercase()
}

pub fn android_deep_link_configured() -> bool {
    android_package_name().is_some() && !android_cert_sha256_list().is_empty()
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
    let Some(package) = android_package_name() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let certs = android_cert_sha256_list();
    if certs.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }
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
    let apk_abs = absolute_https_url(&headers, &release.apk_path);
    let https_app = absolute_https_url(&headers, "/app.html");
    let intent_url = android_package_name().and_then(|pkg| {
        if !android_deep_link_configured() {
            return None;
        }
        Some(intent_url_for_https(&https_app, &pkg, &apk_abs))
    });
    Json(json!({
        "httpsUrl": https_app,
        "apkUrl": apk_abs,
        "intentUrl": intent_url,
    }))
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

/// Chrome custom-tab intent: open `https_url` in the app, else download/install via `browser_fallback_url`.
pub fn intent_url_for_https(https_url: &str, package: &str, browser_fallback_url: &str) -> String {
    let fallback = pct_encode(browser_fallback_url);
    format!(
        "intent://{host_path}#Intent;scheme=https;package={package};S.browser_fallback_url={fallback};end",
        host_path = https_url
            .trim_start_matches("https://")
            .trim_start_matches("http://"),
        package = package,
        fallback = fallback
    )
}

/// Payload encoded in club check-in QR symbols: always a plain HTTPS App Link URL (never `intent://`).
pub fn checkin_qr_https_payload(headers: &HeaderMap, https_path: &str) -> String {
    absolute_https_url(headers, https_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use serial_test::serial;
    use tower::ServiceExt;

    #[test]
    fn assetlinks_shape_with_fingerprint() {
        let j = assetlinks_json("cl.heavenly.flightvis", &["AA:BB:CC".into()]);
        let arr = j.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let target = &arr[0]["target"];
        assert_eq!(target["package_name"], "cl.heavenly.flightvis");
        let fps = target["sha256_cert_fingerprints"].as_array().unwrap();
        assert_eq!(fps[0], "AA:BB:CC");
    }

    #[test]
    fn intent_includes_apk_fallback() {
        let intent = intent_url_for_https(
            "https://flight.heavenly.cl/app.html",
            "cl.heavenly.flightvis",
            "https://flight.heavenly.cl/app/flightvis.apk",
        );
        assert!(intent.contains("S.browser_fallback_url="));
        assert!(intent.contains("%3A%2F%2F"));
        assert!(intent.contains("package=cl.heavenly.flightvis"));
    }

    #[test]
    fn checkin_qr_payload_is_https_not_intent_even_when_app_links_configured() {
        let _guard = super::android_test_env_lock();
        std::env::set_var("FLIGHT_ANDROID_APP_PACKAGE", "cl.heavenly.flightvis");
        std::env::set_var("FLIGHT_ANDROID_CERT_SHA256", "AA:BB:CC");
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "flight.heavenly.cl".parse().unwrap());
        let payload = checkin_qr_https_payload(&headers, "/c/demo/q/v1.demo.1.sig");
        assert!(payload.starts_with("https://flight.heavenly.cl/c/demo/q/"));
        assert!(!payload.contains("intent:"));
        std::env::remove_var("FLIGHT_ANDROID_APP_PACKAGE");
        std::env::remove_var("FLIGHT_ANDROID_CERT_SHA256");
    }

    #[test]
    fn reads_flight_android_cert_from_env() {
        let _guard = super::android_test_env_lock();
        std::env::remove_var("FLIGHT_ANDROID_CERT_SHA256");
        std::env::remove_var("ANDROID_CERT_SHA256");
        std::env::set_var("FLIGHT_ANDROID_CERT_SHA256", "aa:bb:cc, dd:ee:ff");
        let list = android_cert_sha256_list();
        assert_eq!(list, vec!["AA:BB:CC".to_string(), "DD:EE:FF".to_string()]);
        std::env::remove_var("FLIGHT_ANDROID_CERT_SHA256");
    }

    #[tokio::test]
    #[serial]
    #[allow(clippy::await_holding_lock)]
    async fn assetlinks_http_404_when_unconfigured() {
        let _guard = super::android_test_env_lock();
        std::env::remove_var("FLIGHT_ANDROID_APP_PACKAGE");
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
    async fn assetlinks_http_json_when_configured() {
        let _guard = super::android_test_env_lock();
        std::env::set_var("FLIGHT_ANDROID_APP_PACKAGE", "cl.heavenly.flightvis");
        std::env::set_var("FLIGHT_ANDROID_CERT_SHA256", "AA:BB:CC");
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
        assert_eq!(j[0]["target"]["package_name"], "cl.heavenly.flightvis");
        std::env::remove_var("FLIGHT_ANDROID_APP_PACKAGE");
        std::env::remove_var("FLIGHT_ANDROID_CERT_SHA256");
    }
}
