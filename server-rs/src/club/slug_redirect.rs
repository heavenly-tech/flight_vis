//! Permanent redirects from historical club slugs (single hop to current slug).

use crate::club::db::ClubDb;
use crate::static_files::normalize_url_path;
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

fn append_query(path: &str, query: Option<&str>) -> String {
    match query.filter(|q| !q.is_empty()) {
        Some(q) => format!("{path}?{q}"),
        None => path.to_string(),
    }
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
            if k == "club" && v == old {
                changed = true;
                Some(format!("club={new}"))
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

fn replace_slug_in_path(path: &str, segment_index: usize, new_slug: &str) -> String {
    let normalized = normalize_url_path(path).unwrap_or_else(|| path.to_string());
    let mut parts: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if segment_index < parts.len() {
        parts[segment_index] = new_slug;
    }
    format!("/{}", parts.join("/"))
}

pub fn redirect_moved_permanently(location: &str) -> Response {
    (
        StatusCode::MOVED_PERMANENTLY,
        [(header::LOCATION, location)],
    )
        .into_response()
}

/// GET/HEAD only. Returns a 301 when the path or `club=` query uses a historical slug.
pub async fn maybe_club_slug_redirect(
    db: &ClubDb,
    method: &str,
    path: &str,
    query: Option<&str>,
) -> Option<Response> {
    if method != "GET" && method != "HEAD" {
        return None;
    }
    if let Some(q) = query {
        if let Some(club_val) = q
            .split('&')
            .find_map(|pair| pair.strip_prefix("club=").map(str::to_string))
        {
            let norm = club_val.to_ascii_lowercase();
            if let Some(current) = db.current_slug_for_historical(&norm).await {
                let new_q = rewrite_query_club_param(q, &norm, &current)?;
                let base = normalize_url_path(path).unwrap_or_else(|| path.to_string());
                return Some(redirect_moved_permanently(&append_query(
                    &base,
                    Some(&new_q),
                )));
            }
        }
    }
    let normalized = normalize_url_path(path)?;
    let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return None;
    }
    let (slug_idx, old_slug) = if segments.len() >= 2 && segments[0] == "c" {
        (1, segments[1])
    } else if segments.len() == 3 && segments[0] == "api" && segments[1] == "clubs" {
        (2, segments[2])
    } else if segments.len() == 1 {
        (0, segments[0])
    } else {
        return None;
    };
    let current = db.current_slug_for_historical(old_slug).await?;
    let new_path = replace_slug_in_path(&normalized, slug_idx, &current);
    Some(redirect_moved_permanently(&append_query(&new_path, query)))
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
