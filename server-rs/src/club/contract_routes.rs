//! Stable HTTP aliases expected by flightvis-client (I-5).

use crate::club::auth_routes::{
    post_guest, post_magic_link, webauthn_login_begin, webauthn_login_finish,
};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use serde_json::json;

use super::routes::{club_db, is_secure};
use crate::club::auth::parse_session_cookie;
use crate::club::{auth::set_session_cookie, db::now_ms, db::MagicConsumeError};

#[derive(Deserialize)]
struct MagicCompleteBody {
    token: String,
    #[serde(default)]
    slug: Option<String>,
}

async fn api_magic_complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<MagicCompleteBody>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let club_id = if let Some(slug) = body.slug.as_deref().filter(|s| !s.is_empty()) {
        match db.club_id_for_slug(slug).await {
            Some(id) => id,
            None => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": "club not found" })),
                )
                    .into_response();
            }
        }
    } else {
        db.default_club_id().await.unwrap_or_default()
    };
    let now = now_ms();
    match db.consume_magic_login(&body.token, &club_id, now).await {
        Ok((session_id, pilot)) => {
            let cookie = set_session_cookie(&session_id, is_secure(&headers));
            (
                StatusCode::OK,
                [(header::SET_COOKIE, cookie)],
                Json(json!({
                    "ok": true,
                    "pilot": {
                        "id": pilot.id,
                        "name": pilot.name,
                        "email": pilot.email,
                        "status": pilot.status,
                        "role": pilot.role,
                    }
                })),
            )
                .into_response()
        }
        Err(MagicConsumeError::ClubUnavailable) => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "club unavailable" })),
        )
            .into_response(),
        Err(MagicConsumeError::Expired) => {
            (StatusCode::GONE, Json(json!({ "error": "expired" }))).into_response()
        }
        Err(_) => (StatusCode::BAD_REQUEST, Json(json!({ "error": "invalid" }))).into_response(),
    }
}

async fn api_club_by_slug(
    State(state): State<AppState>,
    headers: HeaderMap,
    method: axum::http::Method,
    uri: axum::http::Uri,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    if let Some(resp) = crate::club::slug_redirect::redirect_if_historical_slug(
        db,
        method.as_str(),
        uri.path(),
        uri.query(),
    )
    .await
    {
        return resp;
    }
    let Some(club_id) = db.club_id_for_slug(slug.trim()).await else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    };
    let row: Option<(String, String, String)> = sqlx::query_as::<_, (String, String, String)>(
        "SELECT id, name, slug FROM clubs WHERE id = $1",
    )
    .bind(&club_id)
    .fetch_optional(&db.pool)
    .await
    .ok()
    .flatten();
    let Some((id, name, slug)) = row else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    };
    let default_locale = db
        .club_default_locale(&id)
        .await
        .unwrap_or_else(|_| "es".to_string());
    let session = parse_session_cookie(&headers);
    let now = now_ms();
    let (_viewer, pilot) = db.viewer_with_pilot(session.as_deref(), now).await;
    let user_locale = pilot.as_ref().and_then(|p| p.locale.clone());
    Json(json!({
        "club": { "id": id, "name": name, "slug": slug, "defaultLocale": default_locale },
        "userLocale": user_locale,
    }))
    .into_response()
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/auth/magic/send", post(post_magic_link))
        .route("/api/auth/magic/complete", post(api_magic_complete))
        .route(
            "/api/auth/passkey/login/options",
            post(webauthn_login_begin),
        )
        .route(
            "/api/auth/passkey/login/verify",
            post(webauthn_login_finish),
        )
        .route("/api/devices/anonymous", post(post_guest))
        .route("/api/clubs/{slug}", get(api_club_by_slug))
}
