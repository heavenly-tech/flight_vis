//! HTTP (L-13) tests for multi-admin membership APIs.

use crate::club::db::now_ms;
use crate::club::db_memberships_admin::{ADMIN_INVITE_PURPOSE, PILOT_INVITE_PURPOSE};
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
    let pilot_probe = catalogs.mail(
        "es",
        "pilotInvite",
        &[("minutes", "15"), ("url", "http://p")],
    );
    assert!(
        pilot_probe.1.contains("http://p"),
        "pilotInvite mail template"
    );
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
        account_password_throttle: crate::LoginThrottle::default(),
        account_password_account_throttle: crate::LoginThrottle::default(),
        public_live_trails_throttle: crate::LoginThrottle::default(),
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
        (
            "POST",
            "/api/admin/memberships/invite",
            r#"{"email":"p@test","role":"pilot"}"#,
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

#[tokio::test]
#[serial]
async fn http_pilot_invite_captures_mail_consumes_as_pilot_and_audits() {
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use tower::Service;

    let db = ClubDb::open_test().await.unwrap();
    let club_a = fresh_live_club(&db).await;
    let club_b = fresh_live_club(&db).await;
    let (admin_a_pid, _, cookie_a) = signup_admin_session(&db, &club_a).await;
    let (mut router, state, db) = http_stack(db).await;
    let invitee = "pilot-invitee@test";

    let invite_res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/memberships/invite")
                .header(header::COOKIE, &cookie_a)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{invitee}","role":"pilot"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(invite_res.status(), StatusCode::OK);
    let body = to_bytes(invite_res.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v, serde_json::json!({ "ok": true }));

    let (token_id, token_club): (String, String) = sqlx::query_as(
        "SELECT id, club_id FROM magic_login_tokens
         WHERE lower(email) = lower($1) AND purpose = $2 AND used_at IS NULL",
    )
    .bind(invitee)
    .bind(PILOT_INVITE_PURPOSE)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(token_club, club_a);

    let invite_audit: (String, String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT entity_type, entity_id, detail, actor_pilot_id
         FROM audit_log
         WHERE club_id = $1 AND action = 'invite_pilot'
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&club_a)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(invite_audit.0, "membership");
    assert_eq!(invite_audit.1, token_id);
    assert_ne!(invite_audit.1, invitee);
    assert_eq!(invite_audit.2.as_deref(), Some("pilot"));
    assert_eq!(invite_audit.3.as_deref(), Some(admin_a_pid.as_str()));

    let mails = state.mailer.captured();
    let captured = mails
        .iter()
        .rev()
        .find(|m| m.to.eq_ignore_ascii_case(invitee))
        .expect("pilot invite mail captured");
    assert!(captured.body.contains("purpose=pilot_invite"));
    let token = invite_token_from_mailer(&state, invitee);

    // GET confirmation page does not consume the token and resolves pilot_invite purpose.
    let get_res = router
        .call(
            Request::builder()
                .method("GET")
                .uri(format!("/api/account/auth/magic?token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let get_html = String::from_utf8(
        to_bytes(get_res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(get_html.contains(r#"value="pilot_invite""#));
    let still_unused: bool =
        sqlx::query_scalar("SELECT used_at IS NULL FROM magic_login_tokens WHERE id = $1")
            .bind(&token_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(still_unused);

    // Consume token -> redirects to /account-clubs.html and sets fv_account_session.
    let accept_res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/auth/magic")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("token={token}&purpose=pilot_invite")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accept_res.status(), StatusCode::FOUND);
    assert_eq!(
        accept_res
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/account-clubs.html")
    );
    let set_cookie = accept_res
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(set_cookie.contains("fv_account_session="));

    let (mid, pid, m_role, m_status, p_role, p_status): (
        String,
        String,
        String,
        String,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT m.id, p.id, m.role, m.status, p.role, p.status
         FROM club_memberships m
         JOIN pilots p ON p.id = m.pilot_id
         WHERE m.club_id = $1 AND lower(p.email) = lower($2)",
    )
    .bind(&club_a)
    .bind(invitee)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(m_role, "pilot");
    assert_eq!(m_status, "active");
    assert_eq!(p_role, "pilot");
    assert_eq!(p_status, "approved");

    // Must not grant membership in club B.
    let club_b_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM club_memberships m
         JOIN pilots p ON p.id = m.pilot_id
         WHERE m.club_id = $1 AND lower(p.email) = lower($2)",
    )
    .bind(&club_b)
    .bind(invitee)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(club_b_count, 0);

    // Accept audit row exists in club A with entity_id = membership_id, actor_pilot_id = pid.
    let accept_audit: (String, String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT entity_type, entity_id, detail, actor_pilot_id
         FROM audit_log
         WHERE club_id = $1 AND action = 'accept_invite' AND entity_id = $2",
    )
    .bind(&club_a)
    .bind(&mid)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(accept_audit.0, "membership");
    assert_eq!(accept_audit.1, mid);
    assert_eq!(accept_audit.2.as_deref(), Some("pilot"));
    assert_eq!(accept_audit.3.as_deref(), Some(pid.as_str()));

    // Single-use token: replay rejected.
    let replay_res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/auth/magic")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!("token={token}&purpose=pilot_invite")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay_res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[serial]
async fn http_pilot_invite_token_cannot_be_consumed_as_admin() {
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use tower::Service;

    let db = ClubDb::open_test().await.unwrap();
    let club = fresh_live_club(&db).await;
    let (_, _, cookie) = signup_admin_session(&db, &club).await;
    let (mut router, state, db) = http_stack(db).await;
    let invitee = "pilot-no-escalate@test";

    let res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/memberships/invite")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{invitee}","role":"pilot"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let token = invite_token_from_mailer(&state, invitee);

    // Attempt to tamper purpose=admin_invite when consuming a pilot_invite token.
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
    assert_eq!(
        accept_res
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/account-clubs.html")
    );

    let (m_role, p_role): (String, String) = sqlx::query_as(
        "SELECT m.role, p.role FROM club_memberships m
         JOIN pilots p ON p.id = m.pilot_id
         WHERE m.club_id = $1 AND lower(p.email) = lower($2)",
    )
    .bind(&club)
    .bind(invitee)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(m_role, "pilot");
    assert_eq!(p_role, "pilot");
}

#[tokio::test]
#[serial]
async fn http_pilot_invite_unknown_role_is_noop_and_csrf_rejected() {
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use tower::Service;

    let db = ClubDb::open_test().await.unwrap();
    let club = fresh_live_club(&db).await;
    let (_, _, cookie) = signup_admin_session(&db, &club).await;
    let (mut router, state, db) = http_stack(db).await;

    for (email, role) in [("unknown-role@test", "superadmin"), ("empty-role@test", "")] {
        let res = router
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/memberships/invite")
                    .header(header::COOKIE, &cookie)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(format!(
                        r#"{{"email":"{email}","role":"{role}"}}"#
                    )))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!({ "ok": true })
        );
        let tok_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::bigint FROM magic_login_tokens WHERE lower(email) = lower($1)",
        )
        .bind(email)
        .fetch_one(&db.pool)
        .await
        .unwrap();
        assert_eq!(tok_count, 0);
    }
    assert!(state.mailer.captured().is_empty());

    // CSRF rejection via Sec-Fetch-Site: cross-site and mismatched Origin/Host.
    let csrf_sfs = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/memberships/invite")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .header("sec-fetch-site", "cross-site")
                .body(Body::from(r#"{"email":"csrf-sfs@test","role":"pilot"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(csrf_sfs.status(), StatusCode::FORBIDDEN);

    let csrf_origin = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/memberships/invite")
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::HOST, "flight.heavenly.cl")
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::from(r#"{"email":"csrf-orig@test","role":"pilot"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(csrf_origin.status(), StatusCode::FORBIDDEN);
    assert!(state.mailer.captured().is_empty());
}

#[tokio::test]
#[serial]
async fn http_pilot_invite_existing_admin_stays_admin_and_cross_club_isolated() {
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use tower::Service;

    let db = ClubDb::open_test().await.unwrap();
    let club_a = fresh_live_club(&db).await;
    let club_b = fresh_live_club(&db).await;
    let (_, _, cookie_a) = signup_admin_session(&db, &club_a).await;
    let (_, mid_b, _) = signup_admin_session(&db, &club_b).await;

    // Create a second approved admin in club A (with linked account via admin invite).
    let (mut router, state, db) = http_stack(db).await;
    let existing_admin_email = "keep-admin@test";
    let res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/memberships/invite")
                .header(header::COOKIE, &cookie_a)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{existing_admin_email}","role":"admin"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let admin_tok = invite_token_from_mailer(&state, existing_admin_email);
    let accept_admin = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/auth/magic")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "token={admin_tok}&purpose=admin_invite"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accept_admin.status(), StatusCode::FOUND);
    assert_eq!(
        accept_admin
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("/club-admin-pilots.html")
    );

    // Now invite that existing admin as a pilot -> must NOT demote them.
    let res2 = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/memberships/invite")
                .header(header::COOKIE, &cookie_a)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{existing_admin_email}","role":"pilot"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res2.status(), StatusCode::OK);
    let pilot_tok = invite_token_from_mailer(&state, existing_admin_email);
    let accept_pilot = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/auth/magic")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(format!(
                    "token={pilot_tok}&purpose=pilot_invite"
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(accept_pilot.status(), StatusCode::FOUND);

    let (m_role, m_status, p_role): (String, String, String) = sqlx::query_as(
        "SELECT m.role, m.status, p.role FROM club_memberships m
         JOIN pilots p ON p.id = m.pilot_id
         WHERE m.club_id = $1 AND lower(p.email) = lower($2)",
    )
    .bind(&club_a)
    .bind(existing_admin_email)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(m_role, "admin");
    assert_eq!(m_status, "active");
    assert_eq!(p_role, "admin");

    // Admin A cannot promote/demote/remove Club B's membership.
    for action in ["promote", "demote", "remove"] {
        let cross = router
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/admin/memberships/{mid_b}/{action}"))
                    .header(header::COOKIE, &cookie_a)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cross.status(), StatusCode::NOT_FOUND, "cross-club {action}");
    }

    // Add a pending pilot in Club A and verify GET /api/admin/pilots returns `status` and stays club-scoped.
    let now = now_ms();
    let pending_id = auth::new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Pend A', 'pending-a@test', '', 'pending', 'pilot', '', $3)",
    )
    .bind(&pending_id)
    .bind(&club_a)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

    let pilots_res = router
        .call(
            Request::builder()
                .method("GET")
                .uri("/api/admin/pilots")
                .header(header::COOKIE, &cookie_a)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pilots_res.status(), StatusCode::OK);
    let pilots_bytes = to_bytes(pilots_res.into_body(), usize::MAX).await.unwrap();
    let pilots_json: serde_json::Value = serde_json::from_slice(&pilots_bytes).unwrap();
    let pilots = pilots_json["pilots"].as_array().expect("pilots array");
    assert!(pilots.iter().all(|p| p["club_id"] == club_a));
    assert!(pilots
        .iter()
        .any(|p| p["email"] == existing_admin_email && p["status"] == "approved"));
    assert!(pilots
        .iter()
        .any(|p| p["email"] == "pending-a@test" && p["status"] == "pending"));
}
