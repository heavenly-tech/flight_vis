//! Shared HTTP handling for club slug rename (admin + superadmin).

use crate::club::{
    db::now_ms, db_club_slug::ClubSlugRenameError, routes::club_db,
    slug::validate_club_slug_candidate,
};
use crate::{auth, AppState};
use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json},
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct ClubSlugBody {
    pub slug: String,
}

pub async fn rename_club_slug_for_id(
    state: &AppState,
    club_id: &str,
    body: &ClubSlugBody,
    actor_pilot_id: Option<&str>,
) -> axum::response::Response {
    let Some(db) = club_db(state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let flight_slugs: Vec<String> = state
        .store
        .list_flights()
        .await
        .into_iter()
        .map(|f| f.slug)
        .collect();
    if validate_club_slug_candidate(&body.slug, &flight_slugs).is_err() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid slug" })),
        )
            .into_response();
    }
    let now = now_ms();
    match db
        .rename_club_slug(club_id, &body.slug, actor_pilot_id, now)
        .await
    {
        Ok(_) => {
            let slug = db.club_slug(club_id).await.unwrap_or_default();
            Json(json!({ "ok": true, "slug": slug })).into_response()
        }
        Err(ClubSlugRenameError::InvalidSlug) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid slug" })),
        )
            .into_response(),
        Err(ClubSlugRenameError::Taken) => {
            (StatusCode::CONFLICT, Json(json!({ "error": "slug taken" }))).into_response()
        }
        Err(ClubSlugRenameError::Unchanged) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unchanged" })),
        )
            .into_response(),
        Err(ClubSlugRenameError::Suspended) => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "club suspended" })),
        )
            .into_response(),
        Err(ClubSlugRenameError::RateLimited) => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "rename rate limit" })),
        )
            .into_response(),
        Err(ClubSlugRenameError::Db) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

pub fn require_exact_json_content_type(headers: &HeaderMap) -> Result<(), StatusCode> {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if auth::media_type_is_json(content_type) {
        Ok(())
    } else {
        Err(StatusCode::UNSUPPORTED_MEDIA_TYPE)
    }
}

pub fn parse_slug_body(bytes: &[u8]) -> Result<ClubSlugBody, StatusCode> {
    serde_json::from_slice(bytes).map_err(|_| StatusCode::BAD_REQUEST)
}
