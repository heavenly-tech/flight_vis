//! Jordan / Kepler follow-up security tests (PR #24).

use crate::club::auth::{hash_password, new_id};
use crate::club::db::LoginError;
use crate::club::{db::now_ms, ClubDb};
use serial_test::serial;
use sqlx::Row;
use std::collections::HashSet;

#[tokio::test]
#[serial]
async fn first_magic_verify_wipes_precreated_password_and_sessions() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "verify-wipe@test";
    let hash = hash_password("precreated12").unwrap();
    let id = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, password_hash, created_at)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(&id)
    .bind(email)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let sess = db.create_account_session(&id, None, now).await.unwrap();
    let raw = db
        .issue_account_magic_login(email, now)
        .await
        .unwrap()
        .unwrap();
    let _ = db.consume_account_magic(&raw, None, now).await.unwrap();
    let hash_after: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM accounts WHERE id = $1")
            .bind(&id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(hash_after.as_deref().unwrap_or("").is_empty());
    assert!(db.account_from_session(&sess, now).await.is_none());
}

#[tokio::test]
#[serial]
async fn superadmin_account_global_role_requires_verified_approved_pilot() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let email = "sa-gated@test";
    let acct = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, global_role, created_at, email_verified_at)
         VALUES ($1, $2, 'superadmin', $3, $3)",
    )
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pilot_pending = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, $3, 'P', $4, '', 'pending', 'pilot', '', $5)",
    )
    .bind(&pilot_pending)
    .bind(&club_id)
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let allow = HashSet::new();
    assert!(!db.is_platform_superadmin(&pilot_pending, &allow).await);
    sqlx::query("UPDATE pilots SET status = 'approved', email_verified_at = $1 WHERE id = $2")
        .bind(now)
        .bind(&pilot_pending)
        .execute(&db.pool)
        .await
        .unwrap();
    assert!(db.is_platform_superadmin(&pilot_pending, &allow).await);
}

#[tokio::test]
#[serial]
async fn approve_membership_rejects_non_pending() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let pilot_id = new_id();
    let mid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'R', 'removed-admin@test', '', 'approved', 'admin', 'x', $3)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'admin', 'removed', $4)",
    )
    .bind(&mid)
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(db.approve_membership(&mid, now).await.is_err());
}

#[tokio::test]
#[serial]
async fn suspended_account_blocks_legacy_pilot_password_login() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let hash = hash_password("password12").unwrap();
    let acct = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, password_hash, created_at, email_verified_at, suspended_at)
         VALUES ($1, 'legacy-sus@test', $2, $3, $3, $3)",
    )
    .bind(&acct)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, $3, 'L', 'legacy-sus@test', '', 'approved', 'pilot', $4, $5, $5)",
    )
    .bind(new_id())
    .bind(&club_id)
    .bind(&acct)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pid: String = sqlx::query_scalar("SELECT id FROM pilots WHERE account_id = $1")
        .bind(&acct)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let row = sqlx::query("SELECT club_id FROM pilots WHERE id = $1")
        .bind(&pid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let cid: String = row.try_get(0).unwrap();
    assert!(matches!(
        db.login_password(&cid, "legacy-sus@test", "password12", now, "127.0.0.1")
            .await,
        Err(LoginError::Invalid)
    ));
}

#[tokio::test]
#[serial]
async fn pilot_session_for_account_reuses_one_row_per_account_session() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let email = "reuse-ps@test";
    let acct = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, created_at, email_verified_at)
         VALUES ($1, $2, $3, $3)",
    )
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pilot_id = new_id();
    let mid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, $3, 'R', $4, '', 'approved', 'pilot', '', $5)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'pilot', 'active', $4)",
    )
    .bind(&mid)
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let acct_sess = db
        .create_account_session(&acct, Some(&mid), now)
        .await
        .unwrap();
    let count_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM pilot_sessions WHERE pilot_id = $1")
            .bind(&pilot_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let _ = db.pilot_session_for_account(&acct_sess, now).await.unwrap();
    let _ = db.pilot_session_for_account(&acct_sess, now).await.unwrap();
    let stored: String =
        sqlx::query_scalar("SELECT compat_pilot_session FROM account_sessions WHERE id = $1")
            .bind(crate::club::auth::digest_token(&acct_sess))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(stored.len(), 64);
    let count_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM pilot_sessions WHERE pilot_id = $1")
            .bind(&pilot_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(count_after, count_before + 1);
}

#[tokio::test]
#[serial]
async fn admin_invite_ignores_pending_lookalike_pilot() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let email = "lookalike-admin@test";
    let attacker_pilot = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Evil', $3, '', 'pending', 'admin', 'x', $4)",
    )
    .bind(&attacker_pilot)
    .bind(&club_id)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let inviter = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Inv', 'inv-lookalike@test', '', 'approved', 'admin', 'x', $3)",
    )
    .bind(&inviter)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let raw = db
        .invite_admin_membership(&club_id, email, &inviter, now)
        .await
        .unwrap();
    let accept = db.accept_admin_invite(&raw, now).await.unwrap();
    let promoted: String =
        sqlx::query_scalar("SELECT pilot_id FROM club_memberships WHERE id = $1")
            .bind(&accept.membership_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_ne!(promoted, attacker_pilot);
    let attacker_gone: bool =
        sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM pilots WHERE id = $1)")
            .bind(&attacker_pilot)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(attacker_gone);
}

#[tokio::test]
#[serial]
async fn account_magic_get_does_not_consume_token() {
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
    let email = "scan-safe@test";
    let nonce = crate::club::auth::new_session_id();
    let raw = db
        .issue_account_magic_signup(email, None, Some(&nonce), now)
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
                .uri(format!("/api/account/auth/magic?token={raw}"))
                .header(header::ACCEPT, "text/html")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let _ = to_bytes(get_res.into_body(), usize::MAX).await;
    let used: Option<i64> =
        sqlx::query_scalar("SELECT used_at FROM magic_login_tokens WHERE token_hash = $1")
            .bind(crate::club::auth::digest_token(&raw))
            .fetch_optional(&db.pool)
            .await
            .unwrap()
            .flatten();
    assert!(used.is_none());
}

#[tokio::test]
#[serial]
async fn membership_remove_revokes_pilot_sessions() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let inviter = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Keep', 'keep-sess@test', '', 'approved', 'admin', 'x', $3)",
    )
    .bind(&inviter)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.ensure_membership_row(&inviter, &club_id, "admin", "active")
        .await
        .unwrap();
    let victim = new_id();
    let mid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Victim', 'victim-sess@test', '', 'approved', 'admin', 'x', $3)",
    )
    .bind(&victim)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'admin', 'active', $4)",
    )
    .bind(&mid)
    .bind(&victim)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let sess = db.create_pilot_session(&victim, now).await.unwrap();
    db.remove_admin_membership(&club_id, &mid).await.unwrap();
    assert!(db.pilot_from_session(&sess, now).await.is_none());
}

#[tokio::test]
#[serial]
async fn staff_may_approve_checkins_admin_only_for_types() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let staff_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Staff', 'staff-chk@test', '', 'approved', 'staff', 'x', $3)",
    )
    .bind(&staff_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.ensure_membership_row(&staff_id, &club_id, "staff", "active")
        .await
        .unwrap();
    assert!(db
        .require_club_checkin_approver(&staff_id, &club_id)
        .await
        .is_ok());
    assert!(db.require_club_admin(&staff_id, &club_id).await.is_err());
}

#[tokio::test]
#[serial]
async fn last_admin_guard_counts_after_018_normalize() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = new_id();
    let admin_id = new_id();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at,
         checkin_window_hours, latitude, longitude)
         VALUES ($1, 'CAS2', 'cas2', $2, 'live', $2, 12, -33.4489, -70.6693)",
    )
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'CAS Admin', 'admin@cas2', '', 'approved', 'admin', 'hash', $3)",
    )
    .bind(&admin_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("migrations/000015_backfill_club_memberships.sql");
    sqlx::query(&std::fs::read_to_string(path).unwrap())
        .execute(&db.pool)
        .await
        .unwrap();
    let path18 = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("migrations/000018_account_email_proof.sql");
    for stmt in std::fs::read_to_string(path18)
        .unwrap()
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with("--"))
    {
        sqlx::query(stmt).execute(&db.pool).await.unwrap();
    }
    let mid: String =
        sqlx::query_scalar("SELECT id FROM club_memberships WHERE pilot_id = $1 AND club_id = $2")
            .bind(&admin_id)
            .bind(&club_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM club_memberships WHERE id = $1")
        .bind(&mid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status, "active");
    assert_eq!(
        db.remove_admin_membership(&club_id, &mid).await,
        Err(crate::club::db_memberships_admin::MembershipAdminError::LastAdmin)
    );
}

#[tokio::test]
#[serial]
async fn suspended_membership_stops_account_compat_session() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let email = "sse-cut@test";
    let acct = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, created_at, email_verified_at)
         VALUES ($1, $2, $3, $3)",
    )
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pilot_id = new_id();
    let mid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, $3, 'S', $4, '', 'approved', 'pilot', '', $5)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'pilot', 'active', $4)",
    )
    .bind(&mid)
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let acct_sess = db
        .create_account_session(&acct, Some(&mid), now)
        .await
        .unwrap();
    assert!(db
        .pilot_session_for_account(&acct_sess, now)
        .await
        .is_some());
    db.suspend_membership(&mid).await.unwrap();
    assert!(db
        .pilot_session_for_account(&acct_sess, now)
        .await
        .is_none());
}

#[tokio::test]
#[serial]
async fn backfill_startup_uses_max_pilot_email_verified_at() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_a = db.default_club_id().await.unwrap();
    let club_b = new_id();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at,
         checkin_window_hours, latitude, longitude)
         VALUES ($1, 'B', 'club-b-bf', $2, 'live', $2, 12, -33.4489, -70.6693)",
    )
    .bind(&club_b)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let email = "backfill-verified@test";
    let early = now - 50_000;
    let late = now - 1_000;
    let p1 = new_id();
    let p2 = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, 'A', $3, '', 'approved', 'pilot', '', $4, $5)",
    )
    .bind(&p1)
    .bind(&club_a)
    .bind(email)
    .bind(early)
    .bind(early)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, 'B', $3, '', 'approved', 'pilot', '', $4, $5)",
    )
    .bind(&p2)
    .bind(&club_b)
    .bind(email)
    .bind(late)
    .bind(late)
    .execute(&db.pool)
    .await
    .unwrap();
    db.backfill_accounts_from_pilots().await.unwrap();
    let verified: Option<i64> = sqlx::query_scalar(
        "SELECT email_verified_at FROM accounts WHERE lower(trim(email)) = lower(trim($1))",
    )
    .bind(email)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(verified, Some(late));
}

#[tokio::test]
#[serial]
async fn backfill_on_conflict_keeps_existing_membership_role() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let pilot_id = new_id();
    let mid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, 'Adm', 'role-keep@test', '', 'approved', 'pilot', 'x', $3, $3)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'admin', 'active', $4)",
    )
    .bind(&mid)
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE pilots SET role = 'pilot' WHERE id = $1")
        .bind(&pilot_id)
        .execute(&db.pool)
        .await
        .unwrap();
    db.backfill_accounts_from_pilots().await.unwrap();
    let role: String = sqlx::query_scalar("SELECT role FROM club_memberships WHERE id = $1")
        .bind(&mid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(role, "admin");
}

#[tokio::test]
#[serial]
async fn webauthn_login_finish_rejects_invalid_nonce_via_http() {
    use crate::auth::Auth;
    use crate::club::webauthn::WebauthnState;
    use crate::{build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState};
    use axum::http::{header, Request, StatusCode};
    use std::sync::Arc;
    use tower::Service;
    std::env::set_var("WEBAUTHN_RP_ID", "localhost");
    std::env::set_var("FLIGHT_CLUB_PUBLIC_URL", "http://localhost:8080");
    let webauthn = WebauthnState::from_env().expect("webauthn test rp");
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
        account_password_throttle: crate::LoginThrottle::default(),
        mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
        webauthn: Some(webauthn),
        sse_limiter: std::sync::Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails: HashSet::new(),
    };
    let mut router = build_router(state);
    let res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/club/auth/webauthn/login/finish")
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(
                    r#"{"nonce":"not-a-real-nonce","credential":{"id":"x","rawId":"x","type":"public-key","response":{"authenticatorData":"e","clientDataJSON":"e","signature":"e"}}}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        res.status() == StatusCode::BAD_REQUEST || res.status() == StatusCode::UNPROCESSABLE_ENTITY,
        "unexpected status {}",
        res.status()
    );
}

#[tokio::test]
#[serial]
async fn account_signup_nonce_same_browser_applies_pending_password() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "nonce-keep@test";
    let pw = "password1234";
    let nonce = crate::club::auth::new_session_id();
    let _ = db.ensure_account_for_email(email, None, now).await.unwrap();
    let raw = db
        .issue_account_magic_signup(email, Some(pw), Some(&nonce), now)
        .await
        .unwrap();
    let _ = db
        .consume_account_magic(&raw, Some(&nonce), now)
        .await
        .unwrap();
    assert!(db
        .account_login_password(email, pw, now, "127.0.0.1")
        .await
        .is_ok());
}

#[tokio::test]
#[serial]
async fn account_signup_without_matching_nonce_skips_pending_password() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "nonce-skip@test";
    let pw = "password1234";
    let nonce = crate::club::auth::new_session_id();
    let _ = db.ensure_account_for_email(email, None, now).await.unwrap();
    let raw = db
        .issue_account_magic_signup(email, Some(pw), Some(&nonce), now)
        .await
        .unwrap();
    let wrong = crate::club::auth::new_session_id();
    let _ = db
        .consume_account_magic(&raw, Some(&wrong), now)
        .await
        .unwrap();
    assert!(matches!(
        db.account_login_password(email, pw, now, "127.0.0.1").await,
        Err(LoginError::Invalid)
    ));
}

#[tokio::test]
#[serial]
async fn victim_signup_token_wins_over_attacker_pending_password() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "victim-wins@test";
    let attacker_pw = "attackerpass1";
    let victim_pw = "victimpass12";
    let attacker_nonce = crate::club::auth::new_session_id();
    let _ = db.ensure_account_for_email(email, None, now).await.unwrap();
    let _ = db
        .issue_account_magic_signup(email, Some(attacker_pw), Some(&attacker_nonce), now)
        .await
        .unwrap();
    let victim_nonce = crate::club::auth::new_session_id();
    let victim_raw = db
        .issue_account_magic_signup(email, Some(victim_pw), Some(&victim_nonce), now)
        .await
        .unwrap();
    let _ = db
        .consume_account_magic(&victim_raw, Some(&victim_nonce), now)
        .await
        .unwrap();
    assert!(db
        .account_login_password(email, victim_pw, now, "127.0.0.1")
        .await
        .is_ok());
    assert!(matches!(
        db.account_login_password(email, attacker_pw, now, "127.0.0.1")
            .await,
        Err(LoginError::Invalid)
    ));
}

#[tokio::test]
#[serial]
async fn attacker_signup_victim_login_magic_clears_password() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "magic-wipe@test";
    let attacker_pw = "attackerpass1";
    let attacker_nonce = crate::club::auth::new_session_id();
    let id = db.ensure_account_for_email(email, None, now).await.unwrap();
    sqlx::query("UPDATE accounts SET password_hash = $1 WHERE id = $2")
        .bind(hash_password(attacker_pw).unwrap())
        .bind(&id)
        .execute(&db.pool)
        .await
        .unwrap();
    let _ = db
        .issue_account_magic_signup(email, Some(attacker_pw), Some(&attacker_nonce), now)
        .await
        .unwrap();
    let login_raw = db
        .issue_account_magic_login(email, now)
        .await
        .unwrap()
        .unwrap();
    let _ = db
        .consume_account_magic(&login_raw, None, now)
        .await
        .unwrap();
    assert!(matches!(
        db.account_login_password(email, attacker_pw, now, "127.0.0.1")
            .await,
        Err(LoginError::Invalid)
    ));
}

#[tokio::test]
#[serial]
async fn honest_signup_set_password_after_verify_works() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "set-pw-after@test";
    let pw = "honestpass12";
    let nonce = crate::club::auth::new_session_id();
    let _ = db.ensure_account_for_email(email, None, now).await.unwrap();
    let raw = db
        .issue_account_magic_signup(email, None, Some(&nonce), now)
        .await
        .unwrap();
    let acct = db
        .consume_account_magic(&raw, Some(&nonce), now)
        .await
        .unwrap();
    db.set_account_password(&acct.id, pw, now, None, None)
        .await
        .unwrap();
    assert!(db
        .account_login_password(email, pw, now, "127.0.0.1")
        .await
        .is_ok());
}

#[tokio::test]
#[serial]
async fn account_login_magic_http_roundtrip_sets_session_cookie() {
    use crate::auth::Auth;
    use crate::{build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState};
    use axum::http::{header, Request, StatusCode};
    use std::sync::Arc;
    use tower::Service;
    let data_dir = tempfile::tempdir().unwrap();
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "magic-login-http@test";
    let id = db.ensure_account_for_email(email, None, now).await.unwrap();
    sqlx::query("UPDATE accounts SET email_verified_at = $1 WHERE id = $2")
        .bind(now)
        .bind(&id)
        .execute(&db.pool)
        .await
        .unwrap();
    let raw = db
        .issue_account_magic_login(email, now)
        .await
        .unwrap()
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
        account_password_throttle: crate::LoginThrottle::default(),
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
                .uri(format!("/api/account/auth/magic?token={raw}"))
                .header(header::ACCEPT, "text/html")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    assert!(get_res
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .is_some());
    let post_res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/auth/magic")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::HOST, "127.0.0.1:8080")
                .body(axum::body::Body::from(format!(
                    "token={raw}&purpose=ignored"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(post_res.status(), StatusCode::FOUND);
    assert!(post_res
        .headers()
        .get(header::SET_COOKIE)
        .is_some_and(|v| v.to_str().unwrap_or("").contains("fv_account_session")));
}

#[tokio::test]
#[serial]
async fn post_account_magic_without_cookies_requires_csrf() {
    use crate::auth::Auth;
    use crate::{build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState};
    use axum::http::{header, Request, StatusCode};
    use std::sync::Arc;
    use tower::Service;
    let data_dir = tempfile::tempdir().unwrap();
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let raw = db
        .issue_account_magic_signup("csrf-magic@test", None, None, now)
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
        account_password_throttle: crate::LoginThrottle::default(),
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
                .uri("/api/account/auth/magic")
                .header(header::ORIGIN, "https://evil.example")
                .header(header::HOST, "127.0.0.1:8080")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(axum::body::Body::from(format!("token={raw}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

async fn apply_migration_016_accounts_insert(pool: &sqlx::PgPool) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("migrations/000016_accounts_memberships.sql");
    let sql = std::fs::read_to_string(&path).expect("000016 migration file");
    let start = sql
        .find("INSERT INTO accounts")
        .expect("INSERT INTO accounts in 000016");
    let end = sql
        .find("INSERT INTO club_memberships")
        .expect("membership insert in 000016");
    for stmt in sql[start..end].split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        sqlx::query(stmt).execute(pool).await.unwrap();
    }
}

#[tokio::test]
#[serial]
async fn migration_016_pending_lookalike_password_not_on_account() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "lookalike-victim@test";
    let club_a = db.default_club_id().await.unwrap();
    let club_b = new_id();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at,
         checkin_window_hours, latitude, longitude)
         VALUES ($1, 'Other', 'other-c2', $2, 'live', $2, 12, -33.4489, -70.6693)",
    )
    .bind(&club_b)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let victim_hash = hash_password("victimpass12").unwrap();
    let attacker_hash = hash_password("attackerpass1").unwrap();
    let victim_pilot = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, 'V', $3, '', 'approved', 'pilot', $4, $5, $5)",
    )
    .bind(&victim_pilot)
    .bind(&club_a)
    .bind(email)
    .bind(&victim_hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let attacker_pilot = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'A', $3, '', 'pending', 'pilot', $4, $5)",
    )
    .bind(&attacker_pilot)
    .bind(&club_b)
    .bind(email)
    .bind(&attacker_hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    apply_migration_016_accounts_insert(&db.pool).await;
    let acct_hash: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM accounts WHERE lower(email) = lower($1)")
            .bind(email)
            .fetch_optional(&db.pool)
            .await
            .unwrap()
            .flatten();
    assert_eq!(acct_hash.as_deref(), Some(victim_hash.as_str()));
    assert!(matches!(
        db.account_login_password(email, "attackerpass1", now, "127.0.0.1")
            .await,
        Err(LoginError::Invalid)
    ));
    let admin_mid = new_id();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'admin', 'active', $4)",
    )
    .bind(&admin_mid)
    .bind(&victim_pilot)
    .bind(&club_a)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let bogus = db
        .account_login_password(email, "victimpass12", now, "127.0.0.1")
        .await
        .unwrap()
        .0;
    let _ = db
        .set_active_membership(&bogus, &admin_mid, now)
        .await
        .unwrap();
    assert!(db.pilot_session_for_account(&bogus, now).await.is_some());
    assert!(matches!(
        db.account_login_password(email, "attackerpass1", now, "127.0.0.1")
            .await,
        Err(LoginError::Invalid)
    ));
}

#[tokio::test]
#[serial]
async fn legacy_register_allowed_when_account_unverified() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "squat-unverified@test";
    let squat_id = db.ensure_account_for_email(email, None, now).await.unwrap();
    sqlx::query("UPDATE accounts SET password_hash = $1 WHERE id = $2")
        .bind(hash_password("squatterpass12").unwrap())
        .bind(&squat_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let club_id = db.default_club_id().await.unwrap();
    assert!(db
        .register_into_account(&club_id, "Victim", email, "L-1", "anotherpass12", now)
        .await
        .is_ok());
}

#[tokio::test]
#[serial]
async fn pilot_magic_login_rejects_suspended_account() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let email = "suspended-magic@test";
    let acct = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, created_at, email_verified_at, suspended_at)
         VALUES ($1, $2, $3, $3, $3)",
    )
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, $3, 'S', $4, '', 'approved', 'pilot', '', $5, $5)",
    )
    .bind(&pid)
    .bind(&club_id)
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let raw = db
        .issue_magic_login(&club_id, email, now)
        .await
        .unwrap()
        .unwrap();
    assert!(db.consume_magic_login(&raw, &club_id, now).await.is_err());
}

#[tokio::test]
#[serial]
async fn passkey_complete_login_rejects_suspended_account() {
    use crate::club::db::PilotRow;
    use crate::club::webauthn::WebauthnState;
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let email = "suspended-pk@test";
    let acct = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, created_at, email_verified_at, suspended_at)
         VALUES ($1, $2, $3, $3, $3)",
    )
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pilot = PilotRow {
        id: new_id(),
        club_id,
        name: "PK".to_string(),
        email: email.to_string(),
        license_no: String::new(),
        status: "approved".to_string(),
        role: "pilot".to_string(),
        locale: None,
    };
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, $3, 'PK', $4, '', 'approved', 'pilot', '', $5)",
    )
    .bind(&pilot.id)
    .bind(&pilot.club_id)
    .bind(&acct)
    .bind(&pilot.email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(WebauthnState::complete_passkey_login(&db, &pilot)
        .await
        .is_err());
}

#[tokio::test]
#[serial]
async fn webauthn_login_begin_returns_public_key_via_http() {
    use crate::auth::Auth;
    use crate::club::webauthn::WebauthnState;
    use crate::{build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState};
    use axum::http::{header, Request, StatusCode};
    use std::sync::Arc;
    use tower::Service;
    std::env::set_var("WEBAUTHN_RP_ID", "localhost");
    std::env::set_var("FLIGHT_CLUB_PUBLIC_URL", "http://localhost:8080");
    let webauthn = WebauthnState::from_env().expect("webauthn test rp");
    let data_dir = tempfile::tempdir().unwrap();
    let db = ClubDb::open_test().await.unwrap();
    let club_id = db.default_club_id().await.unwrap();
    let now = now_ms();
    let pilot_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'PK', 'pk-begin@test', '', 'approved', 'pilot', '', $3)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO webauthn_credentials (id, pilot_id, credential_id, public_key, counter, created_at)
         VALUES ($1, $2, '\\x01', '\\x01', 0, $3)",
    )
    .bind(new_id())
    .bind(&pilot_id)
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
        account_password_throttle: crate::LoginThrottle::default(),
        mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
        webauthn: Some(webauthn),
        sse_limiter: std::sync::Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails: HashSet::new(),
    };
    let mut router = build_router(state);
    let res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/club/auth/webauthn/login/begin")
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(r#"{"email":"pk-begin@test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
#[serial]
async fn compat_pilot_session_stored_hashed_not_raw() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let acct = new_id();
    let email = "compat-hash@test";
    sqlx::query(
        "INSERT INTO accounts (id, email, created_at, email_verified_at)
         VALUES ($1, $2, $3, $3)",
    )
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pilot = new_id();
    let mid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, $3, 'C', $4, '', 'approved', 'admin', '', $5)",
    )
    .bind(&pilot)
    .bind(&club_id)
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'admin', 'active', $4)",
    )
    .bind(&mid)
    .bind(&pilot)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let sess = db
        .create_account_session(&acct, Some(&mid), now)
        .await
        .unwrap();
    let _ = db.pilot_session_for_account(&sess, now).await.unwrap();
    let stored: String =
        sqlx::query_scalar("SELECT compat_pilot_session FROM account_sessions WHERE id = $1")
            .bind(crate::club::auth::digest_token(&sess))
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_ne!(stored, sess);
    assert_eq!(stored.len(), 64);
}

#[tokio::test]
#[serial]
async fn parallel_pilot_for_active_membership_no_session_churn() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let acct = new_id();
    let email = "parallel-viewer@test";
    sqlx::query(
        "INSERT INTO accounts (id, email, created_at, email_verified_at)
         VALUES ($1, $2, $3, $3)",
    )
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let pilot_id = new_id();
    let mid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, $3, 'P', $4, '', 'approved', 'pilot', '', $5)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(&acct)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'pilot', 'active', $4)",
    )
    .bind(&mid)
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let before: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM pilot_sessions WHERE pilot_id = $1")
            .bind(&pilot_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let mut handles = Vec::new();
    for _ in 0..12 {
        let db = db.clone();
        let mid = mid.clone();
        handles.push(tokio::spawn(async move {
            db.pilot_for_active_membership(&mid).await.is_some()
        }));
    }
    for h in handles {
        assert!(h.await.unwrap());
    }
    let after: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM pilot_sessions WHERE pilot_id = $1")
            .bind(&pilot_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
#[serial]
async fn admin_invite_clears_legacy_pilot_password() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    let email = "invite-strips-pw@test";
    let inviter = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Adm', 'admin-invite@test', '', 'approved', 'admin', 'x', $3)",
    )
    .bind(&inviter)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.ensure_membership_row(&inviter, &club_id, "admin", "active")
        .await
        .unwrap();
    let _ = db
        .register_into_account(&club_id, "Atk", email, "", "attackerpass1", now)
        .await
        .unwrap();
    let invite = db
        .invite_admin_membership(&club_id, email, &inviter, now)
        .await
        .unwrap();
    let _ = db.accept_admin_invite(&invite, now).await.unwrap();
    assert!(matches!(
        db.login(&club_id, email, "attackerpass1", now).await,
        Err(LoginError::Invalid)
    ));
}

#[tokio::test]
#[serial]
async fn legacy_register_does_not_copy_password_to_verified_account() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "legacy-no-acct-pw@test";
    let victim_hash = hash_password("victimpass12").unwrap();
    let acct = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, password_hash, created_at, email_verified_at)
         VALUES ($1, $2, $3, $4, $4)",
    )
    .bind(&acct)
    .bind(email)
    .bind(&victim_hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let club_id = db.default_club_id().await.unwrap();
    let _ = db
        .register_into_account(&club_id, "New", email, "LIC", "attackerpass1", now)
        .await
        .unwrap();
    let hash: Option<String> =
        sqlx::query_scalar("SELECT password_hash FROM accounts WHERE id = $1")
            .bind(&acct)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(hash.as_deref(), Some(victim_hash.as_str()));
    assert!(matches!(
        db.account_login_password(email, "attackerpass1", now, "127.0.0.1")
            .await,
        Err(LoginError::Invalid)
    ));
}

#[tokio::test]
#[serial]
async fn approve_membership_rejects_unverified_account_leaves_membership_pending() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_id = db.default_club_id().await.unwrap();
    sqlx::query("UPDATE clubs SET status = 'live', email_verified_at = $1 WHERE id = $2")
        .bind(now)
        .bind(&club_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let acct = db
        .create_account("unverified-approve@test", None, now)
        .await
        .unwrap();
    let pilot_id = new_id();
    let mid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, $3, 'U', 'unverified-approve@test', '', 'pending', 'pilot', '', $4)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(&acct)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, 'pilot', 'pending', $4)",
    )
    .bind(&mid)
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    assert!(db.approve_membership(&mid, now).await.is_err());
    let status: String = sqlx::query_scalar("SELECT status FROM club_memberships WHERE id = $1")
        .bind(&mid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status, "pending");
    let pilot_status: String = sqlx::query_scalar(
        "SELECT status FROM pilots WHERE id = (SELECT pilot_id FROM club_memberships WHERE id = $1)",
    )
    .bind(&mid)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(pilot_status, "pending");
}

#[tokio::test]
#[serial]
async fn legacy_register_verified_account_pilot_has_no_password() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "legacy-verified-pilot-pw@test";
    let victim_hash = hash_password("victimpass12").unwrap();
    let acct = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, password_hash, created_at, email_verified_at)
         VALUES ($1, $2, $3, $4, $4)",
    )
    .bind(&acct)
    .bind(email)
    .bind(&victim_hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let club_id = db.default_club_id().await.unwrap();
    let (pilot_id, _) = db
        .register_into_account(&club_id, "Atk", email, "L", "attackerpass1", now)
        .await
        .unwrap();
    let ph: String = sqlx::query_scalar("SELECT password_hash FROM pilots WHERE id = $1")
        .bind(&pilot_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert!(ph.is_empty());
    assert!(matches!(
        db.login(&club_id, email, "attackerpass1", now).await,
        Err(LoginError::Invalid)
    ));
}

#[tokio::test]
#[serial]
async fn set_account_password_wrong_current_rejected() {
    use crate::club::db_account::AccountSignupError;
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "wrong-current-pw@test";
    let id = db.create_account(email, None, now).await.unwrap();
    sqlx::query("UPDATE accounts SET email_verified_at = $1 WHERE id = $2")
        .bind(now)
        .bind(&id)
        .execute(&db.pool)
        .await
        .unwrap();
    db.set_account_password(&id, "password12", now, None, None)
        .await
        .unwrap();
    let err = db
        .set_account_password(&id, "newpassword1", now, Some("wrongpass1"), None)
        .await
        .unwrap_err();
    assert_eq!(err, AccountSignupError::WrongPassword);
}

#[tokio::test]
#[serial]
async fn set_account_password_revokes_other_account_sessions() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "revoke-sessions-pw@test";
    let id = db.create_account(email, None, now).await.unwrap();
    sqlx::query("UPDATE accounts SET email_verified_at = $1 WHERE id = $2")
        .bind(now)
        .bind(&id)
        .execute(&db.pool)
        .await
        .unwrap();
    db.set_account_password(&id, "password12", now, None, None)
        .await
        .unwrap();
    let (keep, _) = db
        .account_login_password(email, "password12", now, "127.0.0.1")
        .await
        .unwrap();
    let other = db.create_account_session(&id, None, now).await.unwrap();
    let before: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM account_sessions WHERE account_id = $1")
            .bind(&id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(before, 2);
    db.set_account_password(&id, "newpassword1", now, Some("password12"), Some(&keep))
        .await
        .unwrap();
    let after: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM account_sessions WHERE account_id = $1")
            .bind(&id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(after, 1);
    assert!(db.account_from_session(&keep, now).await.is_some());
    assert!(db.account_from_session(&other, now).await.is_none());
}

#[tokio::test]
#[serial]
async fn set_account_password_rejects_over_max_length() {
    use crate::club::db_account::AccountSignupError;
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let id = db.create_account("long-pw@test", None, now).await.unwrap();
    sqlx::query("UPDATE accounts SET email_verified_at = $1 WHERE id = $2")
        .bind(now)
        .bind(&id)
        .execute(&db.pool)
        .await
        .unwrap();
    let long = "a".repeat(129);
    let err = db
        .set_account_password(&id, &long, now, None, None)
        .await
        .unwrap_err();
    assert_eq!(err, AccountSignupError::TooLong);
}

#[tokio::test]
#[serial]
async fn account_signup_nonce_cookie_http_set_and_cleared() {
    use crate::auth::Auth;
    use crate::club::auth;
    use crate::{build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState};
    use axum::http::{header, Request, StatusCode};
    use std::sync::Arc;
    use tower::Service;
    let data_dir = tempfile::tempdir().unwrap();
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let email = "signup-nonce-cookie@test";
    let raw = db
        .issue_account_magic_signup(email, Some("password12"), Some("nonce-abc"), now)
        .await
        .unwrap();
    let make_router = |db: ClubDb| {
        let (video_queue, _) = tokio::sync::mpsc::channel(4);
        AppState {
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
            superadmin_emails: HashSet::new(),
        }
    };
    let mut router = build_router(make_router(db.clone()));
    let post_res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/auth/magic")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::HOST, "127.0.0.1:8080")
                .header(
                    header::COOKIE,
                    format!("{}=nonce-abc", auth::SIGNUP_NONCE_COOKIE),
                )
                .body(axum::body::Body::from(format!("token={raw}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(post_res.status(), StatusCode::FOUND);
    let cookies: Vec<String> = post_res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(String::from)
        .collect();
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with(auth::SIGNUP_NONCE_COOKIE) && c.contains("Max-Age=0")),
        "expected cleared signup nonce cookie, got {cookies:?}"
    );
    let mut router2 = build_router(make_router(db));
    let signup_res = router2
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/signup")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::HOST, "127.0.0.1:8080")
                .header("Sec-Fetch-Site", "same-origin")
                .body(axum::body::Body::from(
                    r#"{"email":"other-nonce@test","password":"password12"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(signup_res.status(), StatusCode::CREATED);
    let set = signup_res
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(set.starts_with(auth::SIGNUP_NONCE_COOKIE));
    assert!(set.contains("Secure"));
    assert!(set.contains("Path=/"));
}
