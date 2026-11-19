//! HTTP (L-13) tests for multi-admin membership APIs.

use crate::club::db::now_ms;
use crate::club::db_memberships_admin::ADMIN_INVITE_PURPOSE;
use crate::club::{auth, auth::hash_password, ClubDb};
use serial_test::serial;

async fn http_stack(db: ClubDb) -> (axum::Router, crate::AppState, ClubDb) {
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use std::{collections::HashSet, sync::Arc};
    let data_dir = tempfile::tempdir().unwrap();
    let dist_dir = data_dir.path().join("dist");
    let locales_dir = dist_dir.join("locales");
    std::fs::create_dir_all(&locales_dir).unwrap();
    let repo_locales = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../locales");
    for file in ["manifest.json", "es.json", "en.json"] {
        let src = repo_locales.join(file);
        if src.is_file() {
            std::fs::copy(&src, locales_dir.join(file)).unwrap();
        }
    }
    let locale_dir = crate::i18n::Catalogs::resolve_dir(&dist_dir);
    let catalogs = crate::i18n::init_global(&locale_dir).expect("locales");
    let probe = catalogs.mail(
        "es",
        "adminInvite",
        &[("minutes", "15"), ("url", "http://x")],
    );
    assert!(probe.1.contains("http://x"), "adminInvite mail template");
    let outbox = crate::club::mail::MailOutbox::default();
    let mailer = crate::club::mail::Mailer::from_env(outbox);
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir: dist_dir.clone(),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live: Arc::new(LiveEngine::new(data_dir.path())),
        store: Store::new(data_dir.path()),
        club: Some(db.clone()),
        video_queue,
        login_throttle: crate::LoginThrottle::default(),
        device_register_throttle: crate::LoginThrottle::default(),
        share_lookup_throttle: crate::LoginThrottle::default(),
        mailer,
        webauthn: None,
        sse_limiter: std::sync::Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails: HashSet::new(),
    };
    let router = build_router(state.clone());
    (router, state, db)
}

async fn fresh_live_club(db: &ClubDb) -> String {
    let now = now_ms();
    let slug = format!("http-club-{}", &auth::new_id()[..8]);
    let (club_id, token) = db
        .signup_club("HTTP Club", &slug, &format!("owner-{slug}@test"), None, now)
        .await
        .unwrap();
    db.try_consume_magic_club_verify(&token, now).await.unwrap();
    club_id
}

async fn signup_admin_session(db: &ClubDb, club_id: &str) -> (String, String, String) {
    let pilot_id: String = sqlx::query_scalar(
        "SELECT id FROM pilots WHERE club_id = $1 AND role = 'admin' ORDER BY created_at LIMIT 1",
    )
    .bind(club_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let mid: String =
        sqlx::query_scalar("SELECT id FROM club_memberships WHERE pilot_id = $1 AND club_id = $2")
            .bind(&pilot_id)
            .bind(club_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let session = db.create_pilot_session(&pilot_id, now_ms()).await.unwrap();
    let cookie = format!("fv_club_session={session}");
    (pilot_id, mid, cookie)
}

async fn seed_admin(db: &ClubDb, club_id: &str, email: &str) -> (String, String, String) {
    let now = now_ms();
    let hash = hash_password("password12").unwrap();
    let pilot_id = auth::new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Admin', $3, '', 'approved', 'admin', $4, $5)",
    )
    .bind(&pilot_id)
    .bind(club_id)
    .bind(email)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.ensure_membership_row(&pilot_id, club_id, "admin", "active")
        .await
        .unwrap();
    let mid: String =
        sqlx::query_scalar("SELECT id FROM club_memberships WHERE pilot_id = $1 AND club_id = $2")
            .bind(&pilot_id)
            .bind(club_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let session = db.create_pilot_session(&pilot_id, now).await.unwrap();
    let cookie = format!("fv_club_session={session}");
    (pilot_id, mid, cookie)
}

fn invite_token_from_mailer(state: &crate::AppState, invitee: &str) -> String {
    let mails = state.mailer.captured();
    let body = mails
        .iter()
        .rev()
        .find(|m| m.to.eq_ignore_ascii_case(invitee))
        .map(|m| m.body.as_str())
        .expect("invite mail captured");
    let marker = "token=";
    let start = body
        .find(marker)
        .unwrap_or_else(|| panic!("token in mail: {body}"));
    let rest = &body[start + marker.len()..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '"' || c == '&' || c == '<' || c == '\n')
        .unwrap_or(rest.len());
    rest[..end].to_string()
}

#[tokio::test]
#[serial]
async fn http_admin_invite_becomes_admin_only_after_magic_consume() {
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use tower::Service;

    let db = ClubDb::open_test().await.unwrap();
    let club = fresh_live_club(&db).await;
    let (_admin_pid, _admin_mid, cookie) = signup_admin_session(&db, &club).await;
    let (mut router, state, db) = http_stack(db).await;
    let invitee = "http-invitee@test";

    let before_admins: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM club_memberships m
         JOIN pilots p ON p.id = m.pilot_id
         WHERE m.club_id = $1 AND lower(p.email) = lower($2) AND m.role = 'admin' AND m.status = 'active'",
    )
    .bind(&club)
    .bind(invitee)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(before_admins, 0);

    let invite_res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/memberships/invite")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(r#"{{"email":"{invitee}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invite_res.status(), StatusCode::OK);

    let pending_magic: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM magic_login_tokens WHERE lower(email) = lower($1) AND purpose = $2 AND used_at IS NULL",
    )
    .bind(invitee)
    .bind(ADMIN_INVITE_PURPOSE)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(pending_magic, 1);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM club_memberships m
             JOIN pilots p ON p.id = m.pilot_id
             WHERE m.club_id = $1 AND lower(p.email) = lower($2) AND m.role = 'admin'",
        )
        .bind(&club)
        .bind(invitee)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        0
    );

    let token = invite_token_from_mailer(&state, invitee);
    let accept_res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/auth/magic")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("token={token}&purpose=admin_invite")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accept_res.status(), StatusCode::FOUND);

    let role: String = sqlx::query_scalar(
        "SELECT m.role FROM club_memberships m
         JOIN pilots p ON p.id = m.pilot_id
         WHERE m.club_id = $1 AND lower(p.email) = lower($2)",
    )
    .bind(&club)
    .bind(invitee)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(role, "admin");
}

#[tokio::test]
#[serial]
async fn http_admin_promote_demote_remove() {
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use tower::Service;

    let db = ClubDb::open_test().await.unwrap();
    let club = fresh_live_club(&db).await;
    let (_, _, cookie) = signup_admin_session(&db, &club).await;
    let staff_mid: String = {
        let now = now_ms();
        let hash = hash_password("password12").unwrap();
        let pilot_id = auth::new_id();
        sqlx::query(
            "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
             VALUES ($1, $2, 'Staff', 'http-staff@test', '', 'approved', 'staff', $3, $4)",
        )
        .bind(&pilot_id)
        .bind(&club)
        .bind(&hash)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
        db.ensure_membership_row(&pilot_id, &club, "staff", "active")
            .await
            .unwrap();
        sqlx::query_scalar("SELECT id FROM club_memberships WHERE pilot_id = $1 AND club_id = $2")
            .bind(&pilot_id)
            .bind(&club)
            .fetch_one(&db.pool)
            .await
            .unwrap()
    };
    let (mut router, _state, db) = http_stack(db).await;

    let promote = router
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/api/admin/memberships/{staff_mid}/promote"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(promote.status(), StatusCode::OK);
    let role: String = sqlx::query_scalar("SELECT role FROM club_memberships WHERE id = $1")
        .bind(&staff_mid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(role, "admin");

    let demote = router
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/api/admin/memberships/{staff_mid}/demote"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(demote.status(), StatusCode::OK);

    let remove = router
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/api/admin/memberships/{staff_mid}/remove"))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(remove.status(), StatusCode::OK);
    let status: String = sqlx::query_scalar("SELECT status FROM club_memberships WHERE id = $1")
        .bind(&staff_mid)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(status, "suspended");
}

#[tokio::test]
#[serial]
async fn http_last_admin_demote_and_remove_blocked() {
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use tower::Service;

    let db = ClubDb::open_test().await.unwrap();
    let club = fresh_live_club(&db).await;
    let (_, only_mid, cookie) = signup_admin_session(&db, &club).await;
    let (mut router, _state, _db) = http_stack(db).await;

    for uri in [
        format!("/api/admin/memberships/{only_mid}/demote"),
        format!("/api/admin/memberships/{only_mid}/remove"),
    ] {
        let res = router
            .call(
                Request::builder()
                    .method("POST")
                    .uri(&uri)
                    .header(header::COOKIE, &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{uri}");
    }
}

#[tokio::test]
#[serial]
async fn http_concurrent_demote_two_admins_one_last_admin() {
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use tower::Service;

    let db = ClubDb::open_test().await.unwrap();
    let club = fresh_live_club(&db).await;
    let (_, signup_mid, cookie) = signup_admin_session(&db, &club).await;
    let (_, other_mid, _) = seed_admin(&db, &club, "http-race-b@test").await;
    let mid_a = signup_mid;
    let mid_b = other_mid;
    let (router, _state, db) = http_stack(db).await;
    let mut r_a = router.clone();
    let mut r_b = router.clone();
    let c1 = cookie.clone();
    let c2 = cookie.clone();
    let (res_a, res_b) = tokio::join!(
        async move {
            r_a.call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/admin/memberships/{mid_a}/demote"))
                    .header(header::COOKIE, &c1)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
        },
        async move {
            r_b.call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/admin/memberships/{mid_b}/demote"))
                    .header(header::COOKIE, &c2)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
        },
    );
    let ok = [res_a.status(), res_b.status()]
        .iter()
        .filter(|s| **s == StatusCode::OK)
        .count();
    assert_eq!(
        ok,
        1,
        "demote statuses: {:?} {:?}",
        res_a.status(),
        res_b.status()
    );
    assert!(
        res_a.status() == StatusCode::BAD_REQUEST
            || res_b.status() == StatusCode::BAD_REQUEST
            || res_a.status() == StatusCode::FORBIDDEN
            || res_b.status() == StatusCode::FORBIDDEN,
        "second request blocked"
    );
    let admins: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM club_memberships
         WHERE club_id = $1 AND role = 'admin' AND status = 'active'",
    )
    .bind(&club)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(admins, 1);
}

#[tokio::test]
#[serial]
async fn http_non_admin_membership_ops_forbidden() {
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use tower::Service;

    let db = ClubDb::open_test().await.unwrap();
    let club = fresh_live_club(&db).await;
    let now = now_ms();
    let hash = hash_password("password12").unwrap();
    let pilot_id = auth::new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Pilot', 'http-pilot-only@test', '', 'approved', 'pilot', $3, $4)",
    )
    .bind(&pilot_id)
    .bind(&club)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.ensure_membership_row(&pilot_id, &club, "pilot", "active")
        .await
        .unwrap();
    let session = db.create_pilot_session(&pilot_id, now).await.unwrap();
    let cookie = format!("fv_club_session={session}");
    let (mut router, _state, _db) = http_stack(db).await;

    for (method, uri, body) in [
        (
            "POST",
            "/api/admin/memberships/invite",
            r#"{"email":"x@test"}"#,
        ),
        ("GET", "/api/admin/memberships", ""),
        ("POST", "/api/admin/memberships/nope/promote", ""),
    ] {
        let mut req = Request::builder().method(method).uri(uri);
        if method == "POST" && !body.is_empty() {
            req = req.header(header::CONTENT_TYPE, "application/json");
        }
        let res = router
            .call(
                req.header(header::COOKIE, &cookie)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{method} {uri}");
    }
}

#[tokio::test]
#[serial]
async fn http_club_creator_account_login_after_verify() {
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use tower::Service;

    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let slug = format!("http-creator-{}", &auth::new_id()[..8]);
    let email = format!("owner-{slug}@test");
    let (club_id, token) = db
        .signup_club("HTTP Creator", &slug, &email, None, now)
        .await
        .unwrap();
    db.try_consume_magic_club_verify(&token, now).await.unwrap();
    let hash = hash_password("password12").unwrap();
    sqlx::query(
        "UPDATE accounts SET password_hash = $1 WHERE lower(trim(email)) = lower(trim($2))",
    )
    .bind(&hash)
    .bind(&email)
    .execute(&db.pool)
    .await
    .unwrap();
    let (mut router, _state, db) = http_stack(db).await;
    let login = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"password12"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let bytes = to_bytes(login.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v.get("ok"), Some(&serde_json::json!(true)));
    let memberships = db
        .list_account_memberships(&db.account_by_email(&email).await.expect("account").id)
        .await;
    assert!(memberships
        .iter()
        .any(|m| m.club_id == club_id && m.role == "admin" && m.status == "active"));
}
