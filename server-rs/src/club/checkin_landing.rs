//! Browser fallback for `/c/{slug}` and `/c/{slug}/q/{token}` when the Android app is not verified.
use crate::club::{
    auth::set_session_cookie,
    checkin,
    club_scope::ClubScopeError,
    db::{now_ms, ClubDb},
    routes::{club_db, is_secure},
    slug_redirect,
};
use crate::html_csp::{HTML_STRICT_CSP, SENSITIVE_REFERRER_POLICY};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, Method, StatusCode, Uri},
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
    if db
        .qr_token_landing_consumed(&checkin::token_digest(token))
        .await
    {
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
    method: Method,
    uri: Uri,
    Path((slug, token)): Path<(String, String)>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return checkin_not_found_page(&state, &headers, &slug).await;
    };
    if let Some(resp) =
        slug_redirect::redirect_if_historical_slug(db, method.as_str(), uri.path(), uri.query())
            .await
    {
        return resp;
    }
    let token = token.trim();
    if checkin::is_handoff_token(token) {
        let now = now_ms();
        let Ok((club_id, _, _)) = db.validate_handoff_token(token, now).await else {
            return checkin_not_found_page(&state, &headers, &slug).await;
        };
        let slug_matches = matches!(
            db.resolve_club_context(&slug).await,
            Ok(ctx) if ctx.club_id == club_id
        );
        let suspended = db
            .club_public_row(&club_id)
            .await
            .and_then(|row| row.suspended_at)
            .is_some();
        if !slug_matches || suspended {
            return checkin_not_found_page(&state, &headers, &slug).await;
        }
        return serve_landing_html(&state).await;
    }
    match validate_checkin_peek(db, &slug, token, now_ms()).await {
        Ok(_) => serve_landing_html(&state).await,
        Err(LandingError::NotFound) => checkin_not_found_page(&state, &headers, &slug).await,
        Err(LandingError::Db) => (StatusCode::INTERNAL_SERVER_ERROR, "error").into_response(),
    }
}

async fn club_slug_register_page(
    State(state): State<AppState>,
    method: Method,
    uri: Uri,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    if let Some(resp) =
        slug_redirect::redirect_if_historical_slug(db, method.as_str(), uri.path(), uri.query())
            .await
    {
        return resp;
    }
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
    method: Method,
    uri: Uri,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    if let Some(resp) =
        slug_redirect::redirect_if_historical_slug(db, method.as_str(), uri.path(), uri.query())
            .await
    {
        return resp;
    }
    if db.club_id_for_slug_any(&slug).await.is_none() {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    if let Some(row) = db.club_public_row_by_slug(&slug).await {
        if row.suspended_at.is_some() {
            return serve_suspended_html(&state).await;
        }
    }
    serve_landing_html(&state).await
}

async fn checkin_not_found_page(
    state: &AppState,
    headers: &HeaderMap,
    slug: &str,
) -> axum::response::Response {
    let accept = headers
        .get(axum::http::header::ACCEPT_LANGUAGE)
        .and_then(|v| v.to_str().ok());
    let stored = crate::club::auth::parse_fv_locale_cookie(headers);
    let club_default = if let Some(db) = club_db(state) {
        match db.resolve_club_context(slug).await {
            Ok(ctx) => db
                .club_default_locale(&ctx.club_id)
                .await
                .unwrap_or_else(|_| "es".to_string()),
            Err(_) => "es".to_string(),
        }
    } else {
        "es".to_string()
    };
    let locale = crate::i18n::require_global().resolve_locale(
        None,
        Some(club_default.as_str()),
        accept,
        stored.as_deref(),
    );
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

async fn build_landing_json_with_handoff(
    state: &AppState,
    db: &ClubDb,
    headers: &HeaderMap,
    slug: &str,
    intent_url: Option<String>,
    handoff_token: &str,
    _now: i64,
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
        .map(|c| {
            let stored = crate::club::auth::parse_fv_locale_cookie(headers);
            c.resolve_locale(
                None,
                Some(club_default_locale.as_str()),
                accept,
                stored.as_deref(),
            )
        })
        .unwrap_or(club_default_locale.clone());
    let release = read_app_release(&state.dist_dir).await;
    let logo_url = optional_club_logo_url(&state.dist_dir, &ctx.slug);
    let mut body = json!({
        "slug": ctx.slug,
        "clubName": club_name,
        "logoUrl": logo_url,
        "clubDefaultLocale": club_default_locale,
        "resolvedLocale": resolved_locale,
        "releaseAvailable": release.release_available,
        "apkUrl": release.apk_url,
        "publicMapPath": format!("/public-map.html?club={}", ctx.slug),
        "appPath": "/app.html",
        "claimToken": handoff_token,
    });
    if let Some(intent) = intent_url {
        body["intentUrl"] = json!(intent);
    }
    Ok(body)
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
        .map(|c| {
            let stored = crate::club::auth::parse_fv_locale_cookie(headers);
            c.resolve_locale(
                None,
                Some(club_default_locale.as_str()),
                accept,
                stored.as_deref(),
            )
        })
        .unwrap_or(club_default_locale.clone());
    let release = read_app_release(&state.dist_dir).await;
    let logo_url = optional_club_logo_url(&state.dist_dir, &ctx.slug);
    let mut body = json!({
        "slug": ctx.slug,
        "clubName": club_name,
        "logoUrl": logo_url,
        "clubDefaultLocale": club_default_locale,
        "resolvedLocale": resolved_locale,
        "releaseAvailable": release.release_available,
        "apkUrl": release.apk_url,
        "publicMapPath": format!("/public-map.html?club={}", ctx.slug),
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
    method: Method,
    uri: Uri,
    Path((slug, token)): Path<(String, String)>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    };
    if let Some(resp) =
        slug_redirect::redirect_if_historical_slug(db, method.as_str(), uri.path(), uri.query())
            .await
    {
        return resp;
    }

    let token = token.trim();

    // A handoff token is already the single-use claim: return the same config, never remint.
    if checkin::is_handoff_token(token) {
        let now = now_ms();
        let Ok((club_id, _, _)) = db.validate_handoff_token(token, now).await else {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
        };
        let Ok(ctx) = db.resolve_club_context(&slug).await else {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
        };
        if ctx.club_id != club_id {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
        }
        let public_host = state.mailer.public_url.trim_end_matches('/');
        let intent = Some(format!("{}/c/{}/q/{}", public_host, ctx.slug, token));
        return match build_landing_json_with_handoff(
            &state, db, &headers, &ctx.slug, intent, token, now,
        )
        .await
        {
            Ok(body) => (qr_landing_api_headers(), Json(body)).into_response(),
            Err(LandingError::NotFound) => {
                (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
            }
            Err(LandingError::Db) => (StatusCode::INTERNAL_SERVER_ERROR, "error").into_response(),
        };
    }

    // Validate the QR token (also rejects tokens already consumed by a landing).
    if validate_checkin_peek(db, &slug, token, now_ms())
        .await
        .is_err()
    {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    }

    let now = now_ms();
    let Ok(ctx) = db.resolve_club_context(&slug).await else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    };
    let Ok((_, valid_min)) = db.club_checkin_settings(&ctx.club_id).await else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "error").into_response();
    };

    let digest = checkin::token_digest(token);
    let step = checkin::current_step(now);

    // The browser gets a guest pilot plus a session cookie so the signup form can claim it.
    let Ok(guest_pilot_id) = db.create_guest_for_handoff(&ctx.club_id, now).await else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "error").into_response();
    };
    let Ok(guest_session) = db
        .create_guest_session_for_pilot(&ctx.club_id, &guest_pilot_id, now)
        .await
    else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "error").into_response();
    };

    // Consume the QR token globally: kiosk rotation and a later POST /api/checkin both see it.
    if db
        .insert_checkin_row(
            &ctx.club_id,
            Some(&guest_pilot_id),
            None,
            step,
            &digest,
            now,
            Some(now + valid_min * 60_000),
            "ok",
            Some("landing_claim"),
        )
        .await
        .is_err()
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, "error").into_response();
    }

    let Ok(handoff_token) = db
        .mint_handoff_token(&ctx.club_id, &guest_pilot_id, None, now)
        .await
    else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "error").into_response();
    };

    let public_host = state.mailer.public_url.trim_end_matches('/');
    let intent = Some(format!(
        "{}/c/{}/q/{}",
        public_host, ctx.slug, handoff_token
    ));

    let body = match build_landing_json_with_handoff(
        &state,
        db,
        &headers,
        &ctx.slug,
        intent,
        &handoff_token,
        now,
    )
    .await
    {
        Ok(body) => body,
        Err(LandingError::NotFound) => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response()
        }
        Err(LandingError::Db) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "error").into_response()
        }
    };
    let cookie = set_session_cookie(&guest_session, is_secure(&headers));
    let [(csp, csp_value), (referrer, referrer_value)] = qr_landing_api_headers();
    (
        [
            (csp, csp_value),
            (referrer, referrer_value),
            (header::SET_COOKIE, cookie.as_str()),
        ],
        Json(body),
    )
        .into_response()
}

async fn club_slug_landing_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    method: Method,
    uri: Uri,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
    };
    if let Some(resp) =
        slug_redirect::redirect_if_historical_slug(db, method.as_str(), uri.path(), uri.query())
            .await
    {
        return resp;
    }
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

    async fn get_json(router: &axum::Router, uri: &str) -> (StatusCode, Value, Option<String>) {
        let res = router
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let set_cookie = res
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            set_cookie,
        )
    }

    async fn post_json(
        router: &axum::Router,
        uri: &str,
        body: &str,
        cookie: Option<&str>,
        bearer: Option<&str>,
    ) -> (StatusCode, Value, Option<String>) {
        let mut req = Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(value) = cookie {
            req = req.header(header::COOKIE, value);
        }
        if let Some(value) = bearer {
            req = req.header(header::AUTHORIZATION, format!("Bearer {value}"));
        }
        let res = router
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let set_cookie = res
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
            set_cookie,
        )
    }

    fn cookie_pair(set_cookie: &str) -> String {
        set_cookie
            .split(';')
            .next()
            .unwrap_or(set_cookie)
            .to_string()
    }

    async fn make_club_live(db: &ClubDb) -> String {
        let club_id = db.default_club_id().await.unwrap();
        sqlx::query(
            "UPDATE clubs SET status = 'live', email_verified_at = COALESCE(email_verified_at, $1) WHERE id = $2",
        )
        .bind(now_ms())
        .bind(&club_id)
        .execute(&db.pool)
        .await
        .unwrap();
        club_id
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
        // The HTML page only peeks; the JSON landing endpoint consumes.
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

    #[serial]
    #[tokio::test]
    async fn landing_api_returns_claim_token() {
        let (_dir, router, db) = fixture(false, false).await;
        let club_id = db.default_club_id().await.unwrap();
        let now = now_ms();
        let token = db.mint_checkin_qr_token(&club_id, now).await.unwrap();
        let body = get_landing_api(&router, &format!("/api/c/demo/q/{token}/landing")).await;

        // Check that claimToken is present
        assert!(body.get("claimToken").is_some());
        let claim_token = body["claimToken"].as_str().unwrap();
        assert!(claim_token.starts_with("h1."));

        // Check that intentUrl uses the claim token
        assert!(body.get("intentUrl").is_some());
        let intent_url = body["intentUrl"].as_str().unwrap();
        assert!(intent_url.contains(claim_token));
    }

    #[serial]
    #[tokio::test]
    async fn handoff_token_landing_returns_same_config() {
        let (_dir, router, db) = fixture(false, false).await;
        let club_id = db.default_club_id().await.unwrap();
        let now = now_ms();
        let token = db.mint_checkin_qr_token(&club_id, now).await.unwrap();

        // First request consumes QR and creates handoff token
        let body1 = get_landing_api(&router, &format!("/api/c/demo/q/{token}/landing")).await;
        let claim_token = body1["claimToken"].as_str().unwrap();

        // Second request with handoff token should also work
        let body2 = get_landing_api(&router, &format!("/api/c/demo/q/{claim_token}/landing")).await;

        // Both should have the same club info
        assert_eq!(body1["clubName"], body2["clubName"]);
        assert_eq!(body1["slug"], body2["slug"]);

        // The handoff token request should also return the claimToken
        assert_eq!(body2["claimToken"], claim_token);
    }

    #[serial]
    #[tokio::test]
    async fn guest_first_scan_flow_issues_device_and_accepts_ingest() {
        let (_dir, router, db) = fixture(false, false).await;
        let club_id = make_club_live(&db).await;
        let now = now_ms();

        // 1) Anonymous guest session (app 1.1.0+2 path).
        let (status, body, set_cookie) =
            post_json(&router, "/api/devices/anonymous", "{}", None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], true);
        let guest_cookie = cookie_pair(&set_cookie.expect("guest session cookie"));

        // 2) The guest session may register a device (not a 401).
        let (status, body, _) = post_json(
            &router,
            "/api/me/devices",
            r#"{"label":"Pixel","installId":"install-first-scan-01"}"#,
            Some(&guest_cookie),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let device_id = body["id"].as_str().expect("device id").to_string();
        let device_token = body["token"].as_str().expect("device token").to_string();

        // 3) Landing consumes the q-token and returns a single-use claim token.
        let qr = db.mint_checkin_qr_token(&club_id, now).await.unwrap();
        let (status, landing, _) = get_json(&router, &format!("/api/c/demo/q/{qr}/landing")).await;
        assert_eq!(status, StatusCode::OK);
        let claim = landing["claimToken"]
            .as_str()
            .expect("claimToken")
            .to_string();
        assert!(claim.starts_with("h1."));

        // 4) Check-in with the handoff succeeds for a guest device; still not cleared.
        let (status, checkin, _) = post_json(
            &router,
            "/api/checkin",
            &format!(r#"{{"token":"{claim}"}}"#),
            Some(&guest_cookie),
            Some(&device_token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(checkin["ok"], true);
        assert_eq!(checkin["status"], "guest");
        assert_eq!(checkin["needsSignup"], true);
        assert!(checkin.get("cleared").is_none());

        // 5) The guest device may share location with that club (no flight, not cleared).
        let q = format!(
            "id={device_token}&lat=-33.4&lon=-70.6&timestamp={}",
            now / 1000
        );
        let res = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/osmand?{q}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert!(db
            .club_id_for_device_checkin(&device_id, now)
            .await
            .is_some());
    }

    #[serial]
    #[tokio::test]
    async fn handoff_checkin_survives_qr_step_rotation() {
        let (_dir, router, db) = fixture(false, false).await;
        let club_id = make_club_live(&db).await;
        let now = now_ms();
        let (_session, _guest_sess, guest) = db
            .create_guest_session(&club_id, "Guest", now)
            .await
            .unwrap();
        let qr = db.mint_checkin_qr_token(&club_id, now).await.unwrap();
        let (status, landing, _) = get_json(&router, &format!("/api/c/demo/q/{qr}/landing")).await;
        assert_eq!(status, StatusCode::OK);
        let claim = landing["claimToken"]
            .as_str()
            .expect("claimToken")
            .to_string();

        // The HMAC QR step is long gone, but the handoff (10 min TTL) still checks in.
        let record = db
            .submit_checkin(&guest.id, None, &claim, now + 31_000)
            .await
            .expect("handoff check-in after step rotation");
        assert!(record.expires_at > now + 31_000);
    }

    #[serial]
    #[tokio::test]
    async fn handoff_checkin_is_single_use_and_expires() {
        let (_dir, router, db) = fixture(false, false).await;
        let club_id = make_club_live(&db).await;
        let now = now_ms();
        let (session, _guest_sess, guest) = db
            .create_guest_session(&club_id, "Guest", now)
            .await
            .unwrap();
        let cookie = format!("{}={session}", crate::club::auth::COOKIE);

        let handoff = db
            .mint_handoff_token(&club_id, &guest.id, None, now)
            .await
            .unwrap();
        let body = format!(r#"{{"token":"{handoff}"}}"#);
        let (status, first, _) =
            post_json(&router, "/api/checkin", &body, Some(&cookie), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(first["ok"], true);

        let (status, second, _) =
            post_json(&router, "/api/checkin", &body, Some(&cookie), None).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(second["reason"], "used_claim");
        assert!(second["message"].as_str().is_some_and(|m| !m.is_empty()));

        let expired = db
            .mint_handoff_token(&club_id, &guest.id, None, now - 700_000)
            .await
            .unwrap();
        let (status, body, _) = post_json(
            &router,
            "/api/checkin",
            &format!(r#"{{"token":"{expired}"}}"#),
            Some(&cookie),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["reason"], "expired_claim");
    }

    #[serial]
    #[tokio::test]
    async fn landing_consume_marks_qr_used_for_checkin_and_kiosk() {
        let (_dir, router, db) = fixture(false, false).await;
        let club_id = make_club_live(&db).await;
        let now = now_ms();
        let (session, _guest_sess, _guest) = db
            .create_guest_session(&club_id, "Guest", now)
            .await
            .unwrap();
        let cookie = format!("{}={session}", crate::club::auth::COOKIE);
        let qr = db.mint_checkin_qr_token(&club_id, now).await.unwrap();

        let (status, landing, _) = get_json(&router, &format!("/api/c/demo/q/{qr}/landing")).await;
        assert_eq!(status, StatusCode::OK);
        let claim = landing["claimToken"]
            .as_str()
            .expect("claimToken")
            .to_string();

        // Kiosk rotation sees the landing consume.
        assert!(db
            .club_checkin_ok_for_qr_window(&club_id, now - 1_000, now + 1_000)
            .await
            .unwrap());

        // A later check-in with the original q-token is used_qr.
        let (status, body, _) = post_json(
            &router,
            "/api/checkin",
            &format!(r#"{{"token":"{qr}"}}"#),
            Some(&cookie),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["reason"], "used_qr");
        assert_eq!(body["error"], "used_qr");

        // A second landing GET for the consumed q-token does not remint.
        let (status, _, _) = get_json(&router, &format!("/api/c/demo/q/{qr}/landing")).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // The still-valid unused handoff returns the same config and claim token.
        let (status, again, _) = get_json(&router, &format!("/api/c/demo/q/{claim}/landing")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(again["claimToken"], claim);
    }

    #[serial]
    #[tokio::test]
    async fn historical_slug_landing_api_redirects_without_consuming_qr() {
        let (_dir, router, db) = fixture(false, false).await;
        let club_id = make_club_live(&db).await;
        let now = now_ms();
        let qr = db.mint_checkin_qr_token(&club_id, now).await.unwrap();
        db.rename_club_slug(&club_id, "demo-renamed", None, now)
            .await
            .unwrap();

        let res = router
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/c/demo/q/{qr}/landing"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FOUND);
        assert_eq!(
            res.headers()
                .get(header::LOCATION)
                .and_then(|v| v.to_str().ok()),
            Some(format!("/api/c/demo-renamed/q/{qr}/landing").as_str())
        );
        assert!(
            !db.qr_token_landing_consumed(&checkin::token_digest(&qr))
                .await
        );

        let (status, landing, _) =
            get_json(&router, &format!("/api/c/demo-renamed/q/{qr}/landing")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(landing["slug"], "demo-renamed");
        assert!(landing["claimToken"]
            .as_str()
            .is_some_and(|t| t.starts_with("h1.")));
    }

    #[serial]
    #[tokio::test]
    async fn guest_osmand_rejects_raw_device_id_and_requires_token() {
        let (_dir, router, db) = fixture(false, false).await;
        let club_id = make_club_live(&db).await;
        let now = now_ms();

        let (status, _, set_cookie) =
            post_json(&router, "/api/devices/anonymous", "{}", None, None).await;
        assert_eq!(status, StatusCode::OK);
        let guest_cookie = cookie_pair(&set_cookie.expect("guest session cookie"));

        let (status, body, _) = post_json(
            &router,
            "/api/me/devices",
            r#"{"label":"Pixel","installId":"install-first-scan-02"}"#,
            Some(&guest_cookie),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let device_id = body["id"].as_str().expect("device id").to_string();
        let device_token = body["token"].as_str().expect("device token").to_string();

        let qr = db.mint_checkin_qr_token(&club_id, now).await.unwrap();
        let (status, landing, _) = get_json(&router, &format!("/api/c/demo/q/{qr}/landing")).await;
        assert_eq!(status, StatusCode::OK);
        let claim = landing["claimToken"].as_str().expect("claimToken");

        let (status, _, _) = post_json(
            &router,
            "/api/checkin",
            &format!(r#"{{"token":"{claim}"}}"#),
            Some(&guest_cookie),
            Some(&device_token),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let bad_q = format!(
            "id={device_id}&lat=-33.4&lon=-70.6&timestamp={}",
            now / 1000
        );
        let bad = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/osmand?{bad_q}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);

        let ok_q = format!(
            "id={device_token}&lat=-33.4&lon=-70.6&timestamp={}",
            now / 1000
        );
        let ok = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/osmand?{ok_q}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
    }
}
