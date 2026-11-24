//! Redirects from historical club slugs (single hop to current slug; not cacheable).

use crate::auth::percent_decode_query_value;
use crate::club::db::ClubDb;
use crate::club::slug::normalize_slug;
use crate::static_files::normalize_url_path;
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};

fn append_query(path: &str, query: Option<&str>) -> String {
    match query.filter(|q| !q.is_empty()) {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    }
}

/// Build a same-origin relative Location (exactly one leading `/`, never `//` or `/\`).
fn safe_redirect_target(path: &str, query: Option<&str>) -> Option<String> {
    if path.contains('\\') {
        return None;
    }
    let (path_only, extra_q) = path
        .split_once('?')
        .map_or((path, None), |(p, q)| (p, Some(q)));
    if path_only.starts_with("//") {
        return None;
    }
    let normalized = normalize_url_path(path_only)?;
    if !normalized.starts_with('/') {
        return None;
    }
    let merged_q = match (query, extra_q) {
        (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => Some(format!("{a}&{b}")),
        (Some(a), _) if !a.is_empty() => Some(a.to_string()),
        (_, Some(b)) if !b.is_empty() => Some(b.to_string()),
        _ => None,
    };
    Some(append_query(&normalized, merged_q.as_deref()))
}

fn club_param_raw(query: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        if pair.is_empty() {
            return None;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        if k == "club" {
            Some(v.to_string())
        } else {
            None
        }
    })
}

fn rewrite_query_club_param(query: &str, old: &str, new: &str) -> Option<String> {
    let mut changed = false;
    let parts: Vec<String> = query
        .split('&')
        .filter_map(|pair| {
            if pair.is_empty() {
                return None;
            }
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            if k == "club" {
                let decoded = percent_decode_query_value(v);
                let norm = normalize_slug(&decoded);
                if norm == old {
                    changed = true;
                    Some(format!("club={new}"))
                } else {
                    Some(pair.to_string())
                }
            } else {
                Some(pair.to_string())
            }
        })
        .collect();
    if !changed {
        return None;
    }
    Some(parts.join("&"))
}

fn replace_slug_in_path(normalized: &str, segment_index: usize, new_slug: &str) -> Option<String> {
    let mut parts: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if segment_index >= parts.len() {
        return None;
    }
    parts[segment_index] = new_slug;
    safe_redirect_target(&format!("/{}", parts.join("/")), None)
}

pub fn slug_history_redirect(method: &str, location: &str) -> Response {
    let status = if method == "GET" || method == "HEAD" {
        StatusCode::FOUND
    } else {
        StatusCode::TEMPORARY_REDIRECT
    };
    let cache = HeaderValue::from_static("no-store");
    (
        status,
        [
            (
                header::LOCATION,
                HeaderValue::from_str(location).unwrap_or_else(|_| HeaderValue::from_static("/")),
            ),
            (header::CACHE_CONTROL, cache),
        ],
    )
        .into_response()
}

/// When the path or `club=` query uses a historical slug, redirect once to the current slug.
pub async fn maybe_club_slug_redirect(
    db: &ClubDb,
    method: &str,
    path: &str,
    query: Option<&str>,
) -> Option<Response> {
    let is_get_head = method == "GET" || method == "HEAD";
    if is_get_head {
        if let Some(q) = query {
            if let Some(raw) = club_param_raw(q) {
                let norm = normalize_slug(&percent_decode_query_value(&raw));
                if !norm.is_empty() {
                    if let Some(current) = db.current_slug_for_historical(&norm).await {
                        let new_q = rewrite_query_club_param(q, &norm, &current)?;
                        let target = safe_redirect_target(path, Some(&new_q))?;
                        return Some(slug_history_redirect(method, &target));
                    }
                }
            }
        }
    }
    let normalized = normalize_url_path(path)?;
    let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    let (slug_idx, old_slug) = if segments.len() >= 2 && segments[0] == "c" {
        (1, segments[1])
    } else if segments.len() >= 3
        && segments[0] == "api"
        && (segments[1] == "clubs" || segments[1] == "c")
    {
        (2, segments[2])
    } else {
        return None;
    };
    let current = db.current_slug_for_historical(old_slug).await?;
    let new_path = replace_slug_in_path(&normalized, slug_idx, &current)?;
    let target = safe_redirect_target(&new_path, query)?;
    Some(slug_history_redirect(method, &target))
}

/// Convenience for handlers that already extracted `slug` from the path.
pub async fn redirect_if_historical_slug(
    db: &ClubDb,
    method: &str,
    path: &str,
    query: Option<&str>,
) -> Option<Response> {
    maybe_club_slug_redirect(db, method, path, query).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_redirect_rejects_open_redirect_paths() {
        assert!(safe_redirect_target("/\\evil.com", None).is_none());
        assert!(safe_redirect_target("//evil.com/x", None).is_none());
        assert!(safe_redirect_target("\\evil", None).is_none());
        assert!(safe_redirect_target("/public-map.html", Some("club=old&x=1")).is_some());
    }
}
