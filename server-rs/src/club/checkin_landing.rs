//! Browser fallback for `/c/{slug}` and `/c/{slug}/q/{token}` when the Android app is not verified.
use crate::club::{
    android_assetlinks::{absolute_https_url, intent_url_for_https, ANDROID_PACKAGE},
    checkin,
    club_scope::{checkin_qr_path, ClubScopeError},
    db::{now_ms, ClubDb},
    routes::club_db,
};
use crate::html_csp::{HTML_STRICT_CSP, SENSITIVE_REFERRER_POLICY};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json},
    routing::get,
    Router,
};
use serde_json::{json, Value};
use std::path::Path as FsPath;

#[derive(Debug)]
enum LandingError {
    NotFound,
    Db,
}

struct AppReleaseMeta {
    release_available: bool,
    apk_url: String,
}

async fn read_app_release(dist: &FsPath) -> AppReleaseMeta {
    let path = dist.join("app/app-release.json");
    let default_apk = "/app/flightvis.apk".to_string();
    let Ok(body) = tokio::fs::read_to_string(&path).await else {
        return AppReleaseMeta {
            release_available: false,
            apk_url: default_apk,
        };
    };
    let v: Value = serde_json::from_str(&body).unwrap_or(json!({}));
    let flag = v
        .get("available")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    let apk_url = v
        .get("apkUrl")
        .and_then(|s| s.as_str())
        .unwrap_or("/app/flightvis.apk")
        .to_string();
    let rel = apk_url.trim_start_matches('/');
    let has_file = dist.join(rel).is_file();
    AppReleaseMeta {
        release_available: flag && has_file,
        apk_url,
    }
}

fn optional_club_logo_url(dist: &FsPath, slug: &str) -> Option<String> {
    for name in ["logo.png", "logo.webp", "logo.svg"] {
        let rel = format!("club-logos/{slug}/{name}");
        if dist.join(&rel).is_file() {
            return Some(format!("/{rel}"));
        }
    }
    None
}

async fn club_display_name(db: &ClubDb, club_id: &str) -> Result<String, LandingError> {
    sqlx::query_scalar("SELECT name FROM clubs WHERE id = $1")
        .bind(club_id)
        .fetch_optional(&db.pool)
        .await
        .map_err(|_| LandingError::Db)?
        .ok_or(LandingError::NotFound)
}

async fn validate_checkin_peek(
    db: &ClubDb,
    slug: &str,
    token: &str,
    now: i64,
) -> Result<String, LandingError> {
    let ctx = db.resolve_club_context(slug).await.map_err(|e| match e {
        ClubScopeError::NotFound => LandingError::NotFound,
        ClubScopeError::Forbidden => LandingError::NotFound,
    })?;
    if let Some(row) = db.club_public_row(&ctx.club_id).await {
        if row.suspended_at.is_some() {
            return Err(LandingError::NotFound);
        }
    }
    let (secret, _) = db
        .club_checkin_settings(&ctx.club_id)
        .await
        .map_err(|_| LandingError::Db)?;
    let (token_club, _) =
        checkin::verify_checkin_token(&secret, token, now).map_err(|_| LandingError::NotFound)?;
    if token_club != ctx.club_id {
        return Err(LandingError::NotFound);
    }
    Ok(ctx.slug)
}

fn localize_data_i18n(html: &str, locale: &str) -> String {
    let cat = crate::i18n::require_global();
    let mut out = html.to_string();
    let pairs = [
        (
            r#"<h1 data-i18n="checkin.expiredTitle"></h1>"#,
            "checkin.expiredTitle",
            "h1",
        ),
        (
            r#"<p class="muted" data-i18n="checkin.expiredBody"></p>"#,
            "checkin.expiredBody",
            "p",
        ),
        (
            r#"<a href="/" data-i18n="common.home"></a>"#,
            "common.home",
            "a",
        ),
    ];
    for (empty, key, tag) in pairs {
        let text = cat.t(locale, key, &[]);
        let filled = if tag == "a" {
            format!(r#"<a href="/" data-i18n="{key}">{text}</a>"#)
        } else if tag == "p" {
            format!(r#"<p class="muted" data-i18n="{key}">{text}</p>"#)
        } else {
            format!(r#"<{tag} data-i18n="{key}">{text}</{tag}>"#)
        };
        out = out.replace(empty, &filled);
    }
    if let Some(start) = out.find(r#"<title data-i18n="checkin.expiredTitle">"#) {
        if let Some(end) = out[start..].find("</title>") {
            let title = cat.t(locale, "checkin.expiredTitle", &[]);
            out.replace_range(
                start..start + end + "</title>".len(),
                &format!(r#"<title data-i18n="checkin.expiredTitle">{title}</title>"#),
            );
        }
    }
    out
}

fn qr_landing_page_headers() -> [(header::HeaderName, &'static str); 3] {
    [
        (header::CACHE_CONTROL, "no-store"),
        (header::CONTENT_SECURITY_POLICY, HTML_STRICT_CSP),
        (header::REFERRER_POLICY, SENSITIVE_REFERRER_POLICY),
    ]
}

fn qr_landing_api_headers() -> [(header::HeaderName, &'static str); 2] {
    [
        (header::CACHE_CONTROL, "no-store"),
        (header::REFERRER_POLICY, SENSITIVE_REFERRER_POLICY),
    ]
}

async fn serve_landing_html(state: &AppState) -> axum::response::Response {
    let path = state.dist_dir.join("club-checkin-landing.html");
    match tokio::fs::read_to_string(&path).await {
        Ok(html) => (StatusCode::OK, qr_landing_page_headers(), Html(html)).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "missing landing").into_response(),
    }
}

async fn checkin_landing_page(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((slug, token)): Path<(String, String)>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return checkin_not_found_page(&state, &headers).await;
    };
    match validate_checkin_peek(db, &slug, &token, now_ms()).await {
        Ok(_) => serve_landing_html(&state).await,
        Err(LandingError::NotFound) => checkin_not_found_page(&state, &headers).await,
        Err(LandingError::Db) => (StatusCode::INTERNAL_SERVER_ERROR, "error").into_response(),
    }
}

async fn club_slug_register_page(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let Some(row) = db.club_public_row_by_slug(&slug).await else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    if row.suspended_at.is_some() {
        return serve_suspended_html(&state).await;
    }
    if !ClubDb::is_public_verified(&row) {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let path = state.dist_dir.join("club-register.html");
    match tokio::fs::read_to_string(&path).await {
        Ok(html) => (StatusCode::OK, qr_landing_page_headers(), Html(html)).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "missing page").into_response(),
    }
}

async fn serve_suspended_html(state: &AppState) -> axum::response::Response {
    let path = state.dist_dir.join("club-suspended.html");
    match tokio::fs::read_to_string(&path).await {
        Ok(html) => (StatusCode::FORBIDDEN, qr_landing_page_headers(), Html(html)).into_response(),
        Err(_) => (StatusCode::FORBIDDEN, "club suspended").into_response(),
    }
}

async fn club_slug_landing_page(
    State(state): State<AppState>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    if db.resolve_club_context(&slug).await.is_err() {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    if let Some(row) = db.club_public_row_by_slug(&slug).await {
        if row.suspended_at.is_some() {
            return serve_suspended_html(&state).await;
        }
    }
    serve_landing_html(&state).await
}

async fn checkin_not_found_page(state: &AppState, headers: &HeaderMap) -> axum::response::Response {
    let accept = headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let locale = crate::i18n::require_global().resolve_locale(None, Some("es"), accept);
    let path = state.dist_dir.join("club-checkin-expired.html");
    match tokio::fs::read_to_string(&path).await {
        Ok(html) => (
            StatusCode::NOT_FOUND,
            qr_landing_page_headers(),
            Html(localize_data_i18n(&html, &locale)),
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "expired").into_response(),
    }
}

async fn build_landing_json(
    state: &AppState,
    db: &ClubDb,
    headers: &HeaderMap,
    slug: &str,
    intent_url: Option<String>,
) -> Result<Value, LandingError> {
    let ctx = db.resolve_club_context(slug).await.map_err(|e| match e {
        ClubScopeError::NotFound => LandingError::NotFound,
        ClubScopeError::Forbidden => LandingError::NotFound,
    })?;
    let club_name = club_display_name(db, &ctx.club_id).await?;
    let club_default_locale = db
        .club_default_locale(&ctx.club_id)
        .await
        .unwrap_or_else(|_| "es".to_string());
    let accept = headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let resolved_locale = crate::i18n::global()
        .map(|c| c.resolve_locale(None, Some(club_default_locale.as_str()), accept))
        .unwrap_or(club_default_locale.clone());
    let release = read_app_release(&state.dist_dir).await;
    let logo_url = optional_club_logo_url(&state.dist_dir, slug);
    let mut body = json!({
        "slug": slug,
        "clubName": club_name,
        "logoUrl": logo_url,
        "clubDefaultLocale": club_default_locale,
        "resolvedLocale": resolved_locale,
        "releaseAvailable": release.release_available,
        "apkUrl": release.apk_url,
        "publicMapPath": format!("/public-map.html?club={slug}"),
        "appPath": "/app.html",
    });
    if let Some(intent) = intent_url {
        body["intentUrl"] = json!(intent);
    }
    Ok(body)
}

async fn checkin_landing_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((slug, token)): Path<(String, String)>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    };
    if validate_checkin_peek(db, &slug, &token, now_ms())
        .await
        .is_err()
    {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    }
    let https_path = checkin_qr_path(&slug, &token);
    let https_url = absolute_https_url(&headers, &https_path);
    let intent = intent_url_for_https(&https_url, ANDROID_PACKAGE);
    match build_landing_json(&state, db, &headers, &slug, Some(intent)).await {
        Ok(body) => (qr_landing_api_headers(), Json(body)).into_response(),
        Err(LandingError::NotFound) => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
        }
        Err(LandingError::Db) => (StatusCode::INTERNAL_SERVER_ERROR, "error").into_response(),
    }
}

async fn club_slug_landing_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    };
    match build_landing_json(&state, db, &headers, &slug, None).await {
        Ok(body) => (qr_landing_api_headers(), Json(body)).into_response(),
        Err(LandingError::NotFound) => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
        }
        Err(LandingError::Db) => (StatusCode::INTERNAL_SERVER_ERROR, "error").into_response(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/c/{slug}/q/{token}", get(checkin_landing_page))
        .route("/c/{slug}/registro", get(club_slug_register_page))
        .route("/c/{slug}", get(club_slug_landing_page))
        .route(
            "/api/c/{slug}/q/{token}/landing",
            get(checkin_landing_config),
        )
        .route("/api/c/{slug}/qr-landing", get(club_slug_landing_config))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::club::db::ClubDb;
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
    };
    use serial_test::serial;
    use std::sync::Arc;
    use tower::ServiceExt;

    const LANDING_FIXTURE: &str = r#"<html><body>
<a id="view-live-map" href="/public-map.html">map</a>
<div id="download-primary"><span id="btn-download-primary">x</span></div>
</body></html>"#;

    fn write_app_release(dist: &FsPath, available: bool, with_apk: bool) {
        std::fs::create_dir_all(dist.join("app")).unwrap();
        std::fs::write(
            dist.join("app/app-release.json"),
            serde_json::json!({
                "available": available,
                "apkUrl": "/app/flightvis.apk",
            })
            .to_string(),
        )
        .unwrap();
        if with_apk {
            std::fs::write(dist.join("app/flightvis.apk"), b"fake-apk").unwrap();
        }
    }

    async fn fixture(
        release_available: bool,
        with_apk: bool,
    ) -> (tempfile::TempDir, axum::Router, ClubDb) {
        let locales = crate::i18n::Catalogs::resolve_dir(std::path::Path::new("dist"));
        let _ = crate::i18n::init_global(&locales);
        let dir = tempfile::tempdir().unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("club-checkin-landing.html"), LANDING_FIXTURE).unwrap();
        std::fs::write(
            dist.join("club-checkin-expired.html"),
            r#"<h1 data-i18n="checkin.expiredTitle"></h1>"#,
        )
        .unwrap();
        write_app_release(&dist, release_available, with_apk);
        let db = ClubDb::open_test().await.unwrap();
        let club_id = db.default_club_id().await.unwrap();
        sqlx::query("UPDATE clubs SET slug = 'demo', name = $2 WHERE id = $1")
            .bind(&club_id)
            .bind("Demo & Club <script>")
            .execute(&db.pool)
            .await
            .unwrap();
        let now = now_ms();
        let _token = db.mint_checkin_qr_token(&club_id, now).await.unwrap();
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
            account_password_throttle: crate::LoginThrottle::default(),
            account_password_account_throttle: crate::LoginThrottle::default(),
            mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
            webauthn: None,
            sse_limiter: std::sync::Arc::new(crate::club::sse_limit::SseLimiter::default()),
            superadmin_emails: Default::default(),
        };
        let router = build_router(state);
        (dir, router, db)
    }

    async fn get_landing_api(router: &axum::Router, uri: &str) -> Value {
        let res = router
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[serial]
    #[tokio::test]
    async fn qr_landing_pages_send_csp_and_referrer_policy() {
        let (_dir, router, db) = fixture(false, false).await;
        let club_id = db.default_club_id().await.unwrap();
        let token = db.mint_checkin_qr_token(&club_id, now_ms()).await.unwrap();
        for uri in [format!("/c/demo/q/{token}"), "/c/demo".to_string()] {
            let res = router
                .clone()
                .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            assert_eq!(
                res.headers()
                    .get(header::CONTENT_SECURITY_POLICY)
                    .and_then(|v| v.to_str().ok()),
                Some(HTML_STRICT_CSP)
            );
            assert_eq!(
                res.headers()
                    .get(header::REFERRER_POLICY)
                    .and_then(|v| v.to_str().ok()),
                Some("no-referrer")
            );
        }
    }

    #[serial]
    #[tokio::test]
    async fn qr_landing_json_apis_send_cache_control_and_referrer() {
        let (_dir, router, db) = fixture(false, false).await;
        let club_id = db.default_club_id().await.unwrap();
        let token = db.mint_checkin_qr_token(&club_id, now_ms()).await.unwrap();
        for uri in [
            format!("/api/c/demo/q/{token}/landing"),
            "/api/c/demo/qr-landing".to_string(),
        ] {
            let res = router
                .clone()
                .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            assert_eq!(
                res.headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|v| v.to_str().ok()),
                Some("no-store")
            );
            assert_eq!(
                res.headers()
                    .get(header::REFERRER_POLICY)
                    .and_then(|v| v.to_str().ok()),
                Some("no-referrer")
            );
        }
    }

    #[serial]
    #[tokio::test]
    async fn landing_renders_for_valid_token_without_consuming() {
        let (_dir, router, db) = fixture(false, false).await;
        let club_id = db.default_club_id().await.unwrap();
        let now = now_ms();
        let token = db.mint_checkin_qr_token(&club_id, now).await.unwrap();
        let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM checkins")
            .fetch_one(&db.pool)
            .await
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
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("view-live-map"));
        assert!(html.contains("btn-download-primary"));
        let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM checkins")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        assert_eq!(before, after);
    }

    #[serial]
    #[tokio::test]
    async fn slug_landing_page_and_map_link() {
        let (_dir, router, _db) = fixture(false, false).await;
        let res = router
            .oneshot(
                Request::builder()
                    .uri("/c/demo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let html = String::from_utf8_lossy(&body);
        assert!(html.contains("id=\"view-live-map\""));
    }

    #[serial]
    #[tokio::test]
    async fn landing_api_release_unavailable() {
        let (_dir, router, db) = fixture(false, false).await;
        let club_id = db.default_club_id().await.unwrap();
        let token = db.mint_checkin_qr_token(&club_id, now_ms()).await.unwrap();
        let body = get_landing_api(&router, &format!("/api/c/demo/q/{token}/landing")).await;
        assert_eq!(body["releaseAvailable"], false);
        assert_eq!(body["publicMapPath"], "/public-map.html?club=demo");
        assert_eq!(body["clubName"], "Demo & Club <script>");
    }

    #[serial]
    #[tokio::test]
    async fn landing_api_release_available_when_apk_present() {
        let (_dir, router, db) = fixture(true, true).await;
        let club_id = db.default_club_id().await.unwrap();
        let token = db.mint_checkin_qr_token(&club_id, now_ms()).await.unwrap();
        let body = get_landing_api(&router, &format!("/api/c/demo/q/{token}/landing")).await;
        assert_eq!(body["releaseAvailable"], true);
        assert_eq!(body["apkUrl"], "/app/flightvis.apk");
    }

    #[serial]
    #[tokio::test]
    async fn invalid_token_gets_404_page() {
        let (_dir, router, _db) = fixture(false, false).await;
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

    fn assert_qr_page_headers<B>(res: &axum::response::Response<B>) {
        assert_eq!(
            res.headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .and_then(|v| v.to_str().ok()),
            Some(HTML_STRICT_CSP)
        );
        assert_eq!(
            res.headers()
                .get(header::REFERRER_POLICY)
                .and_then(|v| v.to_str().ok()),
            Some("no-referrer")
        );
        assert_eq!(
            res.headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("no-store")
        );
    }

    #[serial]
    #[tokio::test]
    async fn registro_and_suspended_pages_send_strict_qr_headers() {
        let (_dir, router, db) = fixture(false, false).await;
        std::fs::write(
            _dir.path().join("dist/club-register.html"),
            "<html><body>reg</body></html>",
        )
        .unwrap();
        std::fs::write(
            _dir.path().join("dist/club-suspended.html"),
            "<html><body>susp</body></html>",
        )
        .unwrap();
        sqlx::query("UPDATE clubs SET status = 'live', email_verified_at = $1 WHERE slug = 'demo'")
            .bind(now_ms())
            .execute(&db.pool)
            .await
            .unwrap();
        let reg = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/c/demo/registro")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reg.status(), StatusCode::OK);
        assert_qr_page_headers(&reg);

        let club_id: String = sqlx::query_scalar("SELECT id FROM clubs WHERE slug = 'demo'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
        db.set_club_suspended(&club_id, now_ms()).await.unwrap();
        let susp = router
            .oneshot(
                Request::builder()
                    .uri("/c/demo/registro")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(susp.status(), StatusCode::FORBIDDEN);
        assert_qr_page_headers(&susp);
    }
}
