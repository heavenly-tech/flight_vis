//! L-18 / L-23: cross-site POST CSRF (explicit test names).

use crate::club::auth::{hash_password, new_id, set_account_session_cookie, set_session_cookie};
use crate::club::{db::now_ms, ClubDb};
use crate::{
    auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
};
use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use serial_test::serial;
use std::{collections::HashSet, sync::Arc};
use tower::Service;

struct CsrfFixture {
    router: axum::Router,
    db: ClubDb,
    account_cookie: String,
    admin_cookie: String,
    superadmin_cookie: String,
    target_account_id: String,
}

async fn csrf_fixture() -> CsrfFixture {
    let data_dir = tempfile::tempdir().unwrap();
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let target_account_id = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, created_at, email_verified_at)
         VALUES ($1, 'csrf-victim@test', $2, $2)",
    )
    .bind(&target_account_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let acct_sess = db
        .create_account_session(&target_account_id, None, now)
        .await
        .unwrap();
    let account_cookie = set_account_session_cookie(&acct_sess, false);
    let club = db.default_club_id().await.unwrap();
    let hash = hash_password("password12").unwrap();
    let pid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Adm', 'adm-csrf@test', '', 'approved', 'admin', $3, $4)",
    )
    .bind(&pid)
    .bind(&club)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.ensure_membership_row(&pid, &club, "admin", "active")
        .await
        .unwrap();
    let sa_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, 'SA', 'sa-csrf@test', '', 'approved', 'pilot', $3, $4, $4)",
    )
    .bind(&sa_id)
    .bind(&club)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pilot_sess = db.create_pilot_session(&pid, now).await.unwrap();
    let admin_cookie = set_session_cookie(&pilot_sess, false);
    let sa_sess = db.create_pilot_session(&sa_id, now).await.unwrap();
    let superadmin_cookie = set_session_cookie(&sa_sess, false);
    let mut superadmin_emails = HashSet::new();
    superadmin_emails.insert("sa-csrf@test".to_string());
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
        mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
        webauthn: None,
        sse_limiter: std::sync::Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails,
    };
    CsrfFixture {
        router: build_router(state),
        db,
        account_cookie,
        admin_cookie,
        superadmin_cookie,
        target_account_id,
    }
}

#[tokio::test]
#[serial]
async fn cross_site_account_join_post_forbidden() {
    let mut fx = csrf_fixture().await;
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/join")
                .header(header::COOKIE, &fx.account_cookie)
                .header(header::ORIGIN, "https://evil.example")
                .header(header::HOST, "127.0.0.1:8080")
                .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                .body(axum::body::Body::from(r#"{"slug":"demo","name":"X"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial]
async fn cross_site_admin_membership_invite_post_forbidden() {
    let mut fx = csrf_fixture().await;
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/memberships/invite")
                .header(header::COOKIE, &fx.admin_cookie)
                .header(header::ORIGIN, "https://evil.example")
                .header(header::HOST, "127.0.0.1:8080")
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(r#"{"email":"x@test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial]
async fn cross_site_superadmin_account_suspend_post_forbidden() {
    let mut fx = csrf_fixture().await;
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/superadmin/accounts/{}/suspend",
                    fx.target_account_id
                ))
                .header(header::COOKIE, &fx.superadmin_cookie)
                .header(header::ORIGIN, "https://evil.example")
                .header(header::HOST, "127.0.0.1:8080")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial]
async fn cross_site_club_login_json_post_forbidden() {
    use axum::body::Body;
    let data_dir = tempfile::tempdir().unwrap();
    let db = ClubDb::open_test().await.unwrap();
    let club_id = db.default_club_id().await.unwrap();
    let now = now_ms();
    let hash = hash_password("password12").unwrap();
    let pilot_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, 'X', 'csrf-clublogin@test', '', 'approved', 'pilot', $3, $4, $4)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
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
        mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
        webauthn: None,
        sse_limiter: std::sync::Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails: HashSet::new(),
    };
    let mut router = build_router(state);
    let res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/club/login")
                .header(header::ORIGIN, "https://evil.example")
                .header(header::HOST, "127.0.0.1:8080")
                .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                .header(header::ACCEPT, "application/json")
                .body(Body::from(
                    r#"{"email":"csrf-clublogin@test","password":"password12"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial]
async fn cross_site_account_login_json_post_forbidden() {
    let data_dir = tempfile::tempdir().unwrap();
    let db = ClubDb::open_test().await.unwrap();
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
        mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
        webauthn: None,
        sse_limiter: std::sync::Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails: HashSet::new(),
    };
    let mut router = build_router(state);
    let res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/login")
                .header(header::ORIGIN, "https://evil.example")
                .header(header::HOST, "127.0.0.1:8080")
                .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
                .header(header::ACCEPT, "application/json")
                .body(Body::from(
                    r#"{"email":"x@test","password":"password1234"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial]
async fn cross_site_admin_membership_promote_post_forbidden() {
    let mut fx = csrf_fixture().await;
    let staff_mid = new_id();
    let club = fx.db.default_club_id().await.unwrap();
    let staff_pid = new_id();
    let now = now_ms();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Stf', 'promote-staff@test', '', 'approved', 'staff', 'x', $3)",
    )
    .bind(&staff_pid)
    .bind(&club)
    .bind(now)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    fx.db
        .ensure_membership_row(&staff_pid, &club, "staff", "active")
        .await
        .unwrap();
    sqlx::query(
        "UPDATE club_memberships SET id = $1 WHERE pilot_id = $2 AND club_id = $3",
    )
    .bind(&staff_mid)
    .bind(&staff_pid)
    .bind(&club)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/api/admin/memberships/{staff_mid}/promote"))
                .header(header::COOKIE, &fx.admin_cookie)
                .header(header::ORIGIN, "https://evil.example")
                .header(header::HOST, "127.0.0.1:8080")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial]
async fn cross_site_admin_membership_remove_post_forbidden() {
    let mut fx = csrf_fixture().await;
    let mid = new_id();
    let club = fx.db.default_club_id().await.unwrap();
    let pid = new_id();
    let now = now_ms();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Rm', 'remove-me@test', '', 'approved', 'pilot', 'x', $3)",
    )
    .bind(&pid)
    .bind(&club)
    .bind(now)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'pilot', 'active', $4)",
    )
    .bind(&mid)
    .bind(&pid)
    .bind(&club)
    .bind(now)
    .execute(&fx.db.pool)
    .await
    .unwrap();
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/api/admin/memberships/{mid}/remove"))
                .header(header::COOKIE, &fx.admin_cookie)
                .header(header::ORIGIN, "https://evil.example")
                .header(header::HOST, "127.0.0.1:8080")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial]
async fn cross_site_account_membership_approve_post_forbidden() {
    let mut fx = csrf_fixture().await;
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/memberships/nope/approve")
                .header(header::COOKIE, &fx.account_cookie)
                .header(header::ORIGIN, "https://evil.example")
                .header(header::HOST, "127.0.0.1:8080")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial]
async fn cross_site_superadmin_account_unsuspend_post_forbidden() {
    let mut fx = csrf_fixture().await;
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/api/superadmin/accounts/{}/unsuspend",
                    fx.target_account_id
                ))
                .header(header::COOKIE, &fx.superadmin_cookie)
                .header(header::ORIGIN, "https://evil.example")
                .header(header::HOST, "127.0.0.1:8080")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
