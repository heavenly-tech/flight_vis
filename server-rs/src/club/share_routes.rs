//! Direct pilot flight share links (`/s/{token}`) — separate from public map tracks.
use crate::club::{
    db::{ShareCreateError, ShareRevokeError},
    qr,
    routes::{club_db, is_secure, viewer},
    visibility::Viewer,
};
use crate::{AppState, ClientIp, SHARE_LOOKUP_MAX_ATTEMPTS};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        Html, IntoResponse, Json,
    },
    routing::{delete, get},
    Router,
};
use futures_util::stream;
use serde_json::json;
use std::{convert::Infallible, time::Duration};
use tokio_stream::StreamExt as _;

fn share_public_url(state: &AppState, headers: &HeaderMap, token: &str) -> String {
    let base = state.mailer.public_url.trim_end_matches('/');
    if !base.is_empty() && base.starts_with("http") {
        return format!("{base}/s/{token}");
    }
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("localhost");
    let proto = if is_secure(headers) { "https" } else { "http" };
    format!("{proto}://{host}/s/{token}")
}

fn share_rate_limit(state: &AppState, ip: &ClientIp) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    state
        .share_lookup_throttle
        .allow_up_to(&ip.0, now, SHARE_LOOKUP_MAX_ATTEMPTS)
}

fn share_not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        [
            (header::CACHE_CONTROL, "no-store"),
            (header::PRAGMA, "no-cache"),
        ],
        Json(json!({ "error": "not found" })),
    )
}

async fn share_flight_page(
    State(state): State<AppState>,
    Path(_token): Path<String>,
) -> impl IntoResponse {
    let path = state.dist_dir.join("share-flight.html");
    match tokio::fs::read_to_string(&path).await {
        Ok(html) => (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "no-store")],
            Html(html),
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "share page missing").into_response(),
    }
}

async fn post_flight_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(flight_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    match db.create_flight_share(&id, &flight_id).await {
        Ok((row, token)) => {
            let url = share_public_url(&state, &headers, &token);
            let qr_svg = qr::svg_for_payload(&url, 220).unwrap_or_default();
            (
                StatusCode::OK,
                [(header::CACHE_CONTROL, "no-store")],
                Json(json!({
                    "id": row.id,
                    "token": token,
                    "url": url,
                    "qrSvg": qr_svg,
                    "createdAt": row.created_at,
                })),
            )
                .into_response()
        }
        Err(ShareCreateError::NotOwner) => {
            (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))).into_response()
        }
        Err(ShareCreateError::NotFound) => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn get_flight_shares(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(flight_id): Path<String>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return Json(json!({ "shares": [] })).into_response();
    };
    match db.list_flight_shares(&id, &flight_id).await {
        Ok(shares) => Json(json!({ "shares": shares })).into_response(),
        Err(ShareCreateError::NotOwner) => {
            (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))).into_response()
        }
        Err(ShareCreateError::NotFound) => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn revoke_flight_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((flight_id, share_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Viewer::Pilot { id, .. } = v else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    match db.revoke_flight_share(&id, &flight_id, &share_id).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(ShareRevokeError::NotOwner) => {
            (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" }))).into_response()
        }
        Err(ShareRevokeError::NotFound) => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn share_flight_meta(
    State(state): State<AppState>,
    ip: ClientIp,
    Path(token): Path<String>,
) -> impl IntoResponse {
    if !share_rate_limit(&state, &ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({ "error": "rate limited" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return share_not_found().into_response();
    };
    let Some(meta) = db.resolve_share_token(&token).await else {
        return share_not_found().into_response();
    };
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "tail": meta.tail,
            "status": meta.status,
        })),
    )
        .into_response()
}

async fn share_flight_fixes(
    State(state): State<AppState>,
    ip: ClientIp,
    Path(token): Path<String>,
) -> impl IntoResponse {
    if !share_rate_limit(&state, &ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({ "error": "rate limited" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return share_not_found().into_response();
    };
    let Some(meta) = db.resolve_share_token(&token).await else {
        return share_not_found().into_response();
    };
    let fixes = db.fixes_for_flight(&meta.flight_id).await;
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({ "fixes": fixes, "tail": meta.tail })),
    )
        .into_response()
}

async fn share_flight_stream(
    State(state): State<AppState>,
    ip: ClientIp,
    Path(token): Path<String>,
) -> impl IntoResponse {
    if !share_rate_limit(&state, &ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::CACHE_CONTROL, "no-store")],
            Json(json!({ "error": "rate limited" })),
        )
            .into_response();
    }
    let Some(db) = club_db(&state) else {
        return share_not_found().into_response();
    };
    let Some(meta) = db.resolve_share_token(&token).await else {
        return share_not_found().into_response();
    };
    let db = state.club.clone().unwrap();
    let flight_id = meta.flight_id.clone();
    let token_capture = token.clone();
    let backfill = db.fixes_for_flight(&flight_id).await;
    let stream = stream::iter(backfill.into_iter().enumerate().map(move |(i, fix)| {
        Ok::<Event, Infallible>(Event::default().id(i.to_string()).json_data(fix).unwrap())
    }))
    .chain(stream::unfold(
        (db, flight_id, token_capture, 0_i64),
        #[allow(clippy::question_mark)]
        |(db, fid, tok, mut last_t)| async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if db.share_flight_id(&tok).await.is_none() {
                return None;
            }
            let fresh = db.fixes_for_flight_since(&fid, last_t).await;
            if let Some(t) = fresh.last().map(|f| f.t) {
                last_t = t;
            }
            let event = if fresh.is_empty() {
                Event::default().comment("ping")
            } else {
                Event::default()
                    .json_data(json!({ "fixes": fresh }))
                    .unwrap()
            };
            Some((Ok(event), (db, fid, tok, last_t)))
        },
    ));
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/s/{token}", get(share_flight_page))
        .route("/api/share/{token}", get(share_flight_meta))
        .route("/api/share/{token}/fixes", get(share_flight_fixes))
        .route("/api/share/{token}/stream", get(share_flight_stream))
        .route(
            "/api/me/flights/{id}/shares",
            get(get_flight_shares).post(post_flight_share),
        )
        .route(
            "/api/me/flights/{id}/shares/{share_id}",
            delete(revoke_flight_share),
        )
}
