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
async fn suspend_revokes_device_and_display_tokens() {
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
    let (_disp_id, disp_tok) = db.create_display_session(&admin, "kiosk").await.unwrap();
    db.set_club_suspended(&club_id, now).await.unwrap();
    assert!(db.resolve_device_token(&dev_tok).await.is_none());
    assert!(db.resolve_display_session(&disp_tok).await.is_none());
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
    let suspended_cookie = format!(
        "fv_club_session={}",
        db.create_pilot_session(&admin, now).await.unwrap()
    );
    let mut router = router_for_club(db).await;
    for (method, uri, body) in [
        ("GET", "/api/club/me", None),
        ("GET", "/api/club/map", None),
        ("POST", "/api/checkin", Some(r#"{"token":"x"}"#)),
    ] {
        let mut req = Request::builder().method(method).uri(uri);
        if method == "POST" {
            req = req.header(header::CONTENT_TYPE, "application/json");
        }
        let res = router
            .call(
                req.header(header::COOKIE, &suspended_cookie)
                    .body(Body::from(body.unwrap_or("")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{method} {uri}");
    }
}
