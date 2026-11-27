//! E1 integration & security tests: club-less (public pilot) mode — schema, auth,
//! start-flight without check-in, email verification gate, rate limits, multi-club check-in,
//! device isolation, trail fallback, guest re-scope guard, and club/admin/kiosk isolation.

use crate::{
    auth::Auth,
    build_router,
    club::{
        auth::{hash_password, new_id, ACCOUNT_COOKIE},
        checkin::{current_step, mint_checkin_token},
        db::{normalize_free_tail, now_ms, ACCOUNT_SIGNUP_MAX_PER_IP},
        mail::{MailOutbox, Mailer},
        pilot_live_trails_tests::FlightDataEnvGuard,
        trail::DEFAULT_TRAIL_MINUTES,
        visibility::FlightVisibility,
        ClubDb,
    },
    device_auth::DeviceAuth,
    live::{LiveEngine, LiveFix, CLUB_INGEST_WINDOW_MS, MAX_CLUB_INGEST_FIXES_PER_MINUTE},
    store::Store,
    AppState,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use serde_json::{json, Value};
use serial_test::serial;
use std::{
    collections::HashSet,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
};
use tower::Service;

// ---------------------------------------------------------------------------
// Fixtures & HTTP helpers
// ---------------------------------------------------------------------------

struct TestHarness {
    _dir: tempfile::TempDir,
    db: ClubDb,
    mailer: Mailer,
    router: axum::Router,
    clock_ms: Arc<AtomicI64>,
}

async fn setup_harness() -> TestHarness {
    let locales = crate::i18n::Catalogs::resolve_dir(std::path::Path::new("dist"));
    let _ = crate::i18n::init_global(&locales);
    let dir = tempfile::tempdir().unwrap();
    let dist_dir = dir.path().join("dist");
    std::fs::create_dir_all(&dist_dir).unwrap();
    let db = ClubDb::open_test().await.expect("open test db");
    let outbox = MailOutbox::default();
    let mailer = Mailer::from_env(outbox);
    let clock_ms = Arc::new(AtomicI64::new(now_ms()));
    let clock_clone = Arc::clone(&clock_ms);
    let live = Arc::new(LiveEngine::with_clock(
        dir.path(),
        Arc::new(move || clock_clone.load(Ordering::SeqCst)),
    ));
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir,
        auth: Auth::with_password("test-secret"),
        device_auth: DeviceAuth::empty(),
        live,
        store: Store::new(dir.path()),
        club: Some(db.clone()),
        video_queue,
        login_throttle: crate::LoginThrottle::default(),
        device_register_throttle: crate::LoginThrottle::default(),
        share_lookup_throttle: crate::LoginThrottle::default(),
        account_password_throttle: crate::LoginThrottle::default(),
        account_password_account_throttle: crate::LoginThrottle::default(),
        public_live_trails_throttle: crate::LoginThrottle::default(),
        mailer: mailer.clone(),
        webauthn: None,
        sse_limiter: Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails: HashSet::new(),
    };
    let router = build_router(state);
    TestHarness {
        _dir: dir,
        db,
        mailer,
        router,
        clock_ms,
    }
}

async fn send_json(
    router: &mut axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::ACCEPT, "application/json");
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    if let Some(tok) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {tok}"));
    }
    let payload = if let Some(v) = body {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        v.to_string()
    } else {
        String::new()
    };
    let req = builder.body(Body::from(payload)).unwrap();
    let res = router.call(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let val = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, val)
}

fn extract_cookie(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    headers.get_all(header::SET_COOKIE).iter().find_map(|v| {
        let s = v.to_str().ok()?;
        let pair = s.split(';').next()?.trim();
        pair.starts_with(&prefix).then(|| pair.to_string())
    })
}

fn extract_latest_magic_token(mailer: &Mailer, email: &str) -> String {
    let mails = mailer.captured();
    let mail = mails
        .iter()
        .rev()
        .find(|m| m.to.eq_ignore_ascii_case(email))
        .unwrap_or_else(|| panic!("no mail sent to {email}"));
    let idx = mail
        .body
        .find("token=")
        .unwrap_or_else(|| panic!("no token= in mail body: {}", mail.body));
    mail.body[idx + 6..]
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '&' && *c != '"' && *c != '\'')
        .collect()
}

async fn create_live_club(
    db: &ClubDb,
    prefix: &str,
    owner_email: &str,
    icao: &str,
    lat: f64,
    lon: f64,
) -> (String, String) {
    let now = now_ms();
    let slug = format!("{prefix}-{}", &new_id()[..8]);
    let (club_id, tok) = db
        .signup_club(
            &format!("Club {prefix}"),
            &slug,
            owner_email,
            Some(icao),
            now,
        )
        .await
        .expect("signup_club");
    db.try_consume_magic_club_verify(&tok, now)
        .await
        .expect("verify club");
    db.set_club_base_aerodrome(&club_id, icao, lat, lon)
        .await
        .expect("set base");
    (club_id, slug)
}

async fn create_account_with_session(
    db: &ClubDb,
    email: &str,
    verified: bool,
    password: Option<&str>,
) -> (String, String) {
    let now = now_ms();
    let acct_id = new_id();
    let pw_hash = password.map(|p| hash_password(p).unwrap());
    sqlx::query(
        "INSERT INTO accounts (id, email, password_hash, email_verified_at, global_role, created_at)
         VALUES ($1, $2, $3, $4, '', $5)",
    )
    .bind(&acct_id)
    .bind(email.to_ascii_lowercase())
    .bind(pw_hash)
    .bind(verified.then_some(now))
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let sess = db
        .create_account_session_auth(&acct_id, None, now, "magic")
        .await
        .unwrap();
    (acct_id, format!("{ACCOUNT_COOKIE}={sess}"))
}

async fn add_active_club_membership(
    db: &ClubDb,
    account_id: &str,
    email: &str,
    name: &str,
    club_id: &str,
    role: &str,
) -> (String, String) {
    let now = now_ms();
    let pilot_id = new_id();
    let hash = hash_password("password12").unwrap();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, $3, $4, $5, '', 'approved', $6, $7, $8, $8)",
    )
    .bind(&pilot_id)
    .bind(club_id)
    .bind(account_id)
    .bind(name)
    .bind(email.to_ascii_lowercase())
    .bind(role)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let mem_id = new_id();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, $4, 'active', $5)",
    )
    .bind(&mem_id)
    .bind(&pilot_id)
    .bind(club_id)
    .bind(role)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    (pilot_id, mem_id)
}

async fn qr_token_for_club(db: &ClubDb, club_id: &str, now: i64) -> String {
    let (secret, _) = db.club_checkin_settings(club_id).await.unwrap();
    mint_checkin_token(&secret, club_id, current_step(now))
}

// ---------------------------------------------------------------------------
// 8(a) Sign-up (no clubSlug) -> magic complete (app JSON) -> personal pilot device (idempotent)
//      + app account login + dedicated signup rate limit
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn clubless_signup_magic_complete_and_idempotent_personal_device() {
    let mut fx = setup_harness().await;
    let email = format!("cl-a-{}@test.example", &new_id()[..8]);

    // 1. Sign up without clubSlug.
    let (st, _hdrs, body) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/signup",
        None,
        None,
        Some(json!({ "email": email })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    assert_eq!(body["ok"], true);

    let token = extract_latest_magic_token(&fx.mailer, &email);

    // 2. Consume magic link via app-friendly JSON endpoint (with installId to mint personal device).
    let install_id = format!("inst-app-{}", &new_id()[..10]);
    let (st, hdrs, body) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/auth/magic/complete",
        None,
        None,
        Some(json!({
            "token": token,
            "installId": install_id,
            "label": "Pixel 9",
            "model": "Pixel 9"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true);
    assert_eq!(body["account"]["email"], email);
    assert_eq!(body["account"]["emailVerified"], true);
    assert!(body["activeMembershipId"].is_null());
    assert_eq!(body["memberships"].as_array().map(Vec::len), Some(0));
    assert!(body["device"]["id"].is_string());
    assert!(body["device"]["token"].is_string());
    assert_eq!(body["device"]["created"], true);

    let acct_cookie = extract_cookie(&hdrs, ACCOUNT_COOKIE).expect("fv_account_session cookie set");
    let acct_id = body["account"]["id"].as_str().unwrap().to_string();

    // 3. Replaying the magic token is rejected (one-time).
    let (st, _, _) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/auth/magic/complete",
        None,
        None,
        Some(json!({ "token": token })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // 4. Minting via POST /api/account/devices a second time is idempotent and keeps ONE personal pilot.
    let (st, _, dev2) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/devices",
        Some(&acct_cookie),
        None,
        Some(json!({
            "installId": install_id,
            "label": "Pixel 9 Updated",
            "model": "Pixel 9"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{dev2}");
    assert_eq!(dev2["id"], body["device"]["id"]);
    assert_eq!(dev2["created"], false);

    // 5. Concurrent ensure_personal_pilot calls race-safely converge on the single personal pilot row.
    let now = now_ms();
    let (r1, r2, r3) = tokio::join!(
        fx.db.ensure_personal_pilot(&acct_id, now),
        fx.db.ensure_personal_pilot(&acct_id, now),
        fx.db.ensure_personal_pilot(&acct_id, now),
    );
    let pid1 = r1.unwrap();
    assert_eq!(pid1, r2.unwrap());
    assert_eq!(pid1, r3.unwrap());

    let personal_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pilots WHERE account_id = $1 AND club_id IS NULL")
            .bind(&acct_id)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(personal_count, 1);

    // 6. Account password login via POST /api/account/login returns account JSON + device + cookie.
    let email_pw = format!("cl-pw-{}@test.example", &new_id()[..8]);
    let (_aid2, _) =
        create_account_with_session(&fx.db, &email_pw, true, Some("secretpass12")).await;
    let pw_install = format!("inst-pw-{}", &new_id()[..10]);
    let (st, login_hdrs, login_body) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/login",
        None,
        None,
        Some(json!({
            "email": email_pw,
            "password": "secretpass12",
            "installId": pw_install,
            "label": "Phone",
            "model": "Android"
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{login_body}");
    assert!(extract_cookie(&login_hdrs, ACCOUNT_COOKIE).is_some());
    assert_eq!(login_body["account"]["emailVerified"], true);
    assert!(login_body["activeMembershipId"].is_null());
    assert_eq!(login_body["memberships"].as_array().map(Vec::len), Some(0));
    assert!(login_body["device"]["token"].is_string());

    // 7. Dedicated signup IP rate limit triggers 429 after ACCOUNT_SIGNUP_MAX_PER_IP without
    //    affecting device registration throttle.
    for i in 0..ACCOUNT_SIGNUP_MAX_PER_IP {
        let (st, _, _) = send_json(
            &mut fx.router,
            "POST",
            "/api/account/signup",
            None,
            None,
            Some(json!({ "email": format!("rl-{i}-{}@test.example", &new_id()[..6]) })),
        )
        .await;
        // 1 signup was already consumed at step 1 from the same test IP ("unknown"), so the
        // ACCOUNT_SIGNUP_MAX_PER_IP-th iteration hits 429.
        if i + 1 < ACCOUNT_SIGNUP_MAX_PER_IP {
            assert_eq!(st, StatusCode::CREATED);
        } else {
            assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
        }
    }
    // Device registration still succeeds (separate throttle from account signup).
    let (st, _, dev_after) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/devices",
        Some(&acct_cookie),
        None,
        Some(json!({ "label": "Tablet" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{dev_after}");
}

// ---------------------------------------------------------------------------
// 8(b) Email verification gate: private/default OK unverified; public blocked
//      (403 email_not_verified) until account email verified; PATCH private->public
//      blocked unverified, OK verified; ownership check runs before email check;
//      GET /api/account/me exposes single email_verified boolean (no duplicate key)
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn clubless_email_verification_gates_public_start_and_patch() {
    let mut fx = setup_harness().await;
    let email = format!("cl-unv-{}@test.example", &new_id()[..8]);
    let (acct_id, acct_cookie) = create_account_with_session(&fx.db, &email, false, None).await;

    // GET /api/account/me exposes main's top-level email_verified boolean without duplicate account.emailVerified.
    let (st, _, me_unver) = send_json(
        &mut fx.router,
        "GET",
        "/api/account/me",
        Some(&acct_cookie),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{me_unver}");
    assert_eq!(me_unver["email_verified"], json!(false));
    assert!(me_unver["account"]["emailVerifiedAt"].is_null());
    assert!(
        me_unver["account"].get("emailVerified").is_none(),
        "GET /api/account/me must not emit duplicate account.emailVerified key: {me_unver}"
    );

    // Mint a personal device via POST /api/account/devices.
    let (st, _, dev_body) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/devices",
        Some(&acct_cookie),
        None,
        Some(json!({ "label": "Phone" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{dev_body}");
    let dev_tok = dev_body["token"].as_str().unwrap().to_string();

    // 1. Public start blocked while account email is unverified (main's email_not_verified contract).
    let (st, _, body) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        Some(&acct_cookie),
        Some(&dev_tok),
        Some(json!({ "tail": "CC-PUB1", "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["error"], "email_not_verified");
    assert_eq!(
        body["message"],
        "Verify your email to share flights publicly."
    );
    assert!(body.get("reason").is_none());

    // Even if pilots.email_verified_at is manually populated on a personal pilot row while
    // accounts.email_verified_at IS NULL, personal pilots stay strictly account-bound -> still 403.
    let personal_pid = fx
        .db
        .personal_pilot_id_for_account(&acct_id)
        .await
        .expect("personal pilot exists");
    fx.db
        .mark_email_verified(&personal_pid, now_ms())
        .await
        .unwrap();
    assert!(!fx.db.pilot_email_verified(&personal_pid).await);
    let (st, _, body_still) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        Some(&acct_cookie),
        Some(&dev_tok),
        Some(json!({ "tail": "CC-PUB1", "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{body_still}");
    assert_eq!(body_still["error"], "email_not_verified");

    // 2. Omitting visibility on personal start defaults safely to private (never 403).
    let (st, _, def_body) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        Some(&acct_cookie),
        Some(&dev_tok),
        Some(json!({ "tail": "CC-DEF1" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{def_body}");
    assert_eq!(def_body["visibility"], "private");
    let def_fid = def_body["id"].as_str().unwrap().to_string();
    let (st, _, _) = send_json(
        &mut fx.router,
        "POST",
        &format!("/api/flights/{def_fid}/end"),
        None,
        Some(&dev_tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 3. Explicit private start allowed while email is unverified.
    let (st, _, start_body) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        Some(&acct_cookie),
        Some(&dev_tok),
        Some(json!({ "tail": "cc-priv1", "visibility": "private" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{start_body}");
    let flight_id = start_body["id"].as_str().unwrap().to_string();
    assert_eq!(start_body["visibility"], "private");

    // 4. PATCH private -> public blocked while unverified (main's email_not_verified contract).
    let (st, _, patch_body) = send_json(
        &mut fx.router,
        "PATCH",
        &format!("/api/flights/{flight_id}"),
        None,
        Some(&dev_tok),
        Some(json!({ "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{patch_body}");
    assert_eq!(patch_body["error"], "email_not_verified");
    assert_eq!(
        patch_body["message"],
        "Verify your email to share flights publicly."
    );
    assert!(patch_body.get("reason").is_none());

    // 5. Ownership check runs BEFORE email_not_verified on PATCH (no cross-pilot oracle).
    let other_email = format!("cl-oth-{}@test.example", &new_id()[..8]);
    let (other_acct, _) = create_account_with_session(&fx.db, &other_email, true, None).await;
    let other_pid = fx
        .db
        .ensure_personal_pilot(&other_acct, now_ms())
        .await
        .unwrap();
    let (other_dev, _) = fx.db.create_device(&other_pid, "phone").await.unwrap();
    let other_fid = fx
        .db
        .start_personal_flight_atomic(
            &other_pid,
            &other_acct,
            &other_dev,
            "CC-OTH1",
            FlightVisibility::Private,
            false,
            None,
            now_ms(),
        )
        .await
        .unwrap();
    let (st, _, cross_patch) = send_json(
        &mut fx.router,
        "PATCH",
        &format!("/api/flights/{other_fid}"),
        Some(&acct_cookie),
        None,
        Some(json!({ "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{cross_patch}");
    assert_eq!(cross_patch["error"], "forbidden");

    // 6. Verify the account email -> /api/account/me flips true & PATCH private -> public succeeds.
    sqlx::query("UPDATE accounts SET email_verified_at = $1 WHERE id = $2")
        .bind(now_ms())
        .bind(&acct_id)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    assert!(fx.db.pilot_email_verified(&personal_pid).await);

    let (st, _, me_ver) = send_json(
        &mut fx.router,
        "GET",
        "/api/account/me",
        Some(&acct_cookie),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{me_ver}");
    assert_eq!(me_ver["email_verified"], json!(true));
    assert!(me_ver["account"]["emailVerifiedAt"].is_number());
    assert!(me_ver["account"].get("emailVerified").is_none());

    let (st, _, patch_ok) = send_json(
        &mut fx.router,
        "PATCH",
        &format!("/api/flights/{flight_id}"),
        None,
        Some(&dev_tok),
        Some(json!({ "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{patch_ok}");

    // 7. /api/me/flights lists the club-less flight with its normalized free-text tail.
    let (st, _, list_body) = send_json(
        &mut fx.router,
        "GET",
        "/api/me/flights",
        None,
        Some(&dev_tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{list_body}");
    let flights = list_body["flights"].as_array().unwrap();
    assert_eq!(flights.len(), 2);
    let active = flights
        .iter()
        .find(|f| f["id"] == flight_id)
        .expect("active flight present");
    assert_eq!(active["tail"], "CC-PRIV1");
    assert_eq!(active["visibility"], "public");

    // 8. End the flight via bearer token, then start a new public flight directly (now verified).
    let (st, _, end_body) = send_json(
        &mut fx.router,
        "POST",
        &format!("/api/flights/{flight_id}/end"),
        None,
        Some(&dev_tok),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{end_body}");

    let (st, _, pub_start) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        None,
        Some(&dev_tok),
        Some(json!({ "tail": "N999PV", "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{pub_start}");
    assert_eq!(pub_start["visibility"], "public");
}

// ---------------------------------------------------------------------------
// 8(c) Visibility 'club' rejected (400) on start and PATCH + DB CHECK enforces invariants
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn clubless_visibility_club_rejected_http_and_db_checks() {
    let mut fx = setup_harness().await;
    let email = format!("cl-vis-{}@test.example", &new_id()[..8]);
    let (acct_id, acct_cookie) = create_account_with_session(&fx.db, &email, true, None).await;
    let personal_pid = fx
        .db
        .ensure_personal_pilot(&acct_id, now_ms())
        .await
        .unwrap();
    let (_dev_id, dev_tok) = fx.db.create_device(&personal_pid, "phone").await.unwrap();

    // 1. Start with visibility=club -> 400 club_visibility_requires_club.
    let (st, _, body) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        Some(&acct_cookie),
        Some(&dev_tok),
        Some(json!({ "tail": "CC-CLB", "visibility": "club" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["error"], "club_visibility_requires_club");

    // 2. Invalid tail_text rejected (400) via normalize_free_tail.
    assert!(normalize_free_tail("").is_err());
    assert!(normalize_free_tail("---").is_err());
    assert!(normalize_free_tail("TOO-LONG-TAIL-12345").is_err());
    assert!(normalize_free_tail("CC ABC").is_err());
    assert_eq!(normalize_free_tail("  cc-a1 ").unwrap(), "CC-A1");

    let (st, _, bad_tail) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        None,
        Some(&dev_tok),
        Some(json!({ "tail": "BAD TAIL!", "visibility": "private" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{bad_tail}");
    assert_eq!(bad_tail["error"], "invalid tail");

    // 3. Start private, then PATCH to visibility=club -> 400 club_visibility_requires_club.
    let (st, _, ok_start) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        None,
        Some(&dev_tok),
        Some(json!({ "tail": "CC-OK1", "visibility": "private" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{ok_start}");
    let fid = ok_start["id"].as_str().unwrap();

    let (st, _, patch_club) = send_json(
        &mut fx.router,
        "PATCH",
        &format!("/api/flights/{fid}"),
        Some(&acct_cookie),
        None,
        Some(json!({ "visibility": "club" })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{patch_club}");
    assert_eq!(patch_club["error"], "club_visibility_requires_club");

    // 4. DB CHECK `flights_club_scope_check` rejects a direct INSERT of a club-less 'club' flight.
    let res = sqlx::query(
        "INSERT INTO flights (id, pilot_id, aircraft_id, club_id, tail_text, owner_account_id, status, started_at, visibility)
         VALUES ($1, $2, NULL, NULL, 'CC-DB1', $3, 'active', $4, 'club')",
    )
    .bind(new_id())
    .bind(&personal_pid)
    .bind(&acct_id)
    .bind(now_ms())
    .execute(&fx.db.pool)
    .await;
    assert!(
        res.is_err(),
        "DB CHECK must reject club-less flight with visibility='club'"
    );

    // 5. DB CHECK `flights_tail_text_format_check` rejects lowercase / bad tail_text.
    let res_bad_tail = sqlx::query(
        "INSERT INTO flights (id, pilot_id, aircraft_id, club_id, tail_text, owner_account_id, status, started_at, visibility)
         VALUES ($1, $2, NULL, NULL, 'cc-bad', $3, 'active', $4, 'private')",
    )
    .bind(new_id())
    .bind(&personal_pid)
    .bind(&acct_id)
    .bind(now_ms())
    .execute(&fx.db.pool)
    .await;
    assert!(
        res_bad_tail.is_err(),
        "DB CHECK must reject malformed tail_text"
    );

    // 6. DB CHECK `pilots_personal_requires_account` rejects personal pilot without account_id.
    let res_no_acct = sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, NULL, NULL, 'Ghost', 'ghost@test', '', 'personal', 'pilot', '', $2)",
    )
    .bind(new_id())
    .bind(now_ms())
    .execute(&fx.db.pool)
    .await;
    assert!(
        res_no_acct.is_err(),
        "DB CHECK must reject club_id NULL with account_id NULL"
    );
}

// ---------------------------------------------------------------------------
// 8(d) tail_text never sets aircraft_id even when matching a club aircraft tail
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn clubless_tail_text_never_links_club_aircraft() {
    let mut fx = setup_harness().await;
    let (club_id, _) = create_live_club(
        &fx.db,
        "tailiso",
        &format!("owner-{}@test.example", &new_id()[..6]),
        "SCTB",
        -33.456,
        -70.548,
    )
    .await;
    fx.db.upsert_type(&club_id, "C172").await.unwrap();
    fx.db
        .upsert_aircraft(&club_id, "CC-CLB9", "C172", true)
        .await
        .unwrap();

    let email = format!("cl-tail-{}@test.example", &new_id()[..8]);
    let (acct_id, _) = create_account_with_session(&fx.db, &email, true, None).await;
    let personal_pid = fx
        .db
        .ensure_personal_pilot(&acct_id, now_ms())
        .await
        .unwrap();
    let (_dev_id, dev_tok) = fx.db.create_device(&personal_pid, "phone").await.unwrap();

    let (st, _, start_body) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        None,
        Some(&dev_tok),
        Some(json!({ "tail": "cc-clb9", "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{start_body}");
    let fid = start_body["id"].as_str().unwrap();

    let row = sqlx::query(
        "SELECT club_id, aircraft_id, tail_text, owner_account_id FROM flights WHERE id = $1",
    )
    .bind(fid)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    use sqlx::Row;
    let db_club_id: Option<String> = row.try_get(0).unwrap();
    let db_aircraft_id: Option<String> = row.try_get(1).unwrap();
    let db_tail_text: Option<String> = row.try_get(2).unwrap();
    let db_owner_acct: Option<String> = row.try_get(3).unwrap();

    assert!(db_club_id.is_none(), "club_id must stay NULL");
    assert!(db_aircraft_id.is_none(), "aircraft_id must stay NULL");
    assert_eq!(db_tail_text.as_deref(), Some("CC-CLB9"));
    assert_eq!(db_owner_acct.as_deref(), Some(acct_id.as_str()));
}

// ---------------------------------------------------------------------------
// 8(e) Multi-club check-in + switch-club + start B flight + non-member C rejected
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn multi_club_checkin_switches_to_target_club_and_rejects_non_member() {
    let mut fx = setup_harness().await;
    let (club_a, _slug_a) = create_live_club(
        &fx.db,
        "mca",
        &format!("own-a-{}@test.example", &new_id()[..6]),
        "SCTB",
        -33.456,
        -70.548,
    )
    .await;
    let (club_b, slug_b) = create_live_club(
        &fx.db,
        "mcb",
        &format!("own-b-{}@test.example", &new_id()[..6]),
        "SCEL",
        -33.393,
        -70.785,
    )
    .await;
    let (club_c, _slug_c) = create_live_club(
        &fx.db,
        "mcc",
        &format!("own-c-{}@test.example", &new_id()[..6]),
        "SCVM",
        -32.95,
        -71.48,
    )
    .await;

    fx.db.upsert_type(&club_b, "PA28").await.unwrap();
    fx.db
        .upsert_aircraft(&club_b, "CC-MCB1", "PA28", true)
        .await
        .unwrap();

    let email = format!("mc-pilot-{}@test.example", &new_id()[..8]);
    let (acct_id, acct_cookie) = create_account_with_session(&fx.db, &email, true, None).await;
    let (pilot_a, mem_a) =
        add_active_club_membership(&fx.db, &acct_id, &email, "Multi Pilot", &club_a, "pilot").await;
    let (pilot_b, mem_b) =
        add_active_club_membership(&fx.db, &acct_id, &email, "Multi Pilot", &club_b, "pilot").await;
    fx.db
        .add_checkout(&pilot_b, "PA28", None, None)
        .await
        .unwrap();

    // Activate Club A membership via /api/account/switch-club (app JSON, no Origin).
    let (st, _, sw_a) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/switch-club",
        Some(&acct_cookie),
        None,
        Some(json!({ "membershipId": mem_a })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{sw_a}");
    assert_eq!(sw_a["clubId"], club_a);
    assert_eq!(sw_a["activeMembershipId"], mem_a);

    // Mint a Club A device with installId.
    let shared_install = format!("inst-mc-{}", &new_id()[..10]);
    let dev_a = fx
        .db
        .register_device_for_install(&pilot_a, "Phone", Some("Android"), &shared_install)
        .await
        .unwrap();

    // 1. Logged in on Club A + Club A bearer -> scan Club B's QR -> 200, check-in recorded for pilot_b,
    //    new device minted for pilot_b without rebinding dev_a!
    let now = now_ms();
    let qr_b = qr_token_for_club(&fx.db, &club_b, now).await;
    let (st, _, ci_b) = send_json(
        &mut fx.router,
        "POST",
        "/api/checkin",
        Some(&acct_cookie),
        Some(&dev_a.token),
        Some(json!({ "token": qr_b })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{ci_b}");
    assert_eq!(ci_b["pilotId"], pilot_b);
    assert_eq!(ci_b["clubId"], club_b);
    let dev_b_id = ci_b["deviceId"]
        .as_str()
        .expect("club B device minted")
        .to_string();
    let dev_b_tok = ci_b["token"]
        .as_str()
        .expect("club B device token returned")
        .to_string();
    assert_ne!(dev_b_id, dev_a.id, "must NOT rebind Club A's device");
    assert_eq!(
        fx.db.device_pilot_id(&dev_a.id).await.as_deref(),
        Some(pilot_a.as_str()),
        "Club A device still belongs to pilot_a"
    );
    assert_eq!(
        fx.db.device_pilot_id(&dev_b_id).await.as_deref(),
        Some(pilot_b.as_str()),
        "Club B device belongs to pilot_b"
    );

    // Verify DB check-in row belongs to pilot_b in club_b.
    let ci_count_b: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM checkins WHERE club_id = $1 AND pilot_id = $2 AND outcome = 'ok'",
    )
    .bind(&club_b)
    .bind(&pilot_b)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(ci_count_b, 1);

    // 2. Switch active club to B via /api/account/switch-club (JSON + cookie, no Origin) ->
    //    returns activeMembershipId and club metadata.
    let (st, _, sw_b) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/switch-club",
        Some(&acct_cookie),
        None,
        Some(json!({ "membershipId": mem_b })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{sw_b}");
    assert_eq!(sw_b["activeMembershipId"], mem_b);
    assert_eq!(sw_b["club"]["id"], club_b);
    assert_eq!(sw_b["club"]["slug"], slug_b);

    // 3. Start a Club B flight with Club B tail using Club B device -> 200 OK!
    let (st, _, start_b) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        Some(&acct_cookie),
        Some(&dev_b_tok),
        Some(json!({ "tail": "CC-MCB1", "visibility": "club" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{start_b}");
    let fid_b = start_b["id"].as_str().unwrap();
    assert_eq!(
        fx.db.club_id_for_flight(fid_b).await.as_deref(),
        Some(club_b.as_str())
    );

    // 4. Non-member Club C QR -> 403 club_mismatch, no membership created.
    let qr_c = qr_token_for_club(&fx.db, &club_c, now).await;
    let (st, _, ci_c) = send_json(
        &mut fx.router,
        "POST",
        "/api/checkin",
        Some(&acct_cookie),
        Some(&dev_a.token),
        Some(json!({ "token": qr_c })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{ci_c}");
    assert_eq!(ci_c["reason"], "club_mismatch");

    let mem_c_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM club_memberships m
         JOIN pilots p ON p.id = m.pilot_id
         WHERE p.account_id = $1 AND m.club_id = $2",
    )
    .bind(&acct_id)
    .bind(&club_c)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(mem_c_count, 0);
}

// ---------------------------------------------------------------------------
// 8(f) Club member flies public without check-in (mode=personal) vs existing NoCheckin without mode
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn club_member_mode_personal_without_checkin_vs_club_requires_checkin() {
    let mut fx = setup_harness().await;
    let (club_a, _) = create_live_club(
        &fx.db,
        "memp",
        &format!("own-mp-{}@test.example", &new_id()[..6]),
        "SCTB",
        -33.456,
        -70.548,
    )
    .await;
    fx.db.upsert_type(&club_a, "C152").await.unwrap();
    fx.db
        .upsert_aircraft(&club_a, "CC-MEM1", "C152", true)
        .await
        .unwrap();

    let email = format!("memp-{}@test.example", &new_id()[..8]);
    let (acct_id, acct_cookie) = create_account_with_session(&fx.db, &email, true, None).await;
    let (club_pid, mem_id) =
        add_active_club_membership(&fx.db, &acct_id, &email, "Club Pilot", &club_a, "pilot").await;
    fx.db
        .add_checkout(&club_pid, "C152", None, None)
        .await
        .unwrap();

    // Activate Club A session.
    let (st, _, _) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/switch-club",
        Some(&acct_cookie),
        None,
        Some(json!({ "membershipId": mem_id })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Mint a club device (active membership) AND a personal device (mode=personal).
    let (st, _, club_dev) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/devices",
        Some(&acct_cookie),
        None,
        Some(json!({ "label": "Club Phone" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{club_dev}");
    let club_tok = club_dev["token"].as_str().unwrap().to_string();

    let (st, _, pers_dev) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/devices",
        Some(&acct_cookie),
        None,
        Some(json!({ "label": "Personal Phone", "mode": "personal" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{pers_dev}");
    let pers_tok = pers_dev["token"].as_str().unwrap().to_string();

    // 1. Without mode=personal and no check-in -> existing NoCheckin error (403 check-in required).
    let (st, _, no_ci) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        Some(&acct_cookie),
        Some(&club_tok),
        Some(json!({ "tail": "CC-MEM1", "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{no_ci}");
    assert_eq!(no_ci["error"], "check-in required");

    // 2. With mode=personal and personal device (NO check-in!) -> 200, club_id IS NULL!
    let (st, _, pers_flight) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        Some(&acct_cookie),
        Some(&pers_tok),
        Some(json!({ "tail": "N777ME", "visibility": "public", "mode": "personal" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{pers_flight}");
    let fid = pers_flight["id"].as_str().unwrap();
    let row = fx.db.get_flight(fid).await.expect("flight exists");
    assert!(row.club_id.is_none());
    assert!(row.aircraft_id.is_none());
    assert_eq!(row.tail, "N777ME");
    assert_eq!(row.visibility, "public");
}

// ---------------------------------------------------------------------------
// 8(g) Ingest rate limit (both /api/ingest and /api/osmand): 429 without ending flight,
//      accepted again after window elapses (injected clock)
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn ingest_and_osmand_per_device_rate_limit_preserves_flight_and_recovers() {
    let mut fx = setup_harness().await;
    let t0 = fx.clock_ms.load(Ordering::SeqCst);

    let email = format!("cl-rl-{}@test.example", &new_id()[..8]);
    let (acct_id, _) = create_account_with_session(&fx.db, &email, true, None).await;
    let personal_pid = fx.db.ensure_personal_pilot(&acct_id, t0).await.unwrap();
    let (dev_id, dev_tok) = fx.db.create_device(&personal_pid, "tracker").await.unwrap();

    // Start a club-less flight.
    let (st, _, start) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        None,
        Some(&dev_tok),
        Some(json!({ "tail": "CC-RL01", "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{start}");
    let flight_id = start["id"].as_str().unwrap().to_string();

    // 1. A single batch > MAX_CLUB_INGEST_FIXES_PER_MINUTE (120) is rejected atomically (429)
    //    with Retry-After: 60 (full window) and unchanged error body.
    let oversized: Vec<Value> = (0..=MAX_CLUB_INGEST_FIXES_PER_MINUTE)
        .map(|i| json!({ "t": t0 + i as i64 * 100, "lat": -33.45, "lon": -70.55, "alt": 900.0 }))
        .collect();
    let (st, hdrs_over, body) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&dev_tok),
        Some(json!({ "fixes": oversized })),
    )
    .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"], "rate limited");
    let retry_over: u64 = hdrs_over
        .get(header::RETRY_AFTER)
        .expect("Retry-After header required on oversized batch 429")
        .to_str()
        .unwrap()
        .parse()
        .expect("Retry-After must be integer seconds");
    assert_eq!(retry_over, 60);

    // 2. Fill the 120-fix budget across two batches (100 at t0, 20 at t0 + 15s) -> both 200 OK
    //    with no Retry-After header.
    let batch100: Vec<Value> = (0..100)
        .map(|i| json!({ "t": t0 + i as i64 * 500, "lat": -33.45, "lon": -70.55, "alt": 900.0 }))
        .collect();
    let (st, hdrs_b1, b1) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&dev_tok),
        Some(json!({ "fixes": batch100 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b1}");
    assert_eq!(b1["accepted"], 100);
    assert!(
        hdrs_b1.get(header::RETRY_AFTER).is_none(),
        "200 /api/ingest must not carry Retry-After"
    );

    let t_b2 = t0 + 15_000;
    fx.clock_ms.store(t_b2, Ordering::SeqCst);
    let batch20: Vec<Value> = (100..MAX_CLUB_INGEST_FIXES_PER_MINUTE)
        .map(|i| json!({ "t": t0 + i as i64 * 500, "lat": -33.45, "lon": -70.55, "alt": 910.0 }))
        .collect();
    let (st, hdrs_b2, b2) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&dev_tok),
        Some(json!({ "fixes": batch20 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b2}");
    assert_eq!(b2["accepted"], 20);
    assert!(
        hdrs_b2.get(header::RETRY_AFTER).is_none(),
        "200 /api/ingest must not carry Retry-After"
    );

    // 3. 121st fix via /api/ingest at t0 + 15s -> 429 with Retry-After = 45 (time until t0 batch expires).
    let (st, hdrs_b3, b3) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&dev_tok),
        Some(
            json!({ "fixes": [{ "t": t0 + 90_000, "lat": -33.45, "lon": -70.55, "alt": 920.0 }] }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{b3}");
    assert_eq!(b3["error"], "rate limited");
    let retry_b3: u64 = hdrs_b3
        .get(header::RETRY_AFTER)
        .expect("Retry-After header required on /api/ingest 429")
        .to_str()
        .unwrap()
        .parse()
        .expect("Retry-After must be integer seconds");
    assert!((1..=60).contains(&retry_b3));
    assert_eq!(retry_b3, 45);

    // A 101-fix batch at t0 + 15s cannot fit when only the 100 t0 hits expire; it must wait
    // until a hit from the t0 + 15s batch expires (60s).
    let batch101: Vec<Value> = (0..101)
        .map(
            |i| json!({ "t": t0 + 100_000 + i as i64, "lat": -33.45, "lon": -70.55, "alt": 920.0 }),
        )
        .collect();
    let (st, hdrs_101, b101) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&dev_tok),
        Some(json!({ "fixes": batch101 })),
    )
    .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{b101}");
    assert_eq!(b101["error"], "rate limited");
    let retry_101: u64 = hdrs_101
        .get(header::RETRY_AFTER)
        .expect("Retry-After header required on /api/ingest 429")
        .to_str()
        .unwrap()
        .parse()
        .expect("Retry-After must be integer seconds");
    assert_eq!(retry_101, 60);

    // 4. Advance clock by 20.5s (t0 + 35.5s): Retry-After shrinks and rounds UP (24.5s -> 25s)
    //    on both /api/ingest and /api/osmand.
    fx.clock_ms.store(t0 + 35_500, Ordering::SeqCst);
    let (st, hdrs_b3_later, b3_later) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&dev_tok),
        Some(
            json!({ "fixes": [{ "t": t0 + 90_500, "lat": -33.45, "lon": -70.55, "alt": 920.0 }] }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{b3_later}");
    let retry_b3_later: u64 = hdrs_b3_later
        .get(header::RETRY_AFTER)
        .expect("Retry-After header required on /api/ingest 429")
        .to_str()
        .unwrap()
        .parse()
        .expect("Retry-After must be integer seconds");
    assert_eq!(retry_b3_later, 25);
    assert!(retry_b3_later < retry_b3);

    // Same device via /api/osmand also shares the budget -> 429 rate limited with Retry-After: 25.
    let osmand_req = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/osmand?id={dev_tok}&lat=-33.45&lon=-70.55&altitude=920&timestamp={}",
            (t0 / 1000) + 91
        ))
        .body(Body::empty())
        .unwrap();
    let osmand_res = fx.router.call(osmand_req).await.unwrap();
    assert_eq!(osmand_res.status(), StatusCode::TOO_MANY_REQUESTS);
    let osmand_retry: u64 = osmand_res
        .headers()
        .get(header::RETRY_AFTER)
        .expect("Retry-After header required on /api/osmand 429")
        .to_str()
        .unwrap()
        .parse()
        .expect("Retry-After must be integer seconds");
    assert_eq!(osmand_retry, 25);
    let osmand_body: Value =
        serde_json::from_slice(&to_bytes(osmand_res.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(osmand_body["error"], "rate limited");

    // 5. Flight is STILL active (rate limit never kills the flight).
    let open = fx.db.open_flight_for_device(&dev_id).await;
    assert_eq!(open.as_deref(), Some(flight_id.as_str()));
    let frow = fx.db.get_flight(&flight_id).await.unwrap();
    assert_eq!(frow.status, "active");
    assert!(frow.ended_at.is_none());

    // 6. Advance injected clock past CLUB_INGEST_WINDOW_MS -> both /api/ingest and /api/osmand succeed
    //    with no Retry-After header!
    let t_next = t0 + CLUB_INGEST_WINDOW_MS + 1_000;
    fx.clock_ms.store(t_next, Ordering::SeqCst);

    let (st, hdrs_b4, b4) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&dev_tok),
        Some(json!({ "fixes": [{ "t": t_next, "lat": -33.46, "lon": -70.56, "alt": 950.0 }] })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b4}");
    assert_eq!(b4["accepted"], 1);
    assert!(
        hdrs_b4.get(header::RETRY_AFTER).is_none(),
        "200 /api/ingest must not carry Retry-After"
    );

    let osmand_ok_req = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/osmand?id={dev_tok}&lat=-33.47&lon=-70.57&altitude=960&timestamp={}",
            (t_next / 1000) + 1
        ))
        .body(Body::empty())
        .unwrap();
    let osmand_ok_res = fx.router.call(osmand_ok_req).await.unwrap();
    assert_eq!(osmand_ok_res.status(), StatusCode::OK);
    assert!(
        osmand_ok_res.headers().get(header::RETRY_AFTER).is_none(),
        "200 /api/osmand must not carry Retry-After"
    );

    // 7. Club flights share the exact same per-device ingest rate limit and stay active on 429.
    let (club_id, _) = create_live_club(
        &fx.db,
        "rlclub",
        &format!("own-rlc-{}@test.example", &new_id()[..6]),
        "SCTB",
        -33.456,
        -70.548,
    )
    .await;
    fx.db.upsert_type(&club_id, "C152").await.unwrap();
    fx.db
        .upsert_aircraft(&club_id, "CC-RLC1", "C152", true)
        .await
        .unwrap();
    let c_email = format!("rlc-{}@test.example", &new_id()[..8]);
    let (c_acct, _) = create_account_with_session(&fx.db, &c_email, true, None).await;
    let (c_pid, _) =
        add_active_club_membership(&fx.db, &c_acct, &c_email, "RL Club", &club_id, "pilot").await;
    fx.db
        .add_checkout(&c_pid, "C152", None, None)
        .await
        .unwrap();
    let (c_dev, c_tok) = fx.db.create_device(&c_pid, "club-dev").await.unwrap();
    fx.db
        .issue_open_checkin(&c_pid, &c_dev, &club_id, t_next)
        .await
        .unwrap();
    let c_fid = fx
        .db
        .start_flight(
            &c_pid,
            &c_dev,
            "CC-RLC1",
            FlightVisibility::Club,
            false,
            t_next,
        )
        .await
        .unwrap();

    let club_batch: Vec<Value> = (0..MAX_CLUB_INGEST_FIXES_PER_MINUTE)
        .map(
            |i| json!({ "t": t_next + i as i64 * 100, "lat": -33.45, "lon": -70.55, "alt": 900.0 }),
        )
        .collect();
    let (st, hdrs_cb1, cb1) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&c_tok),
        Some(json!({ "fixes": club_batch })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{cb1}");
    assert_eq!(cb1["accepted"], MAX_CLUB_INGEST_FIXES_PER_MINUTE);
    assert!(hdrs_cb1.get(header::RETRY_AFTER).is_none());

    let (st, hdrs_cb2, cb2) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&c_tok),
        Some(json!({ "fixes": [{ "t": t_next + 20_000, "lat": -33.45, "lon": -70.55, "alt": 910.0 }] })),
    )
    .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS, "{cb2}");
    assert_eq!(cb2["error"], "rate limited");
    let retry_cb2: u64 = hdrs_cb2
        .get(header::RETRY_AFTER)
        .expect("Retry-After header required on club flight 429")
        .to_str()
        .unwrap()
        .parse()
        .expect("Retry-After must be integer seconds");
    assert_eq!(retry_cb2, 60);
    assert_eq!(
        fx.db.open_flight_for_device(&c_dev).await.as_deref(),
        Some(c_fid.as_str()),
        "club flight must stay active after 429"
    );
}

// ---------------------------------------------------------------------------
// 8(h) Club-less isolation: denied on all club/admin/kiosk endpoints; /api/me/live-trails
//      returns zero club flights and area-bounded public layer only; readers don't 500 or leak
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn clubless_pilot_denied_on_club_admin_kiosk_and_isolated_on_live_trails() {
    let mut fx = setup_harness().await;
    let now = now_ms();

    // Seed a live club with an admin, a club-only flight, and a public flight near SCTB.
    let (club_id, club_slug) = create_live_club(
        &fx.db,
        "iso",
        &format!("own-iso-{}@test.example", &new_id()[..6]),
        "SCTB",
        -33.456,
        -70.548,
    )
    .await;
    fx.db.upsert_type(&club_id, "C172").await.unwrap();
    fx.db
        .upsert_aircraft(&club_id, "CC-ISO1", "C172", true)
        .await
        .unwrap();
    fx.db
        .upsert_aircraft(&club_id, "CC-ISO2", "C172", true)
        .await
        .unwrap();

    let (admin_acct, admin_cookie) = create_account_with_session(
        &fx.db,
        &format!("adm-iso-{}@test.example", &new_id()[..6]),
        true,
        None,
    )
    .await;
    let (admin_pid, admin_mem) = add_active_club_membership(
        &fx.db,
        &admin_acct,
        &format!("adm-iso-{}@test.example", &new_id()[..6]),
        "Admin",
        &club_id,
        "admin",
    )
    .await;
    let (st, _, _) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/switch-club",
        Some(&admin_cookie),
        None,
        Some(json!({ "membershipId": admin_mem })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Seed two active club flights (one 'club', one 'public') in Club ISO.
    let (m1_acct, _) = create_account_with_session(
        &fx.db,
        &format!("m1-{}@test.example", &new_id()[..6]),
        true,
        None,
    )
    .await;
    let (m1_pid, _) = add_active_club_membership(
        &fx.db,
        &m1_acct,
        &format!("m1-{}@test.example", &new_id()[..6]),
        "M1",
        &club_id,
        "pilot",
    )
    .await;
    fx.db
        .add_checkout(&m1_pid, "C172", None, None)
        .await
        .unwrap();
    let (m1_dev, _) = fx.db.create_device(&m1_pid, "p1").await.unwrap();
    fx.db
        .issue_open_checkin(&m1_pid, &m1_dev, &club_id, now)
        .await
        .unwrap();
    let fid_club_only = fx
        .db
        .start_flight(
            &m1_pid,
            &m1_dev,
            "CC-ISO1",
            FlightVisibility::Club,
            false,
            now - 5_000,
        )
        .await
        .unwrap();
    fx.db
        .append_fix(
            &fid_club_only,
            &m1_dev,
            &LiveFix {
                t: now - 1_000,
                lat: -33.455,
                lon: -70.547,
                alt: Some(1200.0),
                alt_ref: Some("wgs84_ellipsoid".to_string()),
                speed: Some(35.0),
                course: Some(90.0),
                accuracy: Some(5.0),
            },
        )
        .await
        .unwrap();

    let (m2_acct, _) = create_account_with_session(
        &fx.db,
        &format!("m2-{}@test.example", &new_id()[..6]),
        true,
        None,
    )
    .await;
    let (m2_pid, _) = add_active_club_membership(
        &fx.db,
        &m2_acct,
        &format!("m2-{}@test.example", &new_id()[..6]),
        "M2",
        &club_id,
        "pilot",
    )
    .await;
    fx.db
        .add_checkout(&m2_pid, "C172", None, None)
        .await
        .unwrap();
    let (m2_dev, _) = fx.db.create_device(&m2_pid, "p2").await.unwrap();
    fx.db
        .issue_open_checkin(&m2_pid, &m2_dev, &club_id, now)
        .await
        .unwrap();
    let fid_club_pub = fx
        .db
        .start_flight(
            &m2_pid,
            &m2_dev,
            "CC-ISO2",
            FlightVisibility::Public,
            false,
            now - 5_000,
        )
        .await
        .unwrap();
    fx.db
        .append_fix(
            &fid_club_pub,
            &m2_dev,
            &LiveFix {
                t: now - 1_000,
                lat: -33.457,
                lon: -70.549,
                alt: Some(1200.0),
                alt_ref: Some("wgs84_ellipsoid".to_string()),
                speed: Some(35.0),
                course: Some(90.0),
                accuracy: Some(5.0),
            },
        )
        .await
        .unwrap();

    // Create club-less pilot (account session with no memberships + personal device token)
    // and start an active club-less public flight with a live fix to verify existing readers
    // never panic, 500, or leak it in E1.
    let cl_email = format!("cl-iso-{}@test.example", &new_id()[..8]);
    let (cl_acct, cl_cookie) = create_account_with_session(&fx.db, &cl_email, true, None).await;
    let cl_pid = fx.db.ensure_personal_pilot(&cl_acct, now).await.unwrap();
    let (cl_dev_id, cl_tok) = fx.db.create_device(&cl_pid, "phone").await.unwrap();
    let cl_fid = fx
        .db
        .start_personal_flight_atomic(
            &cl_pid,
            &cl_acct,
            &cl_dev_id,
            "CC-FREE1",
            FlightVisibility::Public,
            false,
            None,
            now - 5_000,
        )
        .await
        .unwrap();
    fx.db
        .append_fix(
            &cl_fid,
            &cl_dev_id,
            &LiveFix {
                t: now - 1_000,
                lat: -33.456,
                lon: -70.548,
                alt: Some(1200.0),
                alt_ref: Some("wgs84_ellipsoid".to_string()),
                speed: Some(30.0),
                course: Some(180.0),
                accuracy: Some(5.0),
            },
        )
        .await
        .unwrap();

    // 1. Club-less pilot (tested with BOTH account cookie AND personal bearer token) is denied
    //    on every club/admin/kiosk/access-request/checkout/settings endpoint.
    let denied_cases: Vec<(&str, &str, Option<Value>)> = vec![
        ("GET", "/api/admin/map", None),
        ("GET", "/api/admin/map/trails", None),
        ("GET", "/api/admin/pilots", None),
        ("GET", "/api/admin/pilots/pending", None),
        ("GET", "/api/admin/memberships", None),
        ("GET", "/api/display/map", None),
        ("GET", "/api/display/map/trails", None),
        ("GET", "/api/display/checkin-qr", None),
        ("GET", "/api/display/base", None),
        ("GET", "/api/club/map", None),
        ("GET", "/api/me/access-requests", None),
        (
            "POST",
            "/api/me/access-requests",
            Some(json!({ "tail": "CC-ISO1" })),
        ),
        ("GET", "/api/admin/checkouts", None),
        (
            "POST",
            "/api/admin/checkouts",
            Some(json!({ "pilot_id": admin_pid, "type": "C172" })),
        ),
        ("GET", "/api/admin/club/base", None),
        (
            "PATCH",
            "/api/admin/club/base",
            Some(json!({ "icao": "SCTB", "lat": -33.456, "lon": -70.548 })),
        ),
        ("GET", "/api/admin/club/trail", None),
        (
            "PATCH",
            "/api/admin/club/trail",
            Some(json!({ "trailMinutes": 10 })),
        ),
        ("GET", "/api/admin/club/nearby-public", None),
        (
            "PATCH",
            "/api/admin/club/nearby-public",
            Some(json!({ "showNearbyPublic": false })),
        ),
        (
            "PATCH",
            "/api/admin/club/locale",
            Some(json!({ "locale": "en" })),
        ),
        ("GET", "/api/me/tails", None),
    ];

    for (method, uri, payload) in denied_cases {
        let (st_cookie, _, body_c) = send_json(
            &mut fx.router,
            method,
            uri,
            Some(&cl_cookie),
            None,
            payload.clone(),
        )
        .await;
        assert!(
            matches!(st_cookie, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
            "cookie club-less caller must be denied on {method} {uri}, got {st_cookie} ({body_c})"
        );

        let (st_bearer, _, body_b) =
            send_json(&mut fx.router, method, uri, None, Some(&cl_tok), payload).await;
        assert!(
            matches!(st_bearer, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
            "bearer club-less caller must be denied on {method} {uri}, got {st_bearer} ({body_b})"
        );
    }

    // 2. /api/me/live-trails?layers=club returns ZERO club flights (never default club!).
    for (cookie, bearer) in [
        (Some(cl_cookie.as_str()), None),
        (None, Some(cl_tok.as_str())),
    ] {
        let (st, _, body) = send_json(
            &mut fx.router,
            "GET",
            "/api/me/live-trails?layers=club",
            cookie,
            bearer,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        assert_eq!(body["flights"].as_array().map(Vec::len), Some(0));

        // Public layer without lat/lon MUST fail with 400 (never a global feed!).
        let (st_no_loc, _, _) = send_json(
            &mut fx.router,
            "GET",
            "/api/me/live-trails?layers=public",
            cookie,
            bearer,
            None,
        )
        .await;
        assert_eq!(st_no_loc, StatusCode::BAD_REQUEST);

        // Public layer WITH lat/lon/radius_km returns ONLY nearby public flights, NEVER club-only!
        let (st_pub, _, body_pub) = send_json(
            &mut fx.router,
            "GET",
            "/api/me/live-trails?layers=club,public&lat=-33.456&lon=-70.548&radius_km=20",
            cookie,
            bearer,
            None,
        )
        .await;
        assert_eq!(st_pub, StatusCode::OK, "{body_pub}");
        let fids: HashSet<&str> = body_pub["flights"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f["flightId"].as_str())
            .collect();
        assert!(
            !fids.contains(fid_club_only.as_str()),
            "club-only must never leak"
        );
        assert!(
            fids.contains(fid_club_pub.as_str()),
            "nearby public club flight visible"
        );
        assert!(
            !fids.contains(cl_fid.as_str()),
            "own / club-less flight excluded in E1"
        );

        // Public layer far away (outside radius) returns ZERO flights (area-bounded!).
        let (st_far, _, body_far) = send_json(
            &mut fx.router,
            "GET",
            "/api/me/live-trails?layers=public&lat=0.0&lon=0.0&radius_km=20",
            cookie,
            bearer,
            None,
        )
        .await;
        assert_eq!(st_far, StatusCode::OK, "{body_far}");
        assert_eq!(body_far["flights"].as_array().map(Vec::len), Some(0));
    }

    // 3. Existing readers (club/admin/display/public maps) do not 500 or emit club-less flights in E1.
    for (uri, cookie) in [
        ("/api/club/map".to_string(), Some(admin_cookie.as_str())),
        ("/api/admin/map".to_string(), Some(admin_cookie.as_str())),
        (
            "/api/admin/map/trails".to_string(),
            Some(admin_cookie.as_str()),
        ),
        ("/api/display/map".to_string(), Some(admin_cookie.as_str())),
        (
            "/api/display/map/trails".to_string(),
            Some(admin_cookie.as_str()),
        ),
        (format!("/api/club/map/public?club={club_slug}"), None),
        (
            "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=20".to_string(),
            None,
        ),
    ] {
        let (st, _, body) = send_json(&mut fx.router, "GET", &uri, cookie, None, None).await;
        assert_eq!(st, StatusCode::OK, "{uri} failed: {body}");
        assert!(
            !body.to_string().contains(&cl_fid),
            "club-less flight must not appear on {uri} in E1: {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// 8(m) Anonymous /api/public/live-trails: never 500s on NULL club_id/aircraft_id,
//      never leaks club-less flights (including from unverified owners), keeps AGL floor
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn public_live_trails_excludes_clubless_and_unverified_without_500_and_keeps_agl_floor() {
    let mut fx = setup_harness().await;
    let now = now_ms();

    // Flat 0 m DEM covering SCTB (-33.45x, -70.548 -> tile N34W071) so AGL checks
    // resolve deterministically both with MSL fallback and under fail-closed DEM floors.
    let dem_root = tempfile::tempdir().unwrap();
    let dem_dir = dem_root.path().join("dem");
    std::fs::create_dir_all(&dem_dir).unwrap();
    std::fs::write(
        dem_dir.join("N34W071.asc"),
        "# 0 m flat terrain\n0.0 0.0 0.0\n0.0 0.0 0.0\n0.0 0.0 0.0\n",
    )
    .unwrap();
    let _env_guard = FlightDataEnvGuard::set(dem_root.path());

    let (club_id, _) = create_live_club(
        &fx.db,
        "pubcl",
        &format!("own-pubcl-{}@test.example", &new_id()[..6]),
        "SCTB",
        -33.456,
        -70.548,
    )
    .await;
    fx.db.upsert_type(&club_id, "C172").await.unwrap();
    fx.db
        .upsert_aircraft(&club_id, "CC-HI01", "C172", true)
        .await
        .unwrap();
    fx.db
        .upsert_aircraft(&club_id, "CC-LO01", "C172", true)
        .await
        .unwrap();

    // High-altitude club public flight (2500 m MSL ~ 8202 ft AGL over 0 m DEM -> passes default and 500 ft floors)
    let hi_email = format!("pubcl-hi-{}@test.example", &new_id()[..6]);
    let (hi_acct, _) = create_account_with_session(&fx.db, &hi_email, true, None).await;
    let (hi_pid, _) =
        add_active_club_membership(&fx.db, &hi_acct, &hi_email, "Hi Pilot", &club_id, "pilot")
            .await;
    fx.db
        .add_checkout(&hi_pid, "C172", None, None)
        .await
        .unwrap();
    let (hi_dev, _) = fx.db.create_device(&hi_pid, "hi-dev").await.unwrap();
    fx.db
        .issue_open_checkin(&hi_pid, &hi_dev, &club_id, now)
        .await
        .unwrap();
    let fid_club_high = fx
        .db
        .start_flight(
            &hi_pid,
            &hi_dev,
            "CC-HI01",
            FlightVisibility::Public,
            false,
            now - 5_000,
        )
        .await
        .unwrap();
    fx.db
        .append_fix(
            &fid_club_high,
            &hi_dev,
            &LiveFix {
                t: now - 1_000,
                lat: -33.456,
                lon: -70.548,
                alt: Some(2500.0),
                alt_ref: Some("msl".to_string()),
                speed: Some(40.0),
                course: Some(90.0),
                accuracy: Some(5.0),
            },
        )
        .await
        .unwrap();

    // Low-altitude club public flight (50 m MSL ~ 164 ft AGL over 0 m DEM -> below min_agl_ft=500 and any 300 ft default floor)
    let lo_email = format!("pubcl-lo-{}@test.example", &new_id()[..6]);
    let (lo_acct, _) = create_account_with_session(&fx.db, &lo_email, true, None).await;
    let (lo_pid, _) =
        add_active_club_membership(&fx.db, &lo_acct, &lo_email, "Lo Pilot", &club_id, "pilot")
            .await;
    fx.db
        .add_checkout(&lo_pid, "C172", None, None)
        .await
        .unwrap();
    let (lo_dev, _) = fx.db.create_device(&lo_pid, "lo-dev").await.unwrap();
    fx.db
        .issue_open_checkin(&lo_pid, &lo_dev, &club_id, now)
        .await
        .unwrap();
    let fid_club_low = fx
        .db
        .start_flight(
            &lo_pid,
            &lo_dev,
            "CC-LO01",
            FlightVisibility::Public,
            false,
            now - 5_000,
        )
        .await
        .unwrap();
    fx.db
        .append_fix(
            &fid_club_low,
            &lo_dev,
            &LiveFix {
                t: now - 1_000,
                lat: -33.457,
                lon: -70.548,
                alt: Some(50.0),
                alt_ref: Some("msl".to_string()),
                speed: Some(5.0),
                course: Some(90.0),
                accuracy: Some(5.0),
            },
        )
        .await
        .unwrap();

    // Public club-less flight from an UNVERIFIED personal pilot (direct DB seed to test read-side defense).
    let unv_email = format!("pubcl-unv-{}@test.example", &new_id()[..6]);
    let (unv_acct, unv_cookie) = create_account_with_session(&fx.db, &unv_email, false, None).await;
    let unv_pid = fx.db.ensure_personal_pilot(&unv_acct, now).await.unwrap();
    let (unv_dev, _) = fx.db.create_device(&unv_pid, "unv-dev").await.unwrap();
    let fid_unv_clubless = fx
        .db
        .start_personal_flight_atomic(
            &unv_pid,
            &unv_acct,
            &unv_dev,
            "CC-UNV1",
            FlightVisibility::Public,
            true,
            None,
            now - 5_000,
        )
        .await
        .unwrap();
    fx.db
        .append_fix(
            &fid_unv_clubless,
            &unv_dev,
            &LiveFix {
                t: now - 1_000,
                lat: -33.455,
                lon: -70.548,
                alt: Some(2500.0),
                alt_ref: Some("msl".to_string()),
                speed: Some(40.0),
                course: Some(180.0),
                accuracy: Some(5.0),
            },
        )
        .await
        .unwrap();

    // Public club-less flight from a VERIFIED personal pilot (still excluded from E1 public feed).
    let ver_email = format!("pubcl-ver-{}@test.example", &new_id()[..6]);
    let (ver_acct, _) = create_account_with_session(&fx.db, &ver_email, true, None).await;
    let ver_pid = fx.db.ensure_personal_pilot(&ver_acct, now).await.unwrap();
    let (ver_dev, _) = fx.db.create_device(&ver_pid, "ver-dev").await.unwrap();
    let fid_ver_clubless = fx
        .db
        .start_personal_flight_atomic(
            &ver_pid,
            &ver_acct,
            &ver_dev,
            "CC-VER1",
            FlightVisibility::Public,
            true,
            None,
            now - 5_000,
        )
        .await
        .unwrap();
    fx.db
        .append_fix(
            &fid_ver_clubless,
            &ver_dev,
            &LiveFix {
                t: now - 1_000,
                lat: -33.454,
                lon: -70.548,
                alt: Some(2500.0),
                alt_ref: Some("msl".to_string()),
                speed: Some(40.0),
                course: Some(180.0),
                accuracy: Some(5.0),
            },
        )
        .await
        .unwrap();

    // 1. Anonymous GET /api/public/live-trails (default altitude floor policy) -> 200 OK, no 500,
    //    returns the high-altitude club public flight; NEVER returns unverified or verified
    //    club-less rows. Presence of fid_club_low (~164 ft AGL) depends on the endpoint's default
    //    AGL floor (none vs. M-27's 300 ft default), so it is not asserted here.
    let (st, _, body) = send_json(
        &mut fx.router,
        "GET",
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=20",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let ids: HashSet<&str> = body["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(ids.contains(fid_club_high.as_str()));
    assert!(
        !ids.contains(fid_unv_clubless.as_str()),
        "unverified club-less flight must never appear on /api/public/live-trails"
    );
    assert!(
        !ids.contains(fid_ver_clubless.as_str()),
        "club-less flight must not appear on /api/public/live-trails in E1"
    );

    // 2. Anonymous GET /api/public/live-trails with min_agl_ft=500 keeps main's altitude floor
    //    (drops low-altitude club flight, keeps high-altitude club flight, excludes club-less).
    let (st_agl, _, body_agl) = send_json(
        &mut fx.router,
        "GET",
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=20&min_agl_ft=500",
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_agl, StatusCode::OK, "{body_agl}");
    let ids_agl: HashSet<&str> = body_agl["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(ids_agl.contains(fid_club_high.as_str()));
    assert!(
        !ids_agl.contains(fid_club_low.as_str()),
        "min_agl_ft floor must filter low-altitude flight"
    );
    assert!(!ids_agl.contains(fid_unv_clubless.as_str()));
    assert!(!ids_agl.contains(fid_ver_clubless.as_str()));

    // 3. Authenticated GET /api/me/live-trails?layers=public also never leaks unverified club-less flight.
    let (st_me, _, body_me) = send_json(
        &mut fx.router,
        "GET",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&radius_km=20",
        Some(&unv_cookie),
        None,
        None,
    )
    .await;
    assert_eq!(st_me, StatusCode::OK, "{body_me}");
    let ids_me: HashSet<&str> = body_me["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(!ids_me.contains(fid_unv_clubless.as_str()));
    assert!(!ids_me.contains(fid_ver_clubless.as_str()));
}

// ---------------------------------------------------------------------------
// 8(i) Device token cannot cross personal <-> club; same install_id creates separate rows
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn device_tokens_cannot_cross_personal_and_club_and_never_rebind() {
    let mut fx = setup_harness().await;
    let now = now_ms();
    let (club_id, _) = create_live_club(
        &fx.db,
        "devx",
        &format!("own-dx-{}@test.example", &new_id()[..6]),
        "SCTB",
        -33.456,
        -70.548,
    )
    .await;
    fx.db.upsert_type(&club_id, "C152").await.unwrap();
    fx.db
        .upsert_aircraft(&club_id, "CC-DX01", "C152", true)
        .await
        .unwrap();

    let email = format!("dx-{}@test.example", &new_id()[..8]);
    let (acct_id, acct_cookie) = create_account_with_session(&fx.db, &email, true, None).await;
    let (club_pid, mem_id) =
        add_active_club_membership(&fx.db, &acct_id, &email, "DX Pilot", &club_id, "pilot").await;
    fx.db
        .add_checkout(&club_pid, "C152", None, None)
        .await
        .unwrap();
    let (st, _, _) = send_json(
        &mut fx.router,
        "POST",
        "/api/account/switch-club",
        Some(&acct_cookie),
        None,
        Some(json!({ "membershipId": mem_id })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Register SAME install_id for club pilot AND personal pilot -> separate rows, no rebind!
    let install_id = format!("inst-shared-{}", &new_id()[..10]);
    let club_dev = fx
        .db
        .register_device_for_install(&club_pid, "Phone Club", Some("Pixel"), &install_id)
        .await
        .unwrap();
    let personal_pid = fx.db.ensure_personal_pilot(&acct_id, now).await.unwrap();
    let personal_dev = fx
        .db
        .register_device_for_install(&personal_pid, "Phone Personal", Some("Pixel"), &install_id)
        .await
        .unwrap();

    assert_ne!(club_dev.id, personal_dev.id);
    assert_eq!(
        fx.db.device_pilot_id(&club_dev.id).await.as_deref(),
        Some(club_pid.as_str()),
        "club device must still belong to club pilot"
    );
    assert_eq!(
        fx.db.device_pilot_id(&personal_dev.id).await.as_deref(),
        Some(personal_pid.as_str()),
        "personal device must belong to personal pilot"
    );

    // Give the club pilot an active check-in so club start would otherwise succeed.
    fx.db
        .issue_open_checkin(&club_pid, &club_dev.id, &club_id, now)
        .await
        .unwrap();

    // 1. Personal device used to start a club flight (with club session cookie) -> 403 device mismatch.
    let (st, _, b1) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        Some(&acct_cookie),
        Some(&personal_dev.token),
        Some(json!({ "tail": "CC-DX01", "visibility": "club" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b1}");
    assert_eq!(b1["error"], "device mismatch");

    // 2. Personal device with explicit mode=club (bearer-only) -> 403 device mismatch.
    let (st, _, b2) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        None,
        Some(&personal_dev.token),
        Some(json!({ "tail": "CC-DX01", "mode": "club" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b2}");
    assert_eq!(b2["error"], "device mismatch");

    // 3. Club device used for mode=personal start (with account cookie) -> 403 device mismatch.
    let (st, _, b3) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        Some(&acct_cookie),
        Some(&club_dev.token),
        Some(json!({ "tail": "N123DX", "visibility": "public", "mode": "personal" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b3}");
    assert_eq!(b3["error"], "device mismatch");

    // 4. Club device used for mode=personal start (bearer-only, no cookie) -> 403 device mismatch.
    let (st, _, b4) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        None,
        Some(&club_dev.token),
        Some(json!({ "tail": "N123DX", "visibility": "public", "mode": "personal" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b4}");
    assert_eq!(b4["error"], "device mismatch");
}

// ---------------------------------------------------------------------------
// 8(j) Trail fallback: club-less effective trailMinutes = 5 when no override; override wins
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn clubless_trail_fallback_and_override() {
    let mut fx = setup_harness().await;
    let email = format!("cl-tr-{}@test.example", &new_id()[..8]);
    let (acct_id, acct_cookie) = create_account_with_session(&fx.db, &email, true, None).await;
    let personal_pid = fx
        .db
        .ensure_personal_pilot(&acct_id, now_ms())
        .await
        .unwrap();
    let (_dev_id, dev_tok) = fx.db.create_device(&personal_pid, "phone").await.unwrap();

    // 1. Start without trailMinutes -> effectiveTrailMinutes = DEFAULT_TRAIL_MINUTES (5).
    let (st, _, start) = send_json(
        &mut fx.router,
        "POST",
        "/api/flights",
        Some(&acct_cookie),
        Some(&dev_tok),
        Some(json!({ "tail": "CC-TRL1", "visibility": "private" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{start}");
    assert!(start["trailMinutes"].is_null());
    assert_eq!(start["effectiveTrailMinutes"], DEFAULT_TRAIL_MINUTES);
    let fid = start["id"].as_str().unwrap().to_string();

    // 2. PATCH /api/flights/{id}/trail override to 15 -> override wins.
    let (st, _, p15) = send_json(
        &mut fx.router,
        "PATCH",
        &format!("/api/flights/{fid}/trail"),
        None,
        Some(&dev_tok),
        Some(json!({ "trailMinutes": 15 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{p15}");
    assert_eq!(p15["trailMinutes"], 15);
    assert_eq!(p15["effectiveTrailMinutes"], 15);

    // 3. PATCH /api/flights/{id}/trail back to null -> falls back to platform default 5.
    let (st, _, pnull) = send_json(
        &mut fx.router,
        "PATCH",
        &format!("/api/flights/{fid}/trail"),
        Some(&acct_cookie),
        None,
        Some(json!({ "trailMinutes": null })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{pnull}");
    assert!(pnull["trailMinutes"].is_null());
    assert_eq!(pnull["effectiveTrailMinutes"], DEFAULT_TRAIL_MINUTES);
}

// ---------------------------------------------------------------------------
// 8(k) Guest re-scope guard: personal pilot never re-scoped by QR or handoff check-in
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn guest_rescope_guard_never_mutates_personal_pilot() {
    let fx = setup_harness().await;
    let now = now_ms();
    let (club_id, _) = create_live_club(
        &fx.db,
        "grd",
        &format!("own-grd-{}@test.example", &new_id()[..6]),
        "SCTB",
        -33.456,
        -70.548,
    )
    .await;

    let email = format!("cl-grd-{}@test.example", &new_id()[..8]);
    let (acct_id, _) = create_account_with_session(&fx.db, &email, true, None).await;
    let personal_pid = fx.db.ensure_personal_pilot(&acct_id, now).await.unwrap();
    let (dev_id, _) = fx.db.create_device(&personal_pid, "phone").await.unwrap();

    // 1. Regular QR check-in by personal pilot (not a member of club_id) -> WrongClub, club_id stays NULL.
    let qr_tok = qr_token_for_club(&fx.db, &club_id, now).await;
    let res_qr = fx
        .db
        .submit_checkin(&personal_pid, Some(&dev_id), &qr_tok, now)
        .await;
    assert!(res_qr.is_err());
    assert_eq!(fx.db.pilot_club_id_opt(&personal_pid).await.unwrap(), None);

    // 2. Handoff token check-in by personal pilot -> WrongClub, club_id stays NULL.
    let (other_acct, _) = create_account_with_session(
        &fx.db,
        &format!("ho-own-{}@test.example", &new_id()[..6]),
        true,
        None,
    )
    .await;
    let (other_club_pid, _) = add_active_club_membership(
        &fx.db,
        &other_acct,
        &format!("ho-own-{}@test.example", &new_id()[..6]),
        "HO Pilot",
        &club_id,
        "pilot",
    )
    .await;
    let handoff = fx
        .db
        .mint_handoff_token(&club_id, &other_club_pid, None, now)
        .await
        .unwrap();
    let res_ho = fx
        .db
        .submit_checkin(&personal_pid, Some(&dev_id), &handoff, now)
        .await;
    assert!(res_ho.is_err());
    assert_eq!(fx.db.pilot_club_id_opt(&personal_pid).await.unwrap(), None);

    // 3. Even if a personal pilot's status were forcibly set to 'guest' in DB, both the Rust
    //    `is_unattached_guest` check and SQL WHERE guard (`account_id IS NULL AND club_id IS NOT NULL`)
    //    refuse to re-scope it on QR or handoff check-in.
    sqlx::query("UPDATE pilots SET status = 'guest' WHERE id = $1")
        .bind(&personal_pid)
        .execute(&fx.db.pool)
        .await
        .unwrap();

    let res_qr2 = fx
        .db
        .submit_checkin(&personal_pid, Some(&dev_id), &qr_tok, now)
        .await;
    assert!(res_qr2.is_err());
    assert_eq!(fx.db.pilot_club_id_opt(&personal_pid).await.unwrap(), None);

    let res_ho2 = fx
        .db
        .submit_checkin(&personal_pid, Some(&dev_id), &handoff, now)
        .await;
    assert!(res_ho2.is_err());
    assert_eq!(fx.db.pilot_club_id_opt(&personal_pid).await.unwrap(), None);
}
