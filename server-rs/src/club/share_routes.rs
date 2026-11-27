//! Direct pilot flight share links (`/s/{token}`) — separate from public map tracks.
use crate::club::{
    db::{ShareCreateError, ShareRevokeError},
    qr,
    routes::{club_db, is_secure, resolve_flight_owner_actor},
};
use crate::html_csp::{HTML_GLOBE_CSP, SENSITIVE_REFERRER_POLICY};
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

async fn share_resolve_operational(
    db: &crate::club::ClubDb,
    token: &str,
) -> Result<crate::club::db::ResolvedShare, StatusCode> {
    let Some(meta) = db.resolve_share_token(token).await else {
        return Err(StatusCode::NOT_FOUND);
    };
    if let Some(ref club_id) = meta.club_id {
        if db.club_operational(club_id).await.is_err() {
            return Err(StatusCode::FORBIDDEN);
        }
    } else {
        let Some(ident) = db.pilot_identity(&meta.pilot_id).await else {
            return Err(StatusCode::NOT_FOUND);
        };
        if ident.account_suspended {
            return Err(StatusCode::NOT_FOUND);
        }
    }
    Ok(meta)
}

fn share_gate_error(status: StatusCode) -> axum::response::Response {
    if status == StatusCode::FORBIDDEN {
        return (
            status,
            [
                (header::CACHE_CONTROL, "no-store"),
                (header::PRAGMA, "no-cache"),
            ],
            Json(json!({ "error": "club unavailable" })),
        )
            .into_response();
    }
    share_not_found().into_response()
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

fn share_flight_page_headers() -> [(header::HeaderName, &'static str); 3] {
    [
        (header::CACHE_CONTROL, "no-store"),
        (header::CONTENT_SECURITY_POLICY, HTML_GLOBE_CSP),
        (header::REFERRER_POLICY, SENSITIVE_REFERRER_POLICY),
    ]
}

async fn share_flight_page(
    State(state): State<AppState>,
    Path(_token): Path<String>,
) -> impl IntoResponse {
    let path = state.dist_dir.join("share-flight.html");
    match tokio::fs::read_to_string(&path).await {
        Ok(html) => (StatusCode::OK, share_flight_page_headers(), Html(html)).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "share page missing").into_response(),
    }
}

async fn post_flight_share(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(flight_id): Path<String>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let id = match resolve_flight_owner_actor(&state, db, &headers, Some(&flight_id), true).await {
        Ok(pid) => pid,
        Err(e) => return e.into_response(),
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
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    };
    let id = match resolve_flight_owner_actor(&state, db, &headers, Some(&flight_id), false).await {
        Ok(pid) => pid,
        Err(e) => return e.into_response(),
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
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        )
            .into_response();
    };
    let id = match resolve_flight_owner_actor(&state, db, &headers, Some(&flight_id), true).await {
        Ok(pid) => pid,
        Err(e) => return e.into_response(),
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
    let meta = match share_resolve_operational(db, &token).await {
        Ok(m) => m,
        Err(status) => return share_gate_error(status),
    };
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "tail": meta.tail,
            "tailSource": meta.tail_source,
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
    let meta = match share_resolve_operational(db, &token).await {
        Ok(m) => m,
        Err(status) => return share_gate_error(status),
    };
    let fixes = db.fixes_for_flight(&meta.flight_id).await;
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({ "fixes": fixes, "tail": meta.tail, "tailSource": meta.tail_source })),
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
    let meta = match share_resolve_operational(db, &token).await {
        Ok(m) => m,
        Err(status) => return share_gate_error(status),
    };
    let db = state.club.clone().unwrap();
    let flight_id = meta.flight_id.clone();
    let share_club_id = meta.club_id.clone();
    let share_pilot_id = meta.pilot_id.clone();
    let token_capture = token.clone();
    let backfill = db.fixes_for_flight(&flight_id).await;
    let stream = stream::iter(backfill.into_iter().enumerate().map(move |(i, fix)| {
        Ok::<Event, Infallible>(Event::default().id(i.to_string()).json_data(fix).unwrap())
    }))
    .chain(stream::unfold(
        (
            db,
            flight_id,
            share_club_id,
            share_pilot_id,
            token_capture,
            0_i64,
        ),
        #[allow(clippy::question_mark)]
        |(db, fid, club_id, pilot_id, tok, mut last_t)| async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if let Some(ref cid) = club_id {
                if db.club_operational(cid).await.is_err() {
                    return None;
                }
            } else if db
                .pilot_identity(&pilot_id)
                .await
                .is_none_or(|ident| ident.account_suspended)
            {
                return None;
            }
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
            Some((Ok(event), (db, fid, club_id, pilot_id, tok, last_t)))
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

#[cfg(test)]
mod share_page_header_tests {
    use super::*;
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    #[tokio::test]
    async fn share_flight_page_sends_globe_csp_and_no_referrer() {
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("share-flight.html"), "<html></html>").unwrap();
        let locales = crate::i18n::Catalogs::resolve_dir(std::path::Path::new("dist"));
        let _ = crate::i18n::init_global(&locales);
        let club = crate::club::ClubDb::open_test().await.expect("club db");
        let state = AppState {
            dist_dir: dist,
            auth: Auth::with_password("x"),
            device_auth: DeviceAuth::empty(),
            live: Arc::new(LiveEngine::new(dir.path())),
            store: Store::new(dir.path()),
            club: Some(club),
            video_queue: tokio::sync::mpsc::channel(1).0,
            login_throttle: crate::LoginThrottle::default(),
            device_register_throttle: crate::LoginThrottle::default(),
            share_lookup_throttle: crate::LoginThrottle::default(),
            account_password_throttle: crate::LoginThrottle::default(),
            account_password_account_throttle: crate::LoginThrottle::default(),
            public_live_trails_throttle: crate::LoginThrottle::with_policy(
                crate::OverflowPolicy::RejectNew,
            ),
            mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
            webauthn: None,
            sse_limiter: std::sync::Arc::new(crate::club::sse_limit::SseLimiter::default()),
            superadmin_emails: Default::default(),
        };
        let router = build_router(state);
        let res = router
            .oneshot(
                Request::builder()
                    .uri("/s/example-share-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .and_then(|v| v.to_str().ok()),
            Some(crate::html_csp::HTML_GLOBE_CSP)
        );
        assert_eq!(
            res.headers()
                .get(header::REFERRER_POLICY)
                .and_then(|v| v.to_str().ok()),
            Some(crate::html_csp::SENSITIVE_REFERRER_POLICY)
        );
    }
}
