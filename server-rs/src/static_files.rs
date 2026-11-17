//! Static file serving with byte-parity semantics to the Node implementation:
//! path-traversal guard, MIME map, SPA fallback to index.html, and
//! cache-control rules (immutable hashed assets, no-cache HTML entries).
use axum::{
    body::Body,
    http::{header, HeaderMap, StatusCode},
};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::fs::File;
use tokio::io::AsyncReadExt;
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
        "apk" => "application/vnd.android.package-archive",
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

fn is_geoid_asset(relative: &str) -> bool {
    relative.starts_with("geoid/")
}

fn ext_of(relative: &str) -> &str {
    relative.rsplit('.').next().unwrap_or("")
}

fn accepts_encoding(headers: &HeaderMap) -> &'static str {
    headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase())
        .map(|v| {
            if v.contains("br") {
                "br"
            } else if v.contains("gzip") {
                "gzip"
            } else {
                "identity"
            }
        })
        .unwrap_or("identity")
}

fn sidecar_path(base: &Path, encoding: &str) -> PathBuf {
    let suffix = match encoding {
        "br" => ".br",
        "gzip" => ".gz",
        _ => return base.to_path_buf(),
    };
    PathBuf::from(format!("{}{}", base.to_string_lossy(), suffix))
}

pub const HTML_CSP: &str =
    "default-src 'self'; img-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'";

fn cache_for(relative: &str) -> &'static str {
    if is_geoid_asset(relative) {
        "public, max-age=31536000"
    } else if is_hashed_asset(relative) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

async fn file_etag(path: &Path) -> Result<String, StatusCode> {
    let mut file = File::open(path).await.map_err(|_| StatusCode::NOT_FOUND)?;
    let meta = file.metadata().await.map_err(|_| StatusCode::NOT_FOUND)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    let len = meta.len();
    Ok(format!("\"{digest:x}-{len}\""))
}

fn header_str(headers: &HeaderMap, name: header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

struct FileResponseOpts<'a> {
    path: &'a Path,
    content_type: &'a str,
    cache: &'a str,
    etag: &'a str,
    content_encoding: Option<&'a str>,
    vary_encoding: bool,
    not_modified: bool,
    sized_body: Option<Vec<u8>>,
}

async fn file_response(opts: FileResponseOpts<'_>) -> Result<axum::response::Response, StatusCode> {
    if opts.not_modified {
        let mut builder = axum::response::Response::builder().status(StatusCode::NOT_MODIFIED);
        builder = builder
            .header(header::CACHE_CONTROL, opts.cache)
            .header(header::ETAG, opts.etag);
        if opts.vary_encoding {
            builder = builder.header(header::VARY, "Accept-Encoding");
        }
        return builder
            .body(Body::empty())
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR);
    }

    let (body, content_length) = if let Some(bytes) = opts.sized_body {
        let len = bytes.len();
        (Body::from(bytes), Some(len))
    } else {
        let file = File::open(opts.path)
            .await
            .map_err(|_| StatusCode::NOT_FOUND)?;
        (Body::from_stream(ReaderStream::new(file)), None)
    };
    let mut builder = axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, opts.content_type)
        .header(header::CACHE_CONTROL, opts.cache)
        .header(header::ETAG, opts.etag);
    if opts.content_type.starts_with("text/html") {
        builder = builder.header(header::CONTENT_SECURITY_POLICY, HTML_CSP);
    }
    if let Some(len) = content_length {
        builder = builder.header(header::CONTENT_LENGTH, len.to_string());
    }
    if let Some(enc) = opts.content_encoding {
        builder = builder.header(header::CONTENT_ENCODING, enc);
    }
    if opts.vary_encoding {
        builder = builder.header(header::VARY, "Accept-Encoding");
    }
    builder
        .body(body)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

pub async fn serve(
    dist_dir: &Path,
    url_path: &str,
    headers: &HeaderMap,
) -> Result<axum::response::Response, StatusCode> {
    if let Some(joined) = safe_join(dist_dir, url_path) {
        if tokio::fs::metadata(&joined)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            let relative = joined
                .strip_prefix(dist_dir)
                .ok()
                .and_then(|p| p.to_str())
                .unwrap_or("");
            let cache = cache_for(relative);
            let geoid = is_geoid_asset(relative);
            let encoding = accepts_encoding(headers);
            let mut serve_path = joined.clone();
            let mut content_encoding: Option<&str> = None;
            let vary = geoid;
            if geoid && encoding != "identity" {
                let sidecar = sidecar_path(&joined, encoding);
                if tokio::fs::metadata(&sidecar)
                    .await
                    .map(|m| m.is_file())
                    .unwrap_or(false)
                {
                    serve_path = sidecar;
                    content_encoding = Some(if encoding == "br" { "br" } else { "gzip" });
                }
            }
            let etag = file_etag(&joined).await?;
            let if_none_match = header_str(headers, header::IF_NONE_MATCH);
            let not_modified = if_none_match == Some(etag.as_str());
            let sized_body = if geoid && !not_modified {
                Some(
                    tokio::fs::read(&serve_path)
                        .await
                        .map_err(|_| StatusCode::NOT_FOUND)?,
                )
            } else {
                None
            };
            return file_response(FileResponseOpts {
                path: &serve_path,
                content_type: mime_for(&ext_of(relative).to_lowercase()),
                cache,
                etag: &etag,
                content_encoding,
                vary_encoding: vary,
                not_modified,
                sized_body,
            })
            .await;
        }
    }
    let index = dist_dir.join("index.html");
    if tokio::fs::metadata(&index)
        .await
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        let etag = file_etag(&index).await?;
        let if_none_match = header_str(headers, header::IF_NONE_MATCH);
        return file_response(FileResponseOpts {
            path: &index,
            content_type: "text/html; charset=UTF-8",
            cache: "no-cache",
            etag: &etag,
            content_encoding: None,
            vary_encoding: false,
            not_modified: if_none_match == Some(etag.as_str()),
            sized_body: None,
        })
        .await;
    }
    Err(StatusCode::INTERNAL_SERVER_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn temp_dist() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "flight-vis-static-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("geoid")).unwrap();
        dir
    }

    #[test]
    fn join_resolves_subpaths() {
        let dir = temp_dist();
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("login.html"), "login").unwrap();
        std::fs::write(dir.join("assets/app.js"), "js").unwrap();

        let login = safe_join(&dir, "/login.html").expect("join must succeed");
        assert_eq!(login, dir.join("login.html"));
        assert!(login.is_file());

        let asset = safe_join(&dir, "/assets/app.js").expect("subpath must resolve");
        assert_eq!(asset, dir.join("assets/app.js"));
        assert!(asset.is_file());

        let _ = std::fs::remove_dir_all(&dir);
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

    #[tokio::test]
    async fn geoid_serves_cache_headers_and_br_encoding() {
        let dir = temp_dist();
        let bin = dir.join("geoid/egm96-025deg.bin");
        let payload = vec![0u8; 128];
        std::fs::write(&bin, &payload).unwrap();
        std::fs::write(bin.to_string_lossy().to_string() + ".br", b"br-payload").unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static("br, gzip"),
        );
        let res = serve(&dir, "/geoid/egm96-025deg.bin", &headers)
            .await
            .expect("geoid bin");
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("public, max-age=31536000")
        );
        assert!(res.headers().get(header::ETAG).is_some());
        assert_eq!(
            res.headers()
                .get(header::CONTENT_ENCODING)
                .and_then(|v| v.to_str().ok()),
            Some("br")
        );
        assert_eq!(
            res.headers()
                .get(header::VARY)
                .and_then(|v| v.to_str().ok()),
            Some("Accept-Encoding")
        );
        assert_eq!(
            res.headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok()),
            Some("10")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn geoid_identity_sets_vary_and_content_length() {
        let dir = temp_dist();
        let bin = dir.join("geoid/grid.bin");
        std::fs::write(&bin, b"12345").unwrap();
        let res = serve(&dir, "/geoid/grid.bin", &HeaderMap::new())
            .await
            .expect("geoid");
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::VARY)
                .and_then(|v| v.to_str().ok()),
            Some("Accept-Encoding")
        );
        assert_eq!(
            res.headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok()),
            Some("5")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn geoid_returns_304_when_etag_matches() {
        let dir = temp_dist();
        let bin = dir.join("geoid/grid.bin");
        std::fs::write(&bin, b"abc").unwrap();
        let etag = file_etag(&bin).await.unwrap();

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, HeaderValue::from_str(&etag).unwrap());
        let res = serve(&dir, "/geoid/grid.bin", &headers)
            .await
            .expect("geoid");
        assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            res.headers()
                .get(header::ETAG)
                .and_then(|v| v.to_str().ok()),
            Some(etag.as_str())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
