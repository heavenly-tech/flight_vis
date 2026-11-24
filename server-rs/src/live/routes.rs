use crate::{live::engine::AckResult, require_auth, AppState};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::json;

use super::ingest::{osmand_ingest, post_ingest};
use super::sse::live_stream;

async fn list_live(State(state): State<AppState>, headers: HeaderMap) -> axum::response::Response {
    if let Err(denied) = require_auth(&state, &headers).await {
        return *denied;
    }
    let sessions = state.live.list_sessions().await;
    Json(json!({ "sessions": sessions })).into_response()
}

async fn get_fixes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> axum::response::Response {
    if let Err(denied) = require_auth(&state, &headers).await {
        return *denied;
    }
    match state.live.get_fixes(&id).await {
        Some(fixes) => Json(json!({ "id": id, "fixes": fixes })).into_response(),
        None => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
    }
}

async fn stop_live(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> axum::response::Response {
    if let Err(denied) = require_auth(&state, &headers).await {
        return *denied;
    }
    if state.live.stop_session(&id).await {
        Json(json!({ "ok": true })).into_response()
    } else {
        (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
    }
}

async fn ack_live(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> axum::response::Response {
    if let Err(denied) = require_auth(&state, &headers).await {
        return *denied;
    }
    match state.live.ack_session(&id).await {
        AckResult::Ok => Json(json!({ "ok": true })).into_response(),
        AckResult::StillActive => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "session still active" })),
        )
            .into_response(),
        AckResult::NotFound => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
        }
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/ingest", post(post_ingest))
        .route("/api/osmand", get(osmand_ingest).post(osmand_ingest))
        .route("/api/live", get(list_live))
        .route("/api/live/{id}/fixes", get(get_fixes))
        .route("/api/live/{id}/stream", get(live_stream))
        .route("/api/live/{id}/stop", post(stop_live))
        .route("/api/live/{id}/ack", post(ack_live))
}
