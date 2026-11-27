//! Integration tests for `DELETE /api/admin/club` (hard delete, auth, CSRF, slug confirm, active-club switch).

use crate::club::auth::{self, hash_password, new_id};
use crate::club::db::now_ms;
use crate::club::db_club_lifecycle::{ClubDeleteError, ClubSignupError, PENDING_CLUB_TTL_MS};
use crate::club::db_club_slug::{ClubSlugRenameError, CLUB_SLUG_DELETE_COOLDOWN_MS};
use crate::club::ClubDb;
use axum::body::{to_bytes, Body};
use axum::http::{header, HeaderMap, Request, StatusCode};
use serial_test::serial;
use std::collections::{BTreeSet, HashSet};
use tower::Service;

async fn http_stack(db: ClubDb) -> (axum::Router, ClubDb) {
    http_stack_with_superadmins(db, HashSet::new()).await
}

async fn http_stack_with_superadmins(
    db: ClubDb,
    superadmin_emails: HashSet<String>,
) -> (axum::Router, ClubDb) {
    use crate::{
        auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
    };
    use std::sync::Arc;
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
        public_live_trails_throttle: crate::LoginThrottle::with_policy(
            crate::OverflowPolicy::RejectNew,
        ),
        mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
        webauthn: None,
        sse_limiter: Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails,
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
                format!("fv_account_session={acct_sess}; fv_club_session={pilot_sess}"),
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
            account_id.clone()
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

    // Tombstone records club id, name, actor account, timestamp, and both current + historical slugs.
    let tombstones = db.list_club_delete_tombstones().await.unwrap();
    assert_eq!(tombstones.len(), 1);
    let tomb = &tombstones[0];
    assert_eq!(tomb.club_id, club_id);
    assert_eq!(tomb.name, "Club del-a");
    assert_eq!(tomb.actor_account_id.as_deref(), Some(account_id.as_str()));
    assert!(tomb.deleted_at >= now);
    let mut expected_slugs = vec![orig_slug.clone(), new_slug.clone()];
    expected_slugs.sort();
    assert_eq!(tomb.slugs, expected_slugs);

    // Deleted current and historical slugs stay blocked during the cooldown window.
    assert!(db
        .club_slug_globally_taken_at(&new_slug, None, tomb.deleted_at)
        .await
        .unwrap());
    assert!(db
        .club_slug_globally_taken_at(&orig_slug, None, tomb.deleted_at)
        .await
        .unwrap());
    assert!(db.club_slug_globally_taken(&new_slug, None).await.unwrap());
    assert!(db.club_slug_globally_taken(&orig_slug, None).await.unwrap());
    assert!(!db
        .club_slug_globally_taken_at(
            &new_slug,
            None,
            tomb.deleted_at + CLUB_SLUG_DELETE_COOLDOWN_MS,
        )
        .await
        .unwrap());
    assert!(!db
        .club_slug_globally_taken_at(
            &orig_slug,
            None,
            tomb.deleted_at + CLUB_SLUG_DELETE_COOLDOWN_MS,
        )
        .await
        .unwrap());
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

    // Old Club A pilot session is gone.
    assert!(db.pilot_from_session(&old_club_sess, now).await.is_none());

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

#[tokio::test]
#[serial]
async fn delete_club_tombstone_atomic_write_and_rollback() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let (club_id, orig_slug) = create_live_club(&db, "atom", "atom-owner@test").await;
    let (admin_pid, account_id, _) = club_admin_ids(&db, &club_id).await;

    let mid_slug = format!("atom-mid-{}", &new_id()[..6]);
    let final_slug = format!("atom-final-{}", &new_id()[..6]);
    db.rename_club_slug(&club_id, &mid_slug, Some(&admin_pid), now + 1)
        .await
        .unwrap();
    db.rename_club_slug(&club_id, &final_slug, Some(&admin_pid), now + 2)
        .await
        .unwrap();

    // Install a temporary BEFORE DELETE trigger on `clubs` (runs after the tombstone INSERT).
    sqlx::query("DROP TRIGGER IF EXISTS trg_test_fail_club_delete ON clubs")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE OR REPLACE FUNCTION test_fail_club_delete() RETURNS trigger AS $$
         BEGIN
             RAISE EXCEPTION 'forced club delete failure';
         END;
         $$ LANGUAGE plpgsql",
    )
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER trg_test_fail_club_delete
         BEFORE DELETE ON clubs
         FOR EACH ROW EXECUTE FUNCTION test_fail_club_delete()",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    let deleted_at = now + 5_000;
    let failed = db
        .delete_club(&club_id, &final_slug, Some(&account_id), deleted_at)
        .await;

    // Always clean up the trigger and helper function before asserting.
    sqlx::query("DROP TRIGGER IF EXISTS trg_test_fail_club_delete ON clubs")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION IF EXISTS test_fail_club_delete()")
        .execute(&db.pool)
        .await
        .unwrap();

    assert_eq!(failed, Err(ClubDeleteError::Db));
    // Rollback must preserve the club + slug history and commit zero tombstone rows.
    assert!(db.club_public_row(&club_id).await.is_some());
    let hist_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM club_slug_history WHERE club_id = $1")
            .bind(&club_id)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(hist_count, 2);
    let tomb_count: i64 = sqlx::query_scalar("SELECT COUNT(*)::bigint FROM club_delete_tombstones")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(tomb_count, 0);
    let tomb_slug_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::bigint FROM club_delete_tombstone_slugs")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(tomb_slug_count, 0);

    // Without the failing trigger, delete succeeds and atomically records all current + historical slugs.
    db.delete_club(&club_id, &final_slug, Some(&account_id), deleted_at)
        .await
        .unwrap();
    assert!(db.club_public_row(&club_id).await.is_none());

    let tombstones = db.list_club_delete_tombstones().await.unwrap();
    assert_eq!(tombstones.len(), 1);
    let tomb = &tombstones[0];
    assert_eq!(tomb.club_id, club_id);
    assert_eq!(tomb.name, "Club atom");
    assert_eq!(tomb.actor_account_id.as_deref(), Some(account_id.as_str()));
    assert_eq!(tomb.deleted_at, deleted_at);
    let mut expected_slugs = vec![orig_slug, mid_slug, final_slug];
    expected_slugs.sort();
    assert_eq!(tomb.slugs, expected_slugs);

    // Passing actor_account_id = None is permitted and stores NULL.
    let (club2, slug2) = create_live_club(&db, "nullactor", "nullactor@test").await;
    db.delete_club(&club2, &slug2, None, deleted_at + 10)
        .await
        .unwrap();
    let tombstones = db.list_club_delete_tombstones().await.unwrap();
    assert_eq!(tombstones.len(), 2);
    let tomb2 = tombstones.iter().find(|t| t.club_id == club2).unwrap();
    assert_eq!(tomb2.actor_account_id, None);
    assert_eq!(tomb2.slugs, vec![slug2]);
}

#[tokio::test]
#[serial]
async fn deleted_slugs_and_confusables_blocked_during_cooldown() {
    let db = ClubDb::open_test().await.unwrap();
    let t0 = now_ms();
    let suffix = &new_id()[..6];
    let orig_slug = format!("cas-{suffix}");
    let curr_slug = format!("sol-{suffix}");
    let orig_confusable = format!("ca5-{suffix}");
    let curr_confusable = format!("5ol-{suffix}");

    let (club_a, tok_a) = db
        .signup_club("Club Cooldown A", &orig_slug, "cool-a@test", None, t0)
        .await
        .unwrap();
    db.try_consume_magic_club_verify(&tok_a, t0).await.unwrap();
    let (admin_a_pid, account_a_id, _) = club_admin_ids(&db, &club_a).await;
    db.rename_club_slug(&club_a, &curr_slug, Some(&admin_a_pid), t0 + 1)
        .await
        .unwrap();

    let (club_b, _slug_b) = create_live_club(&db, "live-b", "cool-b@test").await;
    let (admin_b_pid, _, _) = club_admin_ids(&db, &club_b).await;
    let admin_b_cookie = format!(
        "fv_club_session={}",
        db.create_pilot_session(&admin_b_pid, t0).await.unwrap()
    );

    // Verified allowlisted superadmin for HTTP superadmin rename check.
    let default_id = db.default_club_id().await.unwrap();
    let super_email = "super-cool@test".to_string();
    let super_pid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, 'Super Cool', $3, '', 'approved', 'pilot', '', $4, $4)",
    )
    .bind(&super_pid)
    .bind(&default_id)
    .bind(&super_email)
    .bind(t0)
    .execute(&db.pool)
    .await
    .unwrap();
    let super_cookie = format!(
        "fv_club_session={}",
        db.create_pilot_session(&super_pid, t0).await.unwrap()
    );

    // Delete club A at t0 + 100.
    let del_at = t0 + 100;
    db.delete_club(&club_a, &curr_slug, Some(&account_a_id), del_at)
        .await
        .unwrap();

    // Still inside cooldown (1 second before expiry).
    let t_within = del_at + CLUB_SLUG_DELETE_COOLDOWN_MS - 1_000;
    for candidate in [&curr_slug, &orig_slug, &curr_confusable, &orig_confusable] {
        assert!(
            db.club_slug_globally_taken_at(candidate, None, t_within)
                .await
                .unwrap(),
            "expected {candidate} blocked during cooldown"
        );
        assert_eq!(
            db.signup_club("New Club", candidate, "new-signup@test", None, t_within)
                .await,
            Err(ClubSignupError::SlugTaken),
            "signup_club should return SlugTaken for {candidate}"
        );
        assert_eq!(
            db.rename_club_slug(&club_b, candidate, Some(&admin_b_pid), t_within)
                .await,
            Err(ClubSlugRenameError::Taken),
            "rename_club_slug should return Taken for {candidate}"
        );
    }

    // HTTP club-admin and superadmin rename return identical 409 + {"error":"slug taken"}.
    let mut allow = HashSet::new();
    allow.insert(super_email);
    let (mut router, _db) = http_stack_with_superadmins(db, allow).await;

    for candidate in [&curr_slug, &orig_slug, &curr_confusable] {
        let (st, _, body) = send(
            &mut router,
            Request::builder()
                .method("POST")
                .uri("/api/admin/club/slug")
                .header(header::CONTENT_TYPE, "application/json")
                .header("sec-fetch-site", "same-origin")
                .header(header::COOKIE, &admin_b_cookie)
                .body(Body::from(format!(r#"{{"slug":"{candidate}"}}"#)))
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "admin rename {candidate}: {body}");
        assert_eq!(body["error"], "slug taken");
    }

    for candidate in [&curr_slug, &orig_slug] {
        let (st, _, body) = send(
            &mut router,
            Request::builder()
                .method("POST")
                .uri(format!("/api/superadmin/clubs/{club_b}/slug"))
                .header(header::CONTENT_TYPE, "application/json")
                .header("sec-fetch-site", "same-origin")
                .header(header::COOKIE, &super_cookie)
                .body(Body::from(format!(r#"{{"slug":"{candidate}"}}"#)))
                .unwrap(),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::CONFLICT,
            "superadmin rename {candidate}: {body}"
        );
        assert_eq!(body["error"], "slug taken");
    }
}

#[tokio::test]
#[serial]
async fn deleted_slug_reusable_after_cooldown_and_re_tombstoned_on_second_delete() {
    let db = ClubDb::open_test().await.unwrap();
    let t0 = now_ms();
    let suffix = &new_id()[..6];
    let orig_slug = format!("reuse-o-{suffix}");
    let curr_slug = format!("reuse-c-{suffix}");

    let (club_a, tok_a) = db
        .signup_club("Reuse A", &orig_slug, "reuse-a@test", None, t0)
        .await
        .unwrap();
    db.try_consume_magic_club_verify(&tok_a, t0).await.unwrap();
    let (admin_a_pid, account_a_id, _) = club_admin_ids(&db, &club_a).await;
    db.rename_club_slug(&club_a, &curr_slug, Some(&admin_a_pid), t0 + 1)
        .await
        .unwrap();

    let (club_b, _slug_b) = create_live_club(&db, "reuse-b", "reuse-b@test").await;

    // Delete club A at t0 + 100.
    let del1_at = t0 + 100;
    db.delete_club(&club_a, &curr_slug, Some(&account_a_id), del1_at)
        .await
        .unwrap();

    // Right at cooldown expiry: deleted_at > now - cooldown is false -> reusable.
    let t_after = del1_at + CLUB_SLUG_DELETE_COOLDOWN_MS;
    assert!(!db
        .club_slug_globally_taken_at(&curr_slug, None, t_after)
        .await
        .unwrap());
    assert!(!db
        .club_slug_globally_taken_at(&orig_slug, None, t_after)
        .await
        .unwrap());

    // Self-serve signup reuses the previously deleted current slug.
    let (club_c, tok_c) = db
        .signup_club("Reuse C", &curr_slug, "reuse-c@test", None, t_after)
        .await
        .unwrap();
    db.try_consume_magic_club_verify(&tok_c, t_after)
        .await
        .unwrap();
    let (_, account_c_id, _) = club_admin_ids(&db, &club_c).await;

    // Another live club renames into the previously deleted historical slug.
    db.rename_club_slug(&club_b, &orig_slug, None, t_after)
        .await
        .unwrap();
    assert_eq!(
        db.club_slug(&club_b).await.as_deref(),
        Some(orig_slug.as_str())
    );

    // Deleting the reused club writes a second tombstone for the same slug and starts a fresh cooldown.
    let del2_at = t_after + 10_000;
    db.delete_club(&club_c, &curr_slug, Some(&account_c_id), del2_at)
        .await
        .unwrap();

    let tombstones = db.list_club_delete_tombstones().await.unwrap();
    assert_eq!(tombstones.len(), 2);
    assert_eq!(tombstones[0].club_id, club_c);
    assert_eq!(tombstones[0].deleted_at, del2_at);
    assert_eq!(tombstones[0].slugs, vec![curr_slug.clone()]);
    assert_eq!(tombstones[1].club_id, club_a);
    assert_eq!(tombstones[1].deleted_at, del1_at);

    let t_fresh_within = del2_at + CLUB_SLUG_DELETE_COOLDOWN_MS - 1_000;
    assert!(db
        .club_slug_globally_taken_at(&curr_slug, None, t_fresh_within)
        .await
        .unwrap());
    assert_eq!(
        db.signup_club("Reuse D", &curr_slug, "reuse-d@test", None, t_fresh_within)
            .await,
        Err(ClubSignupError::SlugTaken)
    );
    assert_eq!(
        db.rename_club_slug(&club_b, &curr_slug, None, t_fresh_within)
            .await,
        Err(ClubSlugRenameError::Taken)
    );
    assert!(!db
        .club_slug_globally_taken_at(&curr_slug, None, del2_at + CLUB_SLUG_DELETE_COOLDOWN_MS)
        .await
        .unwrap());

    // Unverified stale pending club purge must NOT write a tombstone and frees its slug immediately.
    let pending_slug = format!("pend-{}", &new_id()[..8]);
    let (pending_id, _) = db
        .signup_club("Pending Only", &pending_slug, "pend@test", None, t0)
        .await
        .unwrap();
    sqlx::query("UPDATE clubs SET created_at = $1 WHERE id = $2")
        .bind(t0 - PENDING_CLUB_TTL_MS - 60_000)
        .bind(&pending_id)
        .execute(&db.pool)
        .await
        .unwrap();
    let purged = db.purge_stale_pending_clubs(t0).await.unwrap();
    assert_eq!(purged, 1);
    assert_eq!(db.list_club_delete_tombstones().await.unwrap().len(), 2);
    assert!(!db
        .club_slug_globally_taken_at(&pending_slug, None, t0)
        .await
        .unwrap());
}

#[tokio::test]
#[serial]
async fn superadmin_club_tombstones_authz_and_payload() {
    let db = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let (club_a, orig_slug) = create_live_club(&db, "tomb-a", "tomb-a@test").await;
    let (admin_a_pid, account_a_id, _) = club_admin_ids(&db, &club_a).await;
    let renamed_slug = format!("tomb-a-new-{}", &new_id()[..6]);
    db.rename_club_slug(&club_a, &renamed_slug, Some(&admin_a_pid), now + 1)
        .await
        .unwrap();
    let deleted_at = now + 2_000;
    db.delete_club(&club_a, &renamed_slug, Some(&account_a_id), deleted_at)
        .await
        .unwrap();

    // Non-superadmin identities on another live club.
    let (club_b, _) = create_live_club(&db, "tomb-b", "tomb-b@test").await;
    let (admin_b_pid, admin_b_acct, admin_b_mem) = club_admin_ids(&db, &club_b).await;
    let admin_b_cookie = format!(
        "fv_account_session={}; fv_club_session={}",
        db.create_account_session(&admin_b_acct, Some(&admin_b_mem), now)
            .await
            .unwrap(),
        db.create_pilot_session(&admin_b_pid, now).await.unwrap()
    );
    let (_, _, pilot_cookie) =
        seed_member_with_session(&db, &club_b, "tomb-pilot@test", "pilot").await;
    let (_, _, staff_cookie) =
        seed_member_with_session(&db, &club_b, "tomb-staff@test", "staff").await;

    // Verified allowlisted superadmin (matches verified_allowlisted_superadmin_lists_clubs).
    let default_id = db.default_club_id().await.unwrap();
    let super_email = "super-tomb@test".to_string();
    let super_pid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, 'Super Tomb', $3, '', 'approved', 'pilot', '', $4, $4)",
    )
    .bind(&super_pid)
    .bind(&default_id)
    .bind(&super_email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let allowlisted_super_cookie = format!(
        "fv_club_session={}",
        db.create_pilot_session(&super_pid, now).await.unwrap()
    );

    // Account platform superadmin (`accounts.global_role = 'superadmin'`).
    let acct_super_id = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, global_role, email_verified_at, created_at)
         VALUES ($1, 'acct-super-tomb@test', 'superadmin', $2, $2)",
    )
    .bind(&acct_super_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let acct_super_cookie = format!(
        "fv_account_session={}",
        db.create_account_session(&acct_super_id, None, now)
            .await
            .unwrap()
    );

    let mut emails = HashSet::new();
    emails.insert(super_email);
    let (mut router, _db) = http_stack_with_superadmins(db, emails).await;

    // Anonymous, club admin, staff, and ordinary pilot are all forbidden.
    for (label, cookie) in [
        ("anonymous", None),
        ("club_admin", Some(admin_b_cookie.as_str())),
        ("staff", Some(staff_cookie.as_str())),
        ("pilot", Some(pilot_cookie.as_str())),
    ] {
        let mut req = Request::builder()
            .method("GET")
            .uri("/api/superadmin/club-tombstones");
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        let (st, _, body) = send(&mut router, req.body(Body::empty()).unwrap()).await;
        assert_eq!(st, StatusCode::FORBIDDEN, "{label}: {body}");
        assert_eq!(body["error"], "forbidden");
    }

    // Both allowlisted superadmin and account platform superadmin get 200 and see the tombstone.
    let mut expected_slugs = vec![orig_slug, renamed_slug];
    expected_slugs.sort();
    let expected_keys: BTreeSet<&str> = [
        "id",
        "clubId",
        "name",
        "slugs",
        "actorAccountId",
        "deletedAt",
    ]
    .into_iter()
    .collect();

    for (label, cookie) in [
        ("allowlisted_superadmin", allowlisted_super_cookie.as_str()),
        ("account_superadmin", acct_super_cookie.as_str()),
    ] {
        let (st, _, body) = send(
            &mut router,
            Request::builder()
                .method("GET")
                .uri("/api/superadmin/club-tombstones")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{label}: {body}");
        let items = body["tombstones"].as_array().expect("tombstones array");
        assert_eq!(items.len(), 1);
        let obj = items[0].as_object().expect("tombstone object");
        let keys: BTreeSet<&str> = obj.keys().map(String::as_str).collect();
        assert_eq!(keys, expected_keys, "no extra PII fields on tombstone");
        assert_eq!(obj["clubId"], club_a);
        assert_eq!(obj["name"], "Club tomb-a");
        assert_eq!(obj["actorAccountId"], account_a_id);
        assert_eq!(obj["deletedAt"], deleted_at);
        let got_slugs: Vec<String> = obj["slugs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(got_slugs, expected_slugs);
    }

    // Public club listing does not expose tombstones or deleted clubs.
    let (pub_st, _, pub_body) = send(
        &mut router,
        Request::builder()
            .method("GET")
            .uri("/api/clubs/public")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(pub_st, StatusCode::OK);
    assert!(pub_body.get("tombstones").is_none());
    let pub_slugs: Vec<&str> = pub_body["clubs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["slug"].as_str())
        .collect();
    assert!(!pub_slugs
        .iter()
        .any(|s| expected_slugs.iter().any(|e| e == s)));
}
