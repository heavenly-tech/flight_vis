use axum::http::{header, HeaderMap, StatusCode};

/// `Content-Type: application/json` with optional parameters (e.g. charset).
pub fn exact_json_content_type(headers: &HeaderMap) -> Result<(), StatusCode> {
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if crate::auth::media_type_is_json(ct) {
        Ok(())
    } else {
        Err(StatusCode::UNSUPPORTED_MEDIA_TYPE)
    }
}

/// Guards cookie-authenticated POST mutations from cross-site form posts.
pub(crate) fn cookie_post_allowed(headers: &HeaderMap) -> Result<(), StatusCode> {
    if let Some(site) = headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) {
        return match site {
            "same-origin" | "same-site" => Ok(()),
            _ => Err(StatusCode::FORBIDDEN),
        };
    }
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if host.is_empty() {
            return Ok(());
        }
        let origin_host = origin
            .strip_prefix("https://")
            .or_else(|| origin.strip_prefix("http://"))
            .and_then(|rest| rest.split('/').next());
        if let Some(oh) = origin_host {
            let expect = host.split(':').next().unwrap_or(host);
            if oh.eq_ignore_ascii_case(expect) {
                return Ok(());
            }
        }
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}
