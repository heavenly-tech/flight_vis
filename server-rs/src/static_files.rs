//! Static file serving with byte-parity semantics to the Node implementation:
//! path-traversal guard, MIME map, SPA fallback to index.html, and
//! cache-control rules (immutable hashed assets, no-cache HTML entries).
use axum::{
    body::Body,
    http::{header, StatusCode},
};
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio_util::io::ReaderStream;

fn mime_for(ext: &str) -> &'static str {
    match ext {
        "css" => "text/css; charset=utf-8",
        "gpx" => "application/gpx+xml",
        "html" => "text/html; charset=utf-8",
        "ico" => "image/x-icon",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Mirror of the Node guard: strip leading slashes, drop `.` segments, and
/// refuse to serve anything escaping the directory (any `..` segment).
fn safe_join(dist_dir: &Path, url_path: &str) -> Option<PathBuf> {
    let trimmed = url_path.trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let mut joined = PathBuf::from(dist_dir);
    for part in trimmed.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return None;
        }
        joined.push(part);
    }
    // mirrors the Node startsWith(distDir) containment check
    if !joined.starts_with(dist_dir) {
        return None;
    }
    Some(joined)
}

/// Faithful port of the Node regex `/-[A-Za-z0-9_-]{6,}\.[a-z0-9]+$/`
/// (unanchored search): a dash, 6+ name chars, a dot, lowercase extension.
fn is_hashed_asset(relative: &str) -> bool {
    let bytes = relative.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'-' {
            let mut j = i + 1;
            while j < bytes.len()
                && matches!(bytes[j], b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-')
            {
                j += 1;
            }
            if j - (i + 1) >= 6 && j < bytes.len() && bytes[j] == b'.' {
                let mut k = j + 1;
                while k < bytes.len() && matches!(bytes[k], b'a'..=b'z' | b'0'..=b'9') {
                    k += 1;
                }
                if k - (j + 1) >= 1 && k == bytes.len() {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

fn ext_of(relative: &str) -> &str {
    relative.rsplit('.').next().unwrap_or("")
}

async fn file_response(path: &Path, content_type: &str, cache: &str) -> Result<axum::response::Response, StatusCode> {
    let file = File::open(path).await.map_err(|_| StatusCode::NOT_FOUND)?;
    let body = Body::from_stream(ReaderStream::new(file));
    Ok(axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, cache)
        .body(body)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?)
}

pub async fn serve(dist_dir: &Path, url_path: &str) -> Result<axum::response::Response, StatusCode> {
    if let Some(joined) = safe_join(dist_dir, url_path) {
        if tokio::fs::metadata(&joined).await.map(|m| m.is_file()).unwrap_or(false) {
            let relative = joined
                .strip_prefix(dist_dir)
                .ok()
                .and_then(|p| p.to_str())
                .unwrap_or("");
            let cache = if is_hashed_asset(relative) {
                "public, max-age=31536000, immutable"
            } else {
                "no-cache"
            };
            return file_response(&joined, mime_for(&ext_of(relative).to_lowercase()), cache).await;
        }
    }
    let index = dist_dir.join("index.html");
    if tokio::fs::metadata(&index).await.map(|m| m.is_file()).unwrap_or(false) {
        return file_response(&index, "text/html; charset=UTF-8", "no-cache").await;
    }
    Err(StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist")
    }

    #[test]
    fn join_resolves_subpaths() {
        let dist = dist();
        let joined = safe_join(&dist, "/login.html").expect("join must succeed");
        println!("joined={joined:?} exists={}", joined.is_file());
        assert!(joined.is_file());
    }

    #[test]
    fn hashed_asset_matches_node_regex() {
        assert!(is_hashed_asset("assets/main-B3-ZkIRo.js"));
        assert!(is_hashed_asset("assets/main-DsRi90xX.css"));
        assert!(!is_hashed_asset("index.html"));
        assert!(!is_hashed_asset("login.html"));
        assert!(!is_hashed_asset("cesiumStatic/Workers/worker.js"));
        assert!(!is_hashed_asset("a-b.js"));
        assert!(!is_hashed_asset("a-bcdefg.JS"));
    }
}
