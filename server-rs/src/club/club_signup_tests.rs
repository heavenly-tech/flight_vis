//! Club self-serve signup, verification, superadmin, and pilot register gates.

use crate::club::db::MagicConsumeError;
use crate::club::{
    db::now_ms,
    db_club_lifecycle::{validate_icao, PENDING_CLUB_TTL_MS},
    slug::validate_slug,
    ClubDb,
};
use serial_test::serial;

#[test]
fn slug_reserved_and_icao_rules() {
    assert!(validate_slug("api").is_err());
    assert!(validate_slug("live").is_err());
    assert!(validate_slug("heavenly-club").is_ok());
    assert!(validate_icao("SCFA"));
    assert!(!validate_icao("SC1A"));
    assert!(!validate_icao("SCF"));
}

#[tokio::test]
#[serial]
async fn pending_club_ttl_frees_slug() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let slug = format!("ttl-{}", &crate::club::auth::new_id()[..8]);
    let email = format!("ttl-{slug}@test");
    let (club_id, _) = db
        .signup_club("TTL Club", &slug, &email, None, now)
        .await
        .unwrap();
    assert!(db.club_id_for_slug(&slug).await.is_some());
    let stale_created = now - PENDING_CLUB_TTL_MS - 3_600_000;
    sqlx::query("UPDATE clubs SET created_at = $1 WHERE id = $2")
        .bind(stale_created)
        .bind(&club_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let removed = db.purge_stale_pending_clubs(now).await.unwrap();
    assert!(removed >= 1);
    assert!(db.club_id_for_slug(&slug).await.is_none());
    let (club_id2, _) = db
        .signup_club("TTL Club 2", &slug, "ttl2@test", None, now)
        .await
        .unwrap();
    assert_ne!(club_id, club_id2);
    let row = db.club_public_row(&club_id2).await.unwrap();
    assert_eq!(row.slug, slug);
    assert_eq!(row.status, "pending");
}

#[tokio::test]
#[serial]
async fn club_verify_token_expiry_and_reuse() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let slug = format!("verify-{}", &crate::club::auth::new_id()[..8]);
    let (club_id, token) = db
        .signup_club("Verify Club", &slug, "verify-club@test", Some("SCFA"), now)
        .await
        .unwrap();
    assert_eq!(
        db.club_public_row(&club_id).await.unwrap().status,
        "pending"
    );
    db.try_consume_magic_club_verify(&token, now).await.unwrap();
    let row = db.club_public_row(&club_id).await.unwrap();
    assert_eq!(row.status, "live");
    assert!(row.email_verified_at.is_some());
    assert!(matches!(
        db.try_consume_magic_club_verify(&token, now).await,
        Err(MagicConsumeError::Used)
    ));
    let fresh = db
        .issue_magic_club_verify(&club_id, "verify-club@test", now)
        .await
        .unwrap();
    assert!(matches!(
        db.try_consume_magic_club_verify(&fresh, now + 16 * 60 * 1000)
            .await,
        Err(MagicConsumeError::Expired)
    ));
}

#[tokio::test]
#[serial]
async fn honeypot_signup_ignored_at_api_layer() {
    use crate::club::api_tests::setup as api_setup;
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use tower::Service;
    let fx = api_setup().await;
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM clubs")
        .fetch_one(&fx.club.pool)
        .await
        .unwrap();
    let mut router = fx.router;
    let res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/clubs/signup")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    r#"{"name":"Bot","slug":"bot-club","email":"bot@test","website":"http://spam"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v.get("ok"), Some(&serde_json::json!(true)));
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM clubs")
        .fetch_one(&fx.club.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
#[serial]
async fn cas_club_unchanged_after_bootstrap_style_row() {
    let db = ClubDb::open_test().await.unwrap();
    let id = crate::club::auth::new_id();
    let now = now_ms();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, contact_email, email_verified_at,
         checkin_window_hours, latitude, longitude)
         VALUES ($1, 'CAS', 'cas', $2, 'live', 'admin@cas.test', $2, 12, -33.4489, -70.6693)",
    )
    .bind(&id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let row = db.club_public_row(&id).await.unwrap();
    assert_eq!(row.slug, "cas");
    assert_eq!(row.status, "live");
    assert!(row.email_verified_at.is_some());
    assert!(row.suspended_at.is_none());
}

#[tokio::test]
#[serial]
async fn pending_club_rejects_pilot_register_slug() {
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::Service;
    let data_dir = tempfile::tempdir().unwrap();
    let club = ClubDb::open_test().await.unwrap();
    let slug = format!("pending-{}", &crate::club::auth::new_id()[..8]);
    let _ = club
        .signup_club("Pending", &slug, "owner-pending@test", None, now_ms())
        .await
        .unwrap();
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir: data_dir.path().join("dist"),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live: Arc::new(LiveEngine::new(data_dir.path())),
        store: Store::new(data_dir.path()),
        club: Some(club),
        video_queue,
        login_throttle: crate::LoginThrottle::default(),
        device_register_throttle: crate::LoginThrottle::default(),
        share_lookup_throttle: crate::LoginThrottle::default(),
        account_password_throttle: crate::LoginThrottle::default(),
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
                .uri("/api/club/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"name":"P","email":"p-pending@test","password":"password12","slug":"{slug}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[serial]
async fn suspended_club_register_returns_suspended() {
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use std::sync::Arc;
    use tower::Service;
    let data_dir = tempfile::tempdir().unwrap();
    let db = ClubDb::open_test().await.unwrap();
    let slug = format!("susp-{}", &crate::club::auth::new_id()[..8]);
    let now = now_ms();
    let (club_id, token) = db
        .signup_club("Susp", &slug, "own-susp@test", None, now)
        .await
        .unwrap();
    db.try_consume_magic_club_verify(&token, now).await.unwrap();
    db.set_club_suspended(&club_id, now).await.unwrap();
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
                .uri("/api/club/register")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"name":"P","email":"p-susp@test","password":"password12","slug":"{slug}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"], "club suspended");
}
