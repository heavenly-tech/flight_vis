//! Android App Links: `/.well-known/assetlinks.json` (cacheable, no auth).
use axum::{
    http::{header, HeaderMap},
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde_json::{json, Value};

pub const ANDROID_PACKAGE: &str = "cl.heavenly.flightvis";

/// SHA-256 cert fingerprints for App Links (`ANDROID_CERT_SHA256` or `ANDROID_CERT_SHA256_FILE`).
pub fn android_cert_sha256_list() -> Vec<String> {
    if let Ok(raw) = std::env::var("ANDROID_CERT_SHA256") {
        let list: Vec<String> = raw
            .split(',')
            .map(|s| normalize_sha256(s.trim()))
            .filter(|s| !s.is_empty())
            .collect();
        if !list.is_empty() {
            return list;
        }
    }
    if let Ok(path) = std::env::var("ANDROID_CERT_SHA256_FILE") {
        if let Ok(body) = std::fs::read_to_string(&path) {
            let list: Vec<String> = body
                .lines()
                .map(|l| l.split('#').next().unwrap_or("").trim())
                .map(normalize_sha256)
                .filter(|s| !s.is_empty())
                .collect();
            if !list.is_empty() {
                return list;
            }
        }
    }
    Vec::new()
}

fn normalize_sha256(s: &str) -> String {
    s.trim()
        .trim_start_matches("sha256:")
        .trim_start_matches("SHA256:")
        .to_uppercase()
}

pub fn assetlinks_json(certs: &[String]) -> Value {
    if certs.is_empty() {
        return json!([]);
    }
    json!([{
        "relation": ["delegate_permission/common.handle_all_urls"],
        "target": {
            "namespace": "android_app",
            "package_name": ANDROID_PACKAGE,
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
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        Json(assetlinks_json(&certs)),
    )
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

use axum::http::StatusCode;

pub fn well_known_routes() -> Router<crate::AppState> {
    Router::new()
        .route("/.well-known/assetlinks.json", get(assetlinks_handler))
        .route("/.well-known/apple-app-site-association", get(aasa_placeholder))
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

pub fn intent_url_for_https(https_url: &str, package: &str) -> String {
    let fallback = pct_encode(https_url);
    format!(
        "intent://{host_path}#Intent;scheme=https;package={package};S.browser_fallback_url={fallback};end",
        host_path = https_url
            .trim_start_matches("https://")
            .trim_start_matches("http://"),
        package = package,
        fallback = fallback
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assetlinks_shape_with_fingerprint() {
        let j = assetlinks_json(&["AA:BB:CC".into()]);
        let arr = j.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let target = &arr[0]["target"];
        assert_eq!(target["package_name"], ANDROID_PACKAGE);
        let fps = target["sha256_cert_fingerprints"].as_array().unwrap();
        assert_eq!(fps[0], "AA:BB:CC");
    }

    #[test]
    fn assetlinks_empty_when_no_certs() {
        assert_eq!(assetlinks_json(&[]), json!([]));
    }

    #[test]
    fn reads_android_cert_from_env() {
        let _guard = env_lock();
        std::env::set_var("ANDROID_CERT_SHA256", "aa:bb:cc, dd:ee:ff");
        let list = android_cert_sha256_list();
        assert_eq!(list, vec!["AA:BB:CC".to_string(), "DD:EE:FF".to_string()]);
        std::env::remove_var("ANDROID_CERT_SHA256");
    }

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
}