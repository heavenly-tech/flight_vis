//! Integration tests for `DELETE /api/admin/club` (hard delete, auth, CSRF, slug confirm, active-club switch).

use crate::club::auth::{self, hash_password, new_id};
use crate::club::db::now_ms;
use crate::club::ClubDb;
use axum::body::{to_bytes, Body};
use axum::http::{header, HeaderMap, Request, StatusCode};
use serial_test::serial;
use tower::Service;

async fn http_stack(db: ClubDb) -> (axum::Router, ClubDb) {
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use std::{collections::HashSet, sync::Arc};
    let data_dir = tempfile::tempdir().unwrap();
    let dist_dir = data_dir.path().join("dist");
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir,
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
        mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
        webauthn: None,
        sse_limiter: Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails: HashSet::new(),
    };
    (build_router(state), db)
}

async fn create_live_club(db: &ClubDb, prefix: &str, owner_email: &str) -> (String, String) {
    let now = now_ms();
    let slug = format!("{prefix}-{}", &new_id()[..8]);
    let (club_id, token) = db
        .signup_club(
            &format!("Club {prefix}"),
            &slug,
            owner_email,
            Some("SCTB"),
            now,
        )
        .await
        .expect("signup_club");
    db.try_consume_magic_club_verify(&token, now)
        .await
        .expect("verify club");
    (club_id, slug)
}

async fn club_admin_ids(db: &ClubDb, club_id: &str) -> (String, String, String) {
    let pilot_id: String = sqlx::query_scalar(
        "SELECT id FROM pilots WHERE club_id = $1 AND role = 'admin' ORDER BY created_at LIMIT 1",
    )
    .bind(club_id)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    let account_id: String = sqlx::query_scalar("SELECT account_id FROM pilots WHERE id = $1")
        .bind(&pilot_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    let membership_id: String =
        sqlx::query_scalar("SELECT id FROM club_memberships WHERE pilot_id = $1 AND club_id = $2")
            .bind(&pilot_id)
            .bind(club_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    (pilot_id, account_id, membership_id)
}

async fn seed_member_with_session(
    db: &ClubDb,
    club_id: &str,
    email: &str,
    role: &str,
) -> (String, String, String) {
    let now = now_ms();
    let hash = hash_password("password12").unwrap();
    let pilot_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, $3, $4, '', 'approved', $5, $6, $7, $7)",
    )
    .bind(&pilot_id)
    .bind(club_id)
    .bind(role)
    .bind(email)
    .bind(role)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.ensure_membership_row(&pilot_id, club_id, role, "active")
        .await
        .unwrap();
    let mid: String =
        sqlx::query_scalar("SELECT id FROM club_memberships WHERE pilot_id = $1 AND club_id = $2")
            .bind(&pilot_id)
            .bind(club_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let sess = db.create_pilot_session(&pilot_id, now).await.unwrap();
    (pilot_id, mid, format!("fv_club_session={sess}"))
}

struct SeededClubData {
    type_id: String,
    aircraft_id: String,
    device_id: String,
    flight_id: String,
    checkout_id: String,
    display_id: String,
    checkin_id: String,
    handoff_id: String,
    access_req_id: String,
    share_id: String,
    guest_pilot_id: String,
    guest_session_id: String,
}

async fn seed_rich_club_rows(
    db: &ClubDb,
    club_id: &str,
    admin_pilot_id: &str,
    tag: &str,
) -> SeededClubData {
    let now = now_ms();
    let type_id = new_id();
    sqlx::query("INSERT INTO aircraft_types (id, club_id, code) VALUES ($1, $2, $3)")
        .bind(&type_id)
        .bind(club_id)
        .bind(format!("C152-{tag}"))
        .execute(&db.pool)
        .await
        .unwrap();

    let aircraft_id = new_id();
    sqlx::query(
        "INSERT INTO aircraft (id, club_id, tail, type_id, active) VALUES ($1, $2, $3, $4, TRUE)",
    )
    .bind(&aircraft_id)
    .bind(club_id)
    .bind(format!("CC-{tag}"))
    .bind(&type_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let checkout_id = new_id();
    sqlx::query(
        "INSERT INTO checkouts (id, pilot_id, type_id, aircraft_id, club_id)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&checkout_id)
    .bind(admin_pilot_id)
    .bind(&type_id)
    .bind(&aircraft_id)
    .bind(club_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let device_id = new_id();
    sqlx::query(
        "INSERT INTO devices (id, pilot_id, token_hash, label, created_at)
         VALUES ($1, $2, $3, 'Phone', $4)",
    )
    .bind(&device_id)
    .bind(admin_pilot_id)
    .bind(auth::digest_token(&format!("dev-tok-{tag}-{device_id}")))
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO device_pair_tokens (device_id, token) VALUES ($1, $2)")
        .bind(&device_id)
        .bind(format!("pair-{tag}"))
        .execute(&db.pool)
        .await
        .unwrap();

    let flight_id = new_id();
    sqlx::query(
        "INSERT INTO flights (id, pilot_id, aircraft_id, club_id, status, started_at, visibility)
         VALUES ($1, $2, $3, $4, 'ended', $5, 'club')",
    )
    .bind(&flight_id)
    .bind(admin_pilot_id)
    .bind(&aircraft_id)
    .bind(club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO flight_devices (flight_id, device_id) VALUES ($1, $2)")
        .bind(&flight_id)
        .bind(&device_id)
        .execute(&db.pool)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO fixes (flight_id, device_id, t_ms, received_at, lat, lon, alt)
         VALUES ($1, $2, $3, $3, -33.45, -70.55, 800.0)",
    )
    .bind(&flight_id)
    .bind(&device_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

    let share_id = new_id();
    sqlx::query(
        "INSERT INTO flight_shares (id, flight_id, club_id, created_by, token_hash, created_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&share_id)
    .bind(&flight_id)
    .bind(club_id)
    .bind(admin_pilot_id)
    .bind(auth::digest_token(&format!("share-{tag}-{share_id}")))
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

    let (display_id, _display_tok) = db
        .create_display_session(admin_pilot_id, "Kiosk")
        .await
        .unwrap();

    let checkin_id = new_id();
    sqlx::query(
        "INSERT INTO checkins (id, club_id, pilot_id, device_id, time_step, token_digest, created_at, outcome)
         VALUES ($1, $2, $3, $4, 1, $5, $6, 'ok')",
    )
    .bind(&checkin_id)
    .bind(club_id)
    .bind(admin_pilot_id)
    .bind(&device_id)
    .bind(auth::digest_token(&format!("chk-{tag}-{checkin_id}")))
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

    let handoff_id = new_id();
    sqlx::query(
        "INSERT INTO checkin_handoff_tokens (id, token_hash, club_id, pilot_id, device_id, created_at, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&handoff_id)
    .bind(auth::digest_token(&format!("ho-{tag}-{handoff_id}")))
    .bind(club_id)
    .bind(admin_pilot_id)
    .bind(&device_id)
    .bind(now)
    .bind(now + 600_000)
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO admin_map_prefs (pilot_id, tails_json) VALUES ($1, '[]')
         ON CONFLICT (pilot_id) DO UPDATE SET tails_json = '[]'",
    )
    .bind(admin_pilot_id)
    .execute(&db.pool)
    .await
    .unwrap();

    let access_req_id = new_id();
    sqlx::query(
        "INSERT INTO access_requests (id, club_id, pilot_id, requested_tail, type_id, status, created_at, decided_by_pilot_id)
         VALUES ($1, $2, $3, $4, $5, 'approved', $6, $3)",
    )
    .bind(&access_req_id)
    .bind(club_id)
    .bind(admin_pilot_id)
    .bind(format!("CC-{tag}"))
    .bind(&type_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, created_at)
         VALUES ($1, $2, $3, 'seed_test', 'club', $2, $4)",
    )
    .bind(new_id())
    .bind(club_id)
    .bind(admin_pilot_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

    let _invite_tok = db
        .invite_membership(
            club_id,
            &format!("invitee-{tag}@test"),
            admin_pilot_id,
            "pilot",
            now,
        )
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO webauthn_credentials (id, pilot_id, credential_id, public_key, counter, created_at)
         VALUES ($1, $2, $3, $4, 0, $5)",
    )
    .bind(new_id())
    .bind(admin_pilot_id)
    .bind(format!("cred-{tag}-{club_id}").into_bytes())
    .bind(b"pubkey".to_vec())
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

    let guest_pilot_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Guest', $3, '', 'guest', 'pilot', '', $4)",
    )
    .bind(&guest_pilot_id)
    .bind(club_id)
    .bind(format!("guest-{tag}@guest.invalid"))
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

    let guest_session_id = new_id();
    sqlx::query(
        "INSERT INTO guest_sessions (id, club_id, guest_pilot_id, created_at, expires_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&guest_session_id)
    .bind(club_id)
    .bind(&guest_pilot_id)
    .bind(now)
    .bind(now + 3_600_000)
    .execute(&db.pool)
    .await
    .unwrap();

    SeededClubData {
        type_id,
        aircraft_id,
        device_id,
        flight_id,
        checkout_id,
        display_id,
        checkin_id,
        handoff_id,
        access_req_id,
        share_id,
        guest_pilot_id,
        guest_session_id,
    }
}

async fn send(
    router: &mut axum::Router,
    req: Request<Body>,
) -> (StatusCode, HeaderMap, serde_json::Value) {
    let res = router.call(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let json = serde_json::from_slice(&bytes).unwrap_or_else(|_| serde_json::json!({}));
    (status, headers, json)
}

fn set_cookies(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(str::to_string))
        .collect()
}

#[tokio::test]
#[serial]
async fn admin_delete_club_cascades_all_scoped_rows_and_clears_cookies() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let owner_email = "del-owner@test";
    let (club_id, orig_slug) = create_live_club(&db, "del-a", owner_email).await;
    let (admin_pid, account_id, admin_mid) = club_admin_ids(&db, &club_id).await;

    // Rename once so club_slug_history + club_slug_registry both hold rows for this club.
    let new_slug = format!("del-a-renamed-{}", &new_id()[..6]);
    db.rename_club_slug(&club_id, &new_slug, Some(&admin_pid), now)
        .await
        .unwrap();

    let seeded = seed_rich_club_rows(&db, &club_id, &admin_pid, "dela").await;
    let _extra_pilot = seed_member_with_session(&db, &club_id, "member-a@test", "pilot").await;

    // Seed global/email-scoped rows that must NOT be deleted.
    sqlx::query(
        "INSERT INTO email_suppressions (email, reason, created_at) VALUES ($1, 'bounce', $2)",
    )
    .bind(owner_email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO login_attempts (ip, email, ts) VALUES ('127.0.0.1', $1, $2)")
        .bind(owner_email)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO magic_send_log (email, ip, ts) VALUES ($1, '127.0.0.1', $2)")
        .bind(owner_email)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();

    let acct_sess = db
        .create_account_session(&account_id, Some(&admin_mid), now)
        .await
        .unwrap();
    let pilot_sess = db.create_pilot_session(&admin_pid, now).await.unwrap();

    let (mut router, db) = http_stack(db).await;
    let (status, headers, body) = send(
        &mut router,
        Request::builder()
            .method("DELETE")
            .uri("/api/admin/club")
            .header(header::CONTENT_TYPE, "application/json")
            .header("sec-fetch-site", "same-origin")
            .header(
                header::COOKIE,
                format!("fv_account_session={acct_sess}; fv_club_session={pilot_sess}; fv_club_display=fake-disp"),
            )
            .body(Body::from(format!(r#"{{"slug":"{new_slug}"}}"#)))
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["redirect"], "/account-clubs.html");
    assert!(body.get("clubId").is_none());

    let cookies = set_cookies(&headers);
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("fv_club_session=;") && c.contains("Max-Age=0")),
        "expected fv_club_session cleared: {cookies:?}"
    );
    assert!(
        cookies
            .iter()
            .any(|c| c.starts_with("fv_club_display=;") && c.contains("Max-Age=0")),
        "expected fv_club_display cleared: {cookies:?}"
    );
    assert!(
        !cookies.iter().any(|c| c.starts_with("fv_account_session=")),
        "must preserve fv_account_session: {cookies:?}"
    );

    // Club and all club-scoped rows are gone.
    let count = |q: &'static str, id: String| {
        let pool = db.pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(q)
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM clubs WHERE id = $1",
            club_id.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM pilots WHERE club_id = $1",
            club_id.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM club_memberships WHERE club_id = $1",
            club_id.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM aircraft WHERE id = $1",
            seeded.aircraft_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM aircraft_types WHERE id = $1",
            seeded.type_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM checkouts WHERE id = $1",
            seeded.checkout_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM devices WHERE id = $1",
            seeded.device_id.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM device_pair_tokens WHERE device_id = $1",
            seeded.device_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM flights WHERE id = $1",
            seeded.flight_id.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM flight_devices WHERE flight_id = $1",
            seeded.flight_id.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM fixes WHERE flight_id = $1",
            seeded.flight_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM flight_shares WHERE id = $1",
            seeded.share_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM display_sessions WHERE id = $1",
            seeded.display_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM checkins WHERE id = $1",
            seeded.checkin_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM checkin_handoff_tokens WHERE id = $1",
            seeded.handoff_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM access_requests WHERE id = $1",
            seeded.access_req_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM audit_log WHERE club_id = $1",
            club_id.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM magic_login_tokens WHERE club_id = $1",
            club_id.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM guest_sessions WHERE id = $1",
            seeded.guest_session_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM pilots WHERE id = $1",
            seeded.guest_pilot_id
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM admin_map_prefs WHERE pilot_id = $1",
            admin_pid.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM webauthn_credentials WHERE pilot_id = $1",
            admin_pid.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM club_slug_history WHERE club_id = $1",
            club_id.clone()
        )
        .await,
        0
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM club_slug_registry WHERE club_id = $1",
            club_id.clone()
        )
        .await,
        0
    );
    // Pilot session is invalidated server-side.
    assert!(db.pilot_from_session(&pilot_sess, now).await.is_none());

    // Account, account session (with NULL active_membership_id), and email-scoped tables are preserved.
    let acct_after = db.account_from_session(&acct_sess, now).await;
    assert!(acct_after.is_some(), "account session should survive");
    assert_eq!(acct_after.unwrap().1, None);
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM accounts WHERE id = $1",
            account_id
        )
        .await,
        1
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM email_suppressions WHERE email = $1",
            owner_email.to_string(),
        )
        .await,
        1
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM login_attempts WHERE email = $1",
            owner_email.to_string(),
        )
        .await,
        1
    );
    assert_eq!(
        count(
            "SELECT COUNT(*)::bigint FROM magic_send_log WHERE email = $1",
            owner_email.to_string(),
        )
        .await,
        1
    );

    // Deleted slugs are released and reusable.
    assert!(!db.club_slug_globally_taken(&new_slug, None).await.unwrap());
    assert!(!db.club_slug_globally_taken(&orig_slug, None).await.unwrap());
}

#[tokio::test]
#[serial]
async fn non_admin_pilot_and_staff_get_403_and_club_survives() {
    let db = ClubDb::open_test().await.unwrap();
    let (club_id, slug) = create_live_club(&db, "roles", "roles-owner@test").await;
    let (_, _, pilot_cookie) =
        seed_member_with_session(&db, &club_id, "plain-pilot@test", "pilot").await;
    let (_, _, staff_cookie) =
        seed_member_with_session(&db, &club_id, "club-staff@test", "staff").await;
    let (mut router, db) = http_stack(db).await;

    for (label, cookie) in [("pilot", pilot_cookie), ("staff", staff_cookie)] {
        let (status, _, body) = send(
            &mut router,
            Request::builder()
                .method("DELETE")
                .uri("/api/admin/club")
                .header(header::CONTENT_TYPE, "application/json")
                .header("sec-fetch-site", "same-origin")
                .header(header::COOKIE, cookie)
                .body(Body::from(format!(r#"{{"slug":"{slug}"}}"#)))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{label}: {body}");
        assert_eq!(body["error"], "admin only");
    }

    assert!(db.club_public_row(&club_id).await.is_some());
}

#[tokio::test]
#[serial]
async fn admin_of_club_b_cannot_delete_club_a() {
    let db = ClubDb::open_test().await.unwrap();
    let (club_a, slug_a) = create_live_club(&db, "iso-a", "iso-a@test").await;
    let (club_b, _slug_b) = create_live_club(&db, "iso-b", "iso-b@test").await;
    let (admin_b_pid, _, _) = club_admin_ids(&db, &club_b).await;
    let admin_b_sess = db
        .create_pilot_session(&admin_b_pid, now_ms())
        .await
        .unwrap();
    let (mut router, db) = http_stack(db).await;

    // Even if caller supplies club A's slug and an extraneous clubId, endpoint only targets active club B.
    let (status, _, _body) = send(
        &mut router,
        Request::builder()
            .method("DELETE")
            .uri("/api/admin/club")
            .header(header::CONTENT_TYPE, "application/json")
            .header("sec-fetch-site", "same-origin")
            .header(header::COOKIE, format!("fv_club_session={admin_b_sess}"))
            .body(Body::from(format!(
                r#"{{"slug":"{slug_a}","clubId":"{club_a}"}}"#
            )))
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(db.club_public_row(&club_a).await.is_some());
    assert!(db.club_public_row(&club_b).await.is_some());
}

#[tokio::test]
#[serial]
async fn cross_origin_and_cross_site_csrf_rejected() {
    let db = ClubDb::open_test().await.unwrap();
    let (club_id, slug) = create_live_club(&db, "csrf", "csrf-del@test").await;
    let (admin_pid, _, _) = club_admin_ids(&db, &club_id).await;
    let admin_sess = db.create_pilot_session(&admin_pid, now_ms()).await.unwrap();
    let cookie = format!("fv_club_session={admin_sess}");
    let (mut router, db) = http_stack(db).await;

    // 1. Mismatched Origin vs Host.
    let (st1, _, body1) = send(
        &mut router,
        Request::builder()
            .method("DELETE")
            .uri("/api/admin/club")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, "https://evil.example")
            .header(header::HOST, "127.0.0.1:8080")
            .header(header::COOKIE, &cookie)
            .body(Body::from(format!(r#"{{"slug":"{slug}"}}"#)))
            .unwrap(),
    )
    .await;
    assert_eq!(st1, StatusCode::FORBIDDEN);
    assert_eq!(body1["error"], "csrf");

    // 2. sec-fetch-site: cross-site.
    let (st2, _, body2) = send(
        &mut router,
        Request::builder()
            .method("DELETE")
            .uri("/api/admin/club")
            .header(header::CONTENT_TYPE, "application/json")
            .header("sec-fetch-site", "cross-site")
            .header(header::COOKIE, &cookie)
            .body(Body::from(format!(r#"{{"slug":"{slug}"}}"#)))
            .unwrap(),
    )
    .await;
    assert_eq!(st2, StatusCode::FORBIDDEN);
    assert_eq!(body2["error"], "csrf");

    // 3. sec-fetch-site: none (e.g. non-same-origin request rejected by cookie_post_allowed).
    let (st3, _, body3) = send(
        &mut router,
        Request::builder()
            .method("DELETE")
            .uri("/api/admin/club")
            .header(header::CONTENT_TYPE, "application/json")
            .header("sec-fetch-site", "none")
            .header(header::COOKIE, &cookie)
            .body(Body::from(format!(r#"{{"slug":"{slug}"}}"#)))
            .unwrap(),
    )
    .await;
    assert_eq!(st3, StatusCode::FORBIDDEN);
    assert_eq!(body3["error"], "csrf");

    assert!(db.club_public_row(&club_id).await.is_some());
}

#[tokio::test]
#[serial]
async fn wrong_slug_bad_json_wrong_content_type_and_unauthenticated_rejected() {
    let db = ClubDb::open_test().await.unwrap();
    let (club_id, slug) = create_live_club(&db, "val", "val-del@test").await;
    let (admin_pid, _, _) = club_admin_ids(&db, &club_id).await;
    let admin_sess = db.create_pilot_session(&admin_pid, now_ms()).await.unwrap();
    let cookie = format!("fv_club_session={admin_sess}");
    let (mut router, db) = http_stack(db).await;

    // Unauthenticated -> 401.
    let (st_unauth, _, _) = send(
        &mut router,
        Request::builder()
            .method("DELETE")
            .uri("/api/admin/club")
            .header(header::CONTENT_TYPE, "application/json")
            .header("sec-fetch-site", "same-origin")
            .body(Body::from(format!(r#"{{"slug":"{slug}"}}"#)))
            .unwrap(),
    )
    .await;
    assert_eq!(st_unauth, StatusCode::UNAUTHORIZED);

    // Wrong Content-Type -> 415.
    let (st_ct, _, _) = send(
        &mut router,
        Request::builder()
            .method("DELETE")
            .uri("/api/admin/club")
            .header(header::CONTENT_TYPE, "text/plain")
            .header("sec-fetch-site", "same-origin")
            .header(header::COOKIE, &cookie)
            .body(Body::from(format!(r#"{{"slug":"{slug}"}}"#)))
            .unwrap(),
    )
    .await;
    assert_eq!(st_ct, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // Invalid JSON -> 400.
    let (st_bad_json, _, _) = send(
        &mut router,
        Request::builder()
            .method("DELETE")
            .uri("/api/admin/club")
            .header(header::CONTENT_TYPE, "application/json")
            .header("sec-fetch-site", "same-origin")
            .header(header::COOKIE, &cookie)
            .body(Body::from("{not-json"))
            .unwrap(),
    )
    .await;
    assert_eq!(st_bad_json, StatusCode::BAD_REQUEST);

    // Missing slug field -> 400.
    let (st_missing, _, _) = send(
        &mut router,
        Request::builder()
            .method("DELETE")
            .uri("/api/admin/club")
            .header(header::CONTENT_TYPE, "application/json")
            .header("sec-fetch-site", "same-origin")
            .header(header::COOKIE, &cookie)
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(st_missing, StatusCode::BAD_REQUEST);

    // Wrong slug -> 400.
    let (st_wrong, _, body_wrong) = send(
        &mut router,
        Request::builder()
            .method("DELETE")
            .uri("/api/admin/club")
            .header(header::CONTENT_TYPE, "application/json")
            .header("sec-fetch-site", "same-origin")
            .header(header::COOKIE, &cookie)
            .body(Body::from(r#"{"slug":"wrong-slug-value"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(st_wrong, StatusCode::BAD_REQUEST);
    assert_eq!(body_wrong["error"], "slug mismatch");

    assert!(db.club_public_row(&club_id).await.is_some());
}

#[tokio::test]
#[serial]
async fn post_delete_switches_active_club_and_leaves_other_club_untouched() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let shared_email = "multi-club-admin@test";

    // Same account owns both Club A and Club B.
    let (club_a, slug_a) = create_live_club(&db, "multi-a", shared_email).await;
    let (club_b, slug_b) = create_live_club(&db, "multi-b", shared_email).await;
    let (admin_a_pid, account_id, mem_a) = club_admin_ids(&db, &club_a).await;
    let (admin_b_pid, account_id_b, mem_b) = club_admin_ids(&db, &club_b).await;
    assert_eq!(account_id, account_id_b);

    let _seeded_a = seed_rich_club_rows(&db, &club_a, &admin_a_pid, "multia").await;
    let seeded_b = seed_rich_club_rows(&db, &club_b, &admin_b_pid, "multib").await;
    let (pilot_b2, _, _) = seed_member_with_session(&db, &club_b, "pilot-b@test", "pilot").await;

    // Cross-club audit_log entry on Club B referencing Club A's pilot must stay (actor nulled, not deleted).
    let cross_audit_id = new_id();
    sqlx::query(
        "INSERT INTO audit_log (id, club_id, actor_pilot_id, action, entity_type, entity_id, created_at)
         VALUES ($1, $2, $3, 'cross_check', 'club', $2, $4)",
    )
    .bind(&cross_audit_id)
    .bind(&club_b)
    .bind(&admin_a_pid)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

    // Active account session currently points to Club A.
    let acct_sess = db
        .create_account_session(&account_id, Some(&mem_a), now)
        .await
        .unwrap();
    let old_club_sess = db.create_pilot_session(&admin_a_pid, now).await.unwrap();

    let (mut router, db) = http_stack(db).await;
    let (status, headers, body) = send(
        &mut router,
        Request::builder()
            .method("DELETE")
            .uri("/api/admin/club")
            .header(header::CONTENT_TYPE, "application/json")
            .header("sec-fetch-site", "same-origin")
            .header("x-forwarded-proto", "https")
            .header(
                header::COOKIE,
                format!("fv_account_session={acct_sess}; fv_club_session={old_club_sess}"),
            )
            .body(Body::from(format!(r#"{{"slug":"{slug_a}"}}"#)))
            .unwrap(),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["redirect"], "/account-clubs.html");
    assert_eq!(body["clubId"], club_b);
    assert_eq!(body["slug"], slug_b);

    // Account session switched to Club B's membership in Postgres.
    let (_, active_mem_after) = db.account_from_session(&acct_sess, now).await.unwrap();
    assert_eq!(active_mem_after.as_deref(), Some(mem_b.as_str()));

    // New fv_club_session cookie minted with Secure flag and resolves to Club B's admin pilot.
    let cookies = set_cookies(&headers);
    let new_club_cookie = cookies
        .iter()
        .find(|c| c.starts_with("fv_club_session=") && !c.starts_with("fv_club_session=;"))
        .expect("new fv_club_session cookie");
    assert!(new_club_cookie.contains("HttpOnly"));
    assert!(new_club_cookie.contains("SameSite=Lax"));
    assert!(new_club_cookie.contains("Path=/"));
    assert!(new_club_cookie.contains("Secure"));
    let new_sess_raw = new_club_cookie
        .strip_prefix("fv_club_session=")
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    let resolved_pilot = db.pilot_from_session(new_sess_raw, now).await.unwrap();
    assert_eq!(resolved_pilot.id, admin_b_pid);
    assert_eq!(resolved_pilot.club_id, club_b);

    // Old Club A pilot session is gone, display cookie cleared with Secure.
    assert!(db.pilot_from_session(&old_club_sess, now).await.is_none());
    assert!(cookies.iter().any(|c| {
        c.starts_with("fv_club_display=;") && c.contains("Max-Age=0") && c.contains("Secure")
    }));

    // Club A is deleted, while ALL of Club B's rows remain untouched.
    assert!(db.club_public_row(&club_a).await.is_none());
    assert!(db.club_public_row(&club_b).await.is_some());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM pilots WHERE club_id = $1")
            .bind(&club_b)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        3 // admin_b + pilot_b2 + guest_b
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM pilots WHERE id = $1")
            .bind(&pilot_b2)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM club_memberships WHERE club_id = $1"
        )
        .bind(&club_b)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM aircraft WHERE id = $1")
            .bind(&seeded_b.aircraft_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM flights WHERE id = $1")
            .bind(&seeded_b.flight_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM fixes WHERE flight_id = $1")
            .bind(&seeded_b.flight_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM checkouts WHERE id = $1")
            .bind(&seeded_b.checkout_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM display_sessions WHERE id = $1")
            .bind(&seeded_b.display_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*)::bigint FROM checkins WHERE id = $1")
            .bind(&seeded_b.checkin_id)
            .fetch_one(&db.pool)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::bigint FROM checkin_handoff_tokens WHERE id = $1"
        )
        .bind(&seeded_b.handoff_id)
        .fetch_one(&db.pool)
        .await
        .unwrap(),
        1
    );
    let cross_audit_actor: Option<String> =
        sqlx::query_scalar("SELECT actor_pilot_id FROM audit_log WHERE id = $1")
            .bind(&cross_audit_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(cross_audit_actor, None);
}
