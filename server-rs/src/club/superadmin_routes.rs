//! Superadmin club moderation (`FLIGHT_SUPERADMIN_EMAILS`).

use crate::club::{
    auth::{parse_account_session_cookie, parse_session_cookie},
    db::now_ms,
    routes::{club_db, viewer},
};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde_json::json;

async fn require_superadmin(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    let Some(db) = club_db(state) else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        ));
    };
    let now = now_ms();
    if let Some(acct_sess) = parse_account_session_cookie(headers) {
        if let Some((account, _)) = db.account_from_session(&acct_sess, now).await {
            if db.account_is_platform_superadmin(&account.id).await {
                return Ok(account.id);
            }
        }
    }
    let v = viewer(state, headers).await;
    let crate::club::visibility::Viewer::Pilot { id, .. } = v else {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))));
    };
    if !db
        .is_platform_superadmin(&id, &state.superadmin_emails)
        .await
    {
        return Err((StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))));
    }
    Ok(id)
}

async fn list_clubs(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(e) = require_superadmin(&state, &headers).await {
        return e.into_response();
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    Json(json!({ "clubs": db.list_all_clubs_admin().await })).into_response()
}

async fn suspend_club(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(club_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_superadmin(&state, &headers).await {
        return e.into_response();
    }
    if parse_session_cookie(&headers).is_some() {
        if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
            return (status, Json(json!({ "error": "csrf" }))).into_response();
        }
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let default_club = match db.default_club_id().await {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "db error" })),
            )
                .into_response();
        }
    };
    if club_id == default_club {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "cannot suspend default club" })),
        )
            .into_response();
    }
    let now = now_ms();
    match db.set_club_suspended(&club_id, now).await {
        Ok(true) => Json(json!({ "ok": true, "suspended": true })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn unsuspend_club(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(club_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_superadmin(&state, &headers).await {
        return e.into_response();
    }
    if parse_session_cookie(&headers).is_some() {
        if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
            return (status, Json(json!({ "error": "csrf" }))).into_response();
        }
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    match db.clear_club_suspended(&club_id).await {
        Ok(true) => Json(json!({ "ok": true, "suspended": false })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn suspend_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_superadmin(&state, &headers).await {
        return e.into_response();
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let now = now_ms();
    match db.set_account_suspended(&account_id, now).await {
        Ok(true) => Json(json!({ "ok": true, "suspended": true })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn unsuspend_account(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(account_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_superadmin(&state, &headers).await {
        return e.into_response();
    }
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    match db.clear_account_suspended(&account_id).await {
        Ok(true) => Json(json!({ "ok": true, "suspended": false })).into_response(),
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/superadmin/clubs", get(list_clubs))
        .route("/api/superadmin/clubs/{id}/suspend", post(suspend_club))
        .route("/api/superadmin/clubs/{id}/unsuspend", post(unsuspend_club))
        .route(
            "/api/superadmin/accounts/{id}/suspend",
            post(suspend_account),
        )
        .route(
            "/api/superadmin/accounts/{id}/unsuspend",
            post(unsuspend_account),
        )
}
