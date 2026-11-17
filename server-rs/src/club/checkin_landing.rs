//! Browser fallback for `/c/{slug}/q/{token}` when the Android app is not verified.
use crate::club::{
    android_assetlinks::{absolute_https_url, intent_url_for_https, ANDROID_PACKAGE},
    checkin,
    club_scope::{checkin_qr_path, ClubScopeError},
    db::{now_ms, ClubDb},
    routes::club_db,
};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use serde_json::json;

#[derive(Debug)]
enum LandingError {
    NotFound,
    Db,
}

fn validate_checkin_peek(db: &ClubDb, slug: &str, token: &str, now: i64) -> Result<String, LandingError> {
    let ctx = db.resolve_club_context(slug).map_err(|e| match e {
        ClubScopeError::NotFound => LandingError::NotFound,
        ClubScopeError::Forbidden => LandingError::NotFound,
    })?;
    let (secret, _) = db
        .club_checkin_settings(&ctx.club_id)
        .map_err(|_| LandingError::Db)?;
    let (token_club, _) =
        checkin::verify_checkin_token(&secret, token, now).map_err(|_| LandingError::NotFound)?;
    if token_club != ctx.club_id {
        return Err(LandingError::NotFound);
    }
    Ok(ctx.slug)
}

async fn checkin_landing_page(
    State(state): State<AppState>,
    Path((slug, token)): Path<(String, String)>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return checkin_not_found_page(&state).await;
    };
  match validate_checkin_peek(db, &slug, &token, now_ms()) {
        Ok(_) => {
            let path = state.dist_dir.join("club-checkin-landing.html");
            match tokio::fs::read_to_string(&path).await {
                Ok(html) => (
                    StatusCode::OK,
                    [(header::CACHE_CONTROL, "no-store")],
                    Html(html),
                )
                    .into_response(),
                Err(_) => (StatusCode::NOT_FOUND, "missing landing").into_response(),
            }
        }
        Err(LandingError::NotFound) => checkin_not_found_page(&state).await,
        Err(LandingError::Db) => (StatusCode::INTERNAL_SERVER_ERROR, "error").into_response(),
    }
}

async fn checkin_not_found_page(state: &AppState) -> axum::response::Response {
    let path = state.dist_dir.join("club-checkin-expired.html");
    match tokio::fs::read_to_string(&path).await {
        Ok(html) => (
            StatusCode::NOT_FOUND,
            [(header::CACHE_CONTROL, "no-store")],
            Html(html),
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "expired").into_response(),
    }
}

async fn checkin_landing_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((slug, token)): Path<(String, String)>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "not found" })),
        )
            .into_response();
    };
    if validate_checkin_peek(db, &slug, &token, now_ms()).is_err() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "not found" })),
        )
            .into_response();
    }
    let ctx = db.resolve_club_context(&slug).ok();
    let club_default_locale = ctx
        .as_ref()
        .and_then(|c| db.club_default_locale(&c.club_id).ok())
        .unwrap_or_else(|| "es".to_string());
    let accept = headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let resolved_locale = crate::i18n::global()
        .map(|c| {
            c.resolve_locale(
                None,
                Some(club_default_locale.as_str()),
                accept,
            )
        })
        .unwrap_or(club_default_locale.clone());
    let https_path = checkin_qr_path(&slug, &token);
    let https_url = absolute_https_url(&headers, &https_path);
    let intent = intent_url_for_https(&https_url, ANDROID_PACKAGE);
    Json(json!({
        "httpsUrl": https_url,
        "intentUrl": intent,
        "slug": slug,
        "appPath": "/app.html",
        "clubDefaultLocale": club_default_locale,
        "resolvedLocale": resolved_locale,
    }))
    .into_response()
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/c/{slug}/q/{token}", get(checkin_landing_page))
        .route("/api/c/{slug}/q/{token}/landing", get(checkin_landing_config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::club::{db::ClubDb, routes::club_db};
    use crate::{
        auth::Auth,
        build_router,
        device_auth::DeviceAuth,
        live::LiveEngine,
        store::Store,
        AppState,
    };
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::ServiceExt;

    fn fixture() -> (tempfile::TempDir, axum::Router, ClubDb) {
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(
            dist.join("club-checkin-landing.html"),
            "<html><body id=\"ok\">Descarga la app</body></html>",
        )
        .unwrap();
        std::fs::write(dist.join("club-checkin-expired.html"), "<html>expired</html>").unwrap();
        let db = ClubDb::open(dir.path()).unwrap();
        let club_id = db.default_club_id().unwrap();
        db.with_conn(|conn| {
            conn.execute(
                "UPDATE clubs SET slug = 'demo' WHERE id = ?1",
                rusqlite::params![club_id],
            )
        })
        .unwrap();
        let now = now_ms();
        let token = db.mint_checkin_qr_token(&club_id, now).unwrap();
        let state = AppState {
            dist_dir: dist,
            auth: Auth::with_password("x"),
            device_auth: DeviceAuth::empty(),
            live: Arc::new(LiveEngine::new(dir.path())),
            store: Store::new(dir.path()),
            club: Some(db.clone()),
            video_queue: tokio::sync::mpsc::channel(1).0,
            login_throttle: crate::LoginThrottle::default(),
            device_register_throttle: crate::LoginThrottle::default(),
            share_lookup_throttle: crate::LoginThrottle::default(),
            mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
        };
        let router = build_router(state);
        (dir, router, db)
    }

    #[tokio::test]
    async fn landing_renders_for_valid_token_without_consuming() {
        let (_dir, mut router, db) = fixture();
        let club_id = db.default_club_id().unwrap();
        let now = now_ms();
        let token = db.mint_checkin_qr_token(&club_id, now).unwrap();
        let before = db
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM checkins", [], |r| r.get::<_, i64>(0))
            })
            .unwrap();
        let res = router
            .oneshot(
                Request::builder()
                    .uri(format!("/c/demo/q/{token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Descarga"));
        let after = db
            .with_conn(|conn| {
                conn.query_row("SELECT COUNT(*) FROM checkins", [], |r| r.get::<_, i64>(0))
            })
            .unwrap();
        assert_eq!(before, after);
    }

    #[tokio::test]
    async fn invalid_token_gets_404_page() {
        let (_dir, mut router, _db) = fixture();
        let res = router
            .oneshot(
                Request::builder()
                    .uri("/c/demo/q/not-a-valid-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }
}