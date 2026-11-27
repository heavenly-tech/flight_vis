//! Security review fixes: migration backfill, superadmin gate, magic POST, suspend revoke.

use crate::club::db::now_ms;
use crate::club::{auth::new_id, ClubDb};
use serial_test::serial;

#[tokio::test]
#[serial]
async fn live_club_null_email_verified_backfill_is_public() {
    let db = ClubDb::open_test().await.unwrap();
    let id = new_id();
    let now = now_ms();
    let slug = format!("legacy-{}", &id[..8]);
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at,
         checkin_window_hours, latitude, longitude)
         VALUES ($1, 'Legacy', $2, $3, 'live', NULL, 12, 0, 0)",
    )
    .bind(&id)
    .bind(&slug)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE clubs SET email_verified_at = created_at
         WHERE status = 'live' AND email_verified_at IS NULL",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let row = db.club_public_row(&id).await.unwrap();
    assert!(ClubDb::is_public_verified(&row));
    assert!(db.list_public_clubs().await.iter().any(|(s, _)| s == &slug));
}

#[tokio::test]
#[serial]
async fn migration_014_backfill_prod_cas_shape() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = new_id();
    let approved_id = new_id();
    let pending_id = new_id();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at,
         checkin_window_hours, latitude, longitude)
         VALUES ($1, 'CAS', 'cas', $2, 'live', NULL, 12, 0, 0)",
    )
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Admin', 'admin@cas', '', 'approved', 'admin', 'hash', $3),
                ($4, $2, 'Pending', 'pending@cas', '', 'pending', 'pilot', 'hash', $3)",
    )
    .bind(&approved_id)
    .bind(&club_id)
    .bind(now)
    .bind(&pending_id)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE clubs SET email_verified_at = created_at
         WHERE status = 'live' AND email_verified_at IS NULL",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE pilots SET email_verified_at = created_at
         WHERE status = 'approved' AND email_verified_at IS NULL",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    let approved_ver: Option<i64> =
        sqlx::query_scalar("SELECT email_verified_at FROM pilots WHERE id = $1")
            .bind(&approved_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let pending_ver: Option<i64> =
        sqlx::query_scalar("SELECT email_verified_at FROM pilots WHERE id = $1")
            .bind(&pending_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let club_ver: Option<i64> =
        sqlx::query_scalar("SELECT email_verified_at FROM clubs WHERE id = $1")
            .bind(&club_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(approved_ver, Some(now));
    assert!(pending_ver.is_none());
    assert_eq!(club_ver, Some(now));
}

#[tokio::test]
#[serial]
async fn magic_post_club_verify_browser_flow() {
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use axum::{
        body::to_bytes,
        http::{header, Request, StatusCode},
    };
    use std::{collections::HashSet, sync::Arc};
    use tower::Service;
    let data_dir = tempfile::tempdir().unwrap();
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let slug = format!("magic-post-{}", &new_id()[..8]);
    let (club_id, token) = db
        .signup_club("Magic Post", &slug, "magic-post@test", None, now)
        .await
        .unwrap();
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir: data_dir.path().join("dist"),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live: Arc::new(LiveEngine::new(data_dir.path())),
        store: Store::new(data_dir.path()),
        club: Some(db.clone()),
        video_queue,
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
        superadmin_emails: HashSet::new(),
    };
    let mut router = build_router(state);
    let get_res = router
        .call(
            Request::builder()
                .method("GET")
                .uri(format!("/api/club/auth/magic?token={token}"))
                .header(header::ACCEPT, "text/html")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    assert_eq!(
        get_res
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok()),
        Some(crate::html_csp::HTML_STRICT_CSP)
    );
    assert_eq!(
        get_res
            .headers()
            .get(header::REFERRER_POLICY)
            .and_then(|v| v.to_str().ok()),
        Some(crate::html_csp::SENSITIVE_REFERRER_POLICY)
    );
    let post_res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/club/auth/magic")
                .header(header::ACCEPT, "text/html")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(axum::body::Body::from(format!("token={token}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(post_res.status(), StatusCode::FOUND);
    let row = db.club_public_row(&club_id).await.unwrap();
    assert_eq!(row.status, "live");
    assert!(row.email_verified_at.is_some());
    let _ = to_bytes(post_res.into_body(), usize::MAX).await;
}

#[tokio::test]
#[serial]
async fn superadmin_allowlist_requires_verified_approved() {
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use std::{collections::HashSet, sync::Arc};
    use tower::Service;
    let data_dir = tempfile::tempdir().unwrap();
    let db = ClubDb::open_test().await.unwrap();
    let club_id = db.default_club_id().await.unwrap();
    let now = now_ms();
    let allow_pending = "super-pending@test".to_string();
    let allow_unverified = "super-unverified@test".to_string();
    let mut emails = HashSet::new();
    emails.insert(allow_pending.clone());
    emails.insert(allow_unverified.clone());
    let pending_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Pending SA', $3, '', 'pending', 'pilot', '', $4)",
    )
    .bind(&pending_id)
    .bind(&club_id)
    .bind(&allow_pending)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let unverified_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Unverified SA', $3, '', 'approved', 'pilot', '', $4)",
    )
    .bind(&unverified_id)
    .bind(&club_id)
    .bind(&allow_unverified)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let plain_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, 'Plain', 'plain-pilot@test', '', 'approved', 'pilot', '', $3, $3)",
    )
    .bind(&plain_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let session_pending = db.create_pilot_session(&pending_id, now).await.unwrap();
    let session_unverified = db.create_pilot_session(&unverified_id, now).await.unwrap();
    let session_plain = db.create_pilot_session(&plain_id, now).await.unwrap();
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir: data_dir.path().join("dist"),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live: Arc::new(LiveEngine::new(data_dir.path())),
        store: Store::new(data_dir.path()),
        club: Some(db),
        video_queue,
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
        superadmin_emails: emails,
    };
    let mut router = build_router(state);
    for cookie in [
        format!("fv_club_session={session_pending}"),
        format!("fv_club_session={session_unverified}"),
        format!("fv_club_session={session_plain}"),
    ] {
        let res = router
            .call(
                Request::builder()
                    .method("GET")
                    .uri("/api/superadmin/clubs")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
#[serial]
async fn suspend_revokes_device_and_pilot_sessions() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let slug = format!("revoke-{}", &new_id()[..8]);
    let (club_id, token) = db
        .signup_club("Revoke", &slug, "revoke-club@test", None, now)
        .await
        .unwrap();
    db.try_consume_magic_club_verify(&token, now).await.unwrap();
    let admin = sqlx::query_scalar::<_, String>(
        "SELECT id FROM pilots WHERE club_id = $1 AND role = 'admin' LIMIT 1",
    )
    .bind(&club_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let (device_id, dev_tok) = db.create_device(&admin, "phone").await.unwrap();
    let sess = db.create_pilot_session(&admin, now).await.unwrap();
    db.set_club_suspended(&club_id, now).await.unwrap();
    assert!(db.resolve_device_token(&dev_tok).await.is_none());
    assert!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM pilot_sessions WHERE id = $1")
            .bind(&sess)
            .fetch_one(&db.pool)
            .await
            .unwrap()
            == 0
    );
    let _ = device_id;
}

#[tokio::test]
#[serial]
async fn suspended_and_pending_clubs_forbidden_on_operational_routes() {
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use std::{collections::HashSet, sync::Arc};
    use tower::Service;

    async fn router_for_club(db: ClubDb) -> axum::Router {
        let data_dir = tempfile::tempdir().unwrap();
        let (video_queue, _) = tokio::sync::mpsc::channel(4);
        let state = AppState {
            dist_dir: data_dir.path().join("dist"),
            auth: Auth::with_password("x"),
            device_auth: DeviceAuth::empty(),
            live: Arc::new(LiveEngine::new(data_dir.path())),
            store: Store::new(data_dir.path()),
            club: Some(db),
            video_queue,
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
            superadmin_emails: HashSet::new(),
        };
        build_router(state)
    }

    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let slug = format!("pending-op-{}", &new_id()[..8]);
    let (pending_club, _tok) = db
        .signup_club("Pending Op", &slug, "pending-op@test", None, now)
        .await
        .unwrap();
    let pending_admin = sqlx::query_scalar::<_, String>(
        "SELECT id FROM pilots WHERE club_id = $1 AND role = 'admin' LIMIT 1",
    )
    .bind(&pending_club)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let pending_cookie = format!(
        "fv_club_session={}",
        db.create_pilot_session(&pending_admin, now).await.unwrap()
    );
    let mut router = router_for_club(db.clone()).await;
    let pending_me = router
        .call(
            Request::builder()
                .method("GET")
                .uri("/api/club/me")
                .header(header::COOKIE, &pending_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pending_me.status(), StatusCode::FORBIDDEN);

    let slug_s = format!("suspend-op-{}", &new_id()[..8]);
    let (club_id, verify_tok) = db
        .signup_club("Suspend Op", &slug_s, "suspend-op@test", None, now)
        .await
        .unwrap();
    db.try_consume_magic_club_verify(&verify_tok, now)
        .await
        .unwrap();
    let admin = sqlx::query_scalar::<_, String>(
        "SELECT id FROM pilots WHERE club_id = $1 AND role = 'admin' LIMIT 1",
    )
    .bind(&club_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE pilots SET email_verified_at = $1 WHERE id = $2")
        .bind(now)
        .bind(&admin)
        .execute(&db.pool)
        .await
        .unwrap();
    db.set_club_suspended(&club_id, now).await.unwrap();
    let checkin_token = db.mint_checkin_qr_token(&club_id, now).await.unwrap();
    let suspended_cookie = format!(
        "fv_club_session={}",
        db.create_pilot_session(&admin, now).await.unwrap()
    );
    let mut router = router_for_club(db).await;
    let checkin_body = format!(r#"{{"token":"{checkin_token}"}}"#);
    for (method, uri, body) in [
        ("GET", "/api/club/me", None::<String>),
        ("GET", "/api/club/map", None),
        ("POST", "/api/checkin", Some(checkin_body)),
    ] {
        let mut req = Request::builder().method(method).uri(uri);
        if method == "POST" {
            req = req.header(header::CONTENT_TYPE, "application/json");
        }
        let payload = body.unwrap_or_default();
        let res = router
            .call(
                req.header(header::COOKIE, &suspended_cookie)
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{method} {uri}");
    }
}

#[tokio::test]
#[serial]
async fn prod_style_null_verified_pilot_club_login_http() {
    use crate::club::auth::hash_password;
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::Service;
    let db = ClubDb::open_test().await.unwrap();
    let club_id = db.default_club_id().await.unwrap();
    let hash = hash_password("password12").unwrap();
    let now = now_ms();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Cas Admin', 'cas-legacy@test', '', 'approved', 'admin', $3, $4)",
    )
    .bind(new_id())
    .bind(&club_id)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir: data_dir.path().join("dist"),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live: Arc::new(LiveEngine::new(data_dir.path())),
        store: Store::new(data_dir.path()),
        club: Some(db),
        video_queue,
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
    let mut router = build_router(state);
    let res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/club/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"cas-legacy@test","password":"password12"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(res.headers().get(header::SET_COOKIE).is_some());
}

#[tokio::test]
#[serial]
async fn verified_allowlisted_superadmin_lists_clubs() {
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use std::{collections::HashSet, sync::Arc};
    use tower::Service;
    let db = ClubDb::open_test().await.unwrap();
    let club_id = db.default_club_id().await.unwrap();
    let now = now_ms();
    let email = "super-ok@test".to_string();
    let pid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, 'Super OK', $3, '', 'approved', 'pilot', '', $4, $4)",
    )
    .bind(&pid)
    .bind(&club_id)
    .bind(&email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let cookie = format!(
        "fv_club_session={}",
        db.create_pilot_session(&pid, now).await.unwrap()
    );
    let mut emails = HashSet::new();
    emails.insert(email);
    let data_dir = tempfile::tempdir().unwrap();
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir: data_dir.path().join("dist"),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live: Arc::new(LiveEngine::new(data_dir.path())),
        store: Store::new(data_dir.path()),
        club: Some(db),
        video_queue,
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
        superadmin_emails: emails,
    };
    let mut router = build_router(state);
    let res = router
        .call(
            Request::builder()
                .method("GET")
                .uri("/api/superadmin/clubs")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
#[serial]
async fn superadmin_cannot_suspend_default_club() {
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use std::{collections::HashSet, sync::Arc};
    use tower::Service;
    let db = ClubDb::open_test().await.unwrap();
    let default_id = db.default_club_id().await.unwrap();
    let now = now_ms();
    let pid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, 'Super', 'super-def@test', '', 'approved', 'pilot', '', $3, $3)",
    )
    .bind(&pid)
    .bind(&default_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let cookie = format!(
        "fv_club_session={}",
        db.create_pilot_session(&pid, now).await.unwrap()
    );
    let mut emails = HashSet::new();
    emails.insert("super-def@test".to_string());
    let data_dir = tempfile::tempdir().unwrap();
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir: data_dir.path().join("dist"),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live: Arc::new(LiveEngine::new(data_dir.path())),
        store: Store::new(data_dir.path()),
        club: Some(db),
        video_queue,
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
        superadmin_emails: emails,
    };
    let mut router = build_router(state);
    let res = router
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/api/superadmin/clubs/{default_id}/suspend"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[serial]
async fn checkin_gates_on_target_club_not_home_when_home_operational() {
    use crate::club::auth::hash_password;
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::Service;
    let db = ClubDb::open_test().await.unwrap();
    let home = db.default_club_id().await.unwrap();
    let now = now_ms();
    let slug = format!("suspend-chk-{}", &new_id()[..8]);
    let (other, tok) = db
        .signup_club("Chk Suspend", &slug, "chk-sus@test", None, now)
        .await
        .unwrap();
    db.try_consume_magic_club_verify(&tok, now).await.unwrap();
    let pid = new_id();
    let hash = hash_password("password12").unwrap();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $3, 'Visitor', 'visitor@test', '', 'approved', 'pilot', $2, $4)",
    )
    .bind(&pid)
    .bind(&hash)
    .bind(&other)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let qr = db.mint_checkin_qr_token(&other, now).await.unwrap();
    db.set_club_suspended(&other, now).await.unwrap();
    let cookie = format!(
        "fv_club_session={}",
        db.create_pilot_session(&pid, now).await.unwrap()
    );
    let _ = home;
    let data_dir = tempfile::tempdir().unwrap();
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir: data_dir.path().join("dist"),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live: Arc::new(LiveEngine::new(data_dir.path())),
        store: Store::new(data_dir.path()),
        club: Some(db),
        video_queue,
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
    let mut router = build_router(state);
    let res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/checkin")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"token":"{qr}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

async fn apply_migration_015(pool: &sqlx::PgPool) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("migrations/000015_backfill_club_memberships.sql");
    let sql = std::fs::read_to_string(&path).expect("000015 migration file");
    sqlx::query(&sql).execute(pool).await.unwrap();
}

#[tokio::test]
#[serial]
async fn migration_015_prod_cas_one_admin_membership() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = new_id();
    let admin_id = new_id();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at,
         checkin_window_hours, latitude, longitude)
         VALUES ($1, 'CAS', 'cas', $2, 'live', $2, 12, -33.4489, -70.6693)",
    )
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'CAS Admin', 'admin@cas', '', 'approved', 'admin', 'hash', $3),
                ($4, $2, 'Guest', 'guest@cas', '', 'guest', 'pilot', '', $3)",
    )
    .bind(&admin_id)
    .bind(&club_id)
    .bind(now)
    .bind(new_id())
    .execute(&db.pool)
    .await
    .unwrap();
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM club_memberships")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(before, 0);
    apply_migration_015(&db.pool).await;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM club_memberships")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let member: (String, String) = sqlx::query_as(
        "SELECT role, status FROM club_memberships WHERE pilot_id = $1 AND club_id = $2",
    )
    .bind(&admin_id)
    .bind(&club_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(member, ("admin".to_string(), "approved".to_string()));
}

#[tokio::test]
#[serial]
async fn prod_admin_admin_api_200_after_membership_backfill() {
    use crate::club::auth::hash_password;
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::Service;

    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let admin_id = new_id();
    let hash = hash_password("password12").unwrap();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Admin', 'prod-admin@test', '', 'approved', 'admin', $3, $4)",
    )
    .bind(&admin_id)
    .bind(&club_id)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    apply_migration_015(&db.pool).await;
    assert!(db.require_club_admin(&admin_id, &club_id).await.is_ok());
    let data_dir = tempfile::tempdir().unwrap();
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir: data_dir.path().join("dist"),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live: Arc::new(LiveEngine::new(data_dir.path())),
        store: Store::new(data_dir.path()),
        club: Some(db),
        video_queue,
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
    let mut router = build_router(state);
    let login = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/club/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"email":"prod-admin@test","password":"password12"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_string();
    for uri in ["/api/admin/pilots", "/api/display/map"] {
        let res = router
            .call(
                Request::builder()
                    .uri(uri)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{uri}");
    }
}

#[tokio::test]
#[serial]
async fn approve_pilot_with_checkout_upserts_membership_row() {
    use crate::club::auth::hash_password;
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let hash = hash_password("password12").unwrap();
    let admin_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, 'Admin', 'club-admin@test', '', 'approved', 'admin', $3, $4, $4)",
    )
    .bind(&admin_id)
    .bind(&club_id)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.ensure_membership_row(&admin_id, &club_id, "admin", "approved")
        .await
        .unwrap();
    let pending_id = db
        .register(&club_id, "New", "pending-appr@test", "", "password12")
        .await
        .unwrap();
    db.mark_email_verified(&pending_id, now).await.unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM club_memberships WHERE pilot_id = $1 AND club_id = $2",
    )
    .bind(&pending_id)
    .bind(&club_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
    db.approve_pilot_with_checkout(&admin_id, &pending_id, None)
        .await
        .unwrap();
    let row: (String, String) = sqlx::query_as(
        "SELECT role, status FROM club_memberships WHERE pilot_id = $1 AND club_id = $2",
    )
    .bind(&pending_id)
    .bind(&club_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(row, ("pilot".to_string(), "active".to_string()));
}

#[tokio::test]
#[serial]
async fn pilot_is_approved_member_after_membership_backfill() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let pilot_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'P', 'map-member@test', '', 'approved', 'pilot', '', $3)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(!db.pilot_is_approved_member(&pilot_id, &club_id).await);
    apply_migration_015(&db.pool).await;
    assert!(db.pilot_is_approved_member(&pilot_id, &club_id).await);
}

#[tokio::test]
#[serial]
async fn bootstrap_email_exists_lookup_is_case_insensitive() {
    use std::sync::{Mutex, MutexGuard};
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    let saved_email = std::env::var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_EMAIL").ok();
    let saved_pw = std::env::var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_PASSWORD").ok();
    {
        let _guard: MutexGuard<'_, ()> = ENV_LOCK.lock().unwrap();
        std::env::set_var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_EMAIL", "Admin@Cas.example");
        std::env::set_var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_PASSWORD", "password12");
    }
    let db = ClubDb::open_test().await.unwrap();
    db.bootstrap().await.unwrap();
    let club_id = db.default_club_id().await.unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pilots WHERE club_id = $1 AND lower(email) = lower($2)",
    )
    .bind(&club_id)
    .bind("admin@cas.example")
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
    {
        let _guard: MutexGuard<'_, ()> = ENV_LOCK.lock().unwrap();
        match saved_email {
            Some(v) => std::env::set_var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_EMAIL", v),
            None => std::env::remove_var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_EMAIL"),
        }
        match saved_pw {
            Some(v) => std::env::set_var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_PASSWORD", v),
            None => std::env::remove_var("FLIGHT_CLUB_BOOTSTRAP_ADMIN_PASSWORD"),
        }
    }
}

#[tokio::test]
#[serial]
async fn passkey_login_path_preserves_null_email_verified() {
    use crate::club::db::PilotRow;
    use crate::club::webauthn::WebauthnState;
    let db = ClubDb::open_test().await.unwrap();
    let club_id = db.default_club_id().await.unwrap();
    let pilot_id = new_id();
    let now = now_ms();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'PK', 'passkey@test', '', 'approved', 'pilot', '', $3)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pilot = PilotRow {
        id: pilot_id.clone(),
        club_id: club_id.clone(),
        name: "PK".to_string(),
        email: "passkey@test".to_string(),
        license_no: String::new(),
        status: "approved".to_string(),
        role: "pilot".to_string(),
        locale: None,
    };
    let _ = WebauthnState::complete_passkey_login(&db, &pilot)
        .await
        .expect("passkey post-login session");
    let verified: Option<i64> =
        sqlx::query_scalar("SELECT email_verified_at FROM pilots WHERE id = $1")
            .bind(&pilot_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(verified.is_none());
}

#[tokio::test]
#[serial]
async fn allowlisted_passkey_pilot_unverified_not_platform_superadmin() {
    use crate::club::db::PilotRow;
    use crate::club::webauthn::WebauthnState;
    use std::collections::HashSet;
    let db = ClubDb::open_test().await.unwrap();
    let club_id = db.default_club_id().await.unwrap();
    let now = now_ms();
    let pilot_id = new_id();
    let email = "super-passkey@test".to_string();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'SA', $3, '', 'approved', 'pilot', '', $4)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(&email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pilot = PilotRow {
        id: pilot_id.clone(),
        club_id,
        name: "SA".to_string(),
        email: email.clone(),
        license_no: String::new(),
        status: "approved".to_string(),
        role: "pilot".to_string(),
        locale: None,
    };
    let _ = WebauthnState::complete_passkey_login(&db, &pilot)
        .await
        .expect("passkey post-login session");
    let mut allow = HashSet::new();
    allow.insert(email);
    assert!(!db.is_platform_superadmin(&pilot_id, &allow).await);
}

#[tokio::test]
#[serial]
async fn pending_admin_pilot_admin_api_403() {
    use crate::club::auth::hash_password;
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::Service;

    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let admin_id = new_id();
    let hash = hash_password("password12").unwrap();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Pending Admin', 'pending-admin@test', '', 'pending', 'admin', $3, $4)",
    )
    .bind(&admin_id)
    .bind(&club_id)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    apply_migration_015(&db.pool).await;
    let cookie = format!(
        "fv_club_session={}",
        db.create_pilot_session(&admin_id, now).await.unwrap()
    );
    let data_dir = tempfile::tempdir().unwrap();
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir: data_dir.path().join("dist"),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live: Arc::new(LiveEngine::new(data_dir.path())),
        store: Store::new(data_dir.path()),
        club: Some(db),
        video_queue,
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
    let mut router = build_router(state);
    for uri in ["/api/admin/pilots", "/api/display/map"] {
        let res = router
            .call(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{uri}");
    }
}
