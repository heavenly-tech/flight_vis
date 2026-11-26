//! Tests for M-26: verified-email gate on setting flight visibility to `"public"`
//! and `email_verified` exposure on `GET /api/club/me` (and `GET /api/account/me`).

use crate::club::{
    auth::{new_id, ACCOUNT_COOKIE, COOKIE},
    db::now_ms,
    pilot_live_trails_tests::http_stack,
    routes::clamp_default_start_visibility,
    visibility::FlightVisibility,
    ClubDb,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, Request, StatusCode},
};
use serde_json::{json, Value};
use serial_test::serial;
use tower::Service;

const EXPECTED_ERROR: &str = "email_not_verified";
const EXPECTED_MESSAGE: &str = "Verify your email to share flights publicly.";

async fn send_json(
    router: &mut axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    if let Some(tok) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {tok}"));
    }
    let req_body = match body {
        Some(val) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(val.to_string())
        }
        None => Body::empty(),
    };
    let res = router
        .call(builder.body(req_body).unwrap())
        .await
        .expect("router call");
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, headers, parsed)
}

async fn setup_live_default_club(db: &ClubDb) -> String {
    let club_id = db.default_club_id().await.expect("default club");
    let now = now_ms();
    sqlx::query(
        "UPDATE clubs
         SET email_verified_at = COALESCE(email_verified_at, $1),
             status = 'live',
             suspended_at = NULL
         WHERE id = $2",
    )
    .bind(now)
    .bind(&club_id)
    .execute(&db.pool)
    .await
    .expect("make default club live");
    db.upsert_type(&club_id, "C152").await.unwrap();
    for tail in ["N1", "N2", "N3"] {
        db.upsert_aircraft(&club_id, tail, "C152", true)
            .await
            .unwrap();
    }
    club_id
}

async fn seed_pilot_ready_to_fly(
    db: &ClubDb,
    club_id: &str,
    email: &str,
    verified: bool,
) -> (String, String, String, String) {
    let now = now_ms();
    let pilot_id = db
        .register(club_id, "Test Pilot", email, "L-1", "password12")
        .await
        .expect("register pilot");
    if verified {
        db.mark_email_verified(&pilot_id, now).await.unwrap();
    }
    db.approve_pilot(&pilot_id).await.unwrap();
    db.add_checkout(&pilot_id, "C152", None, None)
        .await
        .unwrap();
    let (device_id, device_token) = db.create_device(&pilot_id, "phone").await.unwrap();
    db.issue_open_checkin(&pilot_id, &device_id, club_id, now)
        .await
        .unwrap();
    let sess = db.create_pilot_session(&pilot_id, now).await.unwrap();
    let cookie = format!("{COOKIE}={sess}");
    (pilot_id, device_id, device_token, cookie)
}

fn cookie_pair_from_set_cookie(headers: &HeaderMap) -> String {
    let raw = headers
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("Set-Cookie header");
    raw.split(';').next().unwrap().trim().to_string()
}

// ---------------------------------------------------------------------------
// 1. Unit test: default-start clamping helper
// ---------------------------------------------------------------------------

#[test]
fn default_start_visibility_clamps_public_to_club_for_unverified() {
    assert_eq!(
        clamp_default_start_visibility(FlightVisibility::Public, false),
        FlightVisibility::Club
    );
    assert_eq!(
        clamp_default_start_visibility(FlightVisibility::Public, true),
        FlightVisibility::Public
    );
    assert_eq!(
        clamp_default_start_visibility(FlightVisibility::Club, false),
        FlightVisibility::Club
    );
    assert_eq!(
        clamp_default_start_visibility(FlightVisibility::Private, false),
        FlightVisibility::Private
    );
}

// ---------------------------------------------------------------------------
// 2. Unverified pilot flight start: public -> 403, club/private/default -> 200
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn unverified_pilot_cannot_start_public_flight_but_club_private_and_default_succeed() {
    let db = ClubDb::open_test().await.expect("db");
    let club_id = setup_live_default_club(&db).await;
    let (pilot_id, _dev_id, token, cookie) =
        seed_pilot_ready_to_fly(&db, &club_id, "unverified-start@test", false).await;
    let (_tmp, mut router) = http_stack(db.clone()).await;

    // Explicit visibility=public -> 403 email_not_verified; no flight created.
    let (st, _, body) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&cookie),
        Some(&token),
        Some(json!({ "tail": "N1", "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], EXPECTED_ERROR);
    assert_eq!(body["message"], EXPECTED_MESSAGE);
    assert!(db.list_my_flights(&pilot_id).await.is_empty());

    // Same unverified pilot with visibility=club -> 200.
    let (st, _, body) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&cookie),
        Some(&token),
        Some(json!({ "tail": "N1", "visibility": "club" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["visibility"], "club");
    let club_fid = body["id"].as_str().unwrap().to_string();
    assert_eq!(db.get_flight(&club_fid).await.unwrap().visibility, "club");

    let (st, _, _) = send_json(
        &mut router,
        "POST",
        &format!("/api/flights/{club_fid}/end"),
        Some(&cookie),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Same unverified pilot with visibility=private -> 200.
    let (st, _, body) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&cookie),
        Some(&token),
        Some(json!({ "tail": "N1", "visibility": "private" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["visibility"], "private");
    let priv_fid = body["id"].as_str().unwrap().to_string();

    let (st, _, _) = send_json(
        &mut router,
        "POST",
        &format!("/api/flights/{priv_fid}/end"),
        Some(&cookie),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Omitting visibility defaults to "club" (never 403).
    let (st, _, body) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&cookie),
        Some(&token),
        Some(json!({ "tail": "N1" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["visibility"], "club");
}

// ---------------------------------------------------------------------------
// 3. Guest pilot: public -> 403 on start & PATCH, club allowed, club/me false
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn guest_pilot_rejected_for_public_visibility_and_club_me_reports_false() {
    let db = ClubDb::open_test().await.expect("db");
    let club_id = setup_live_default_club(&db).await;
    let (_tmp, mut router) = http_stack(db.clone()).await;

    // Create guest session via HTTP.
    let (st, headers, guest_body) = send_json(
        &mut router,
        "POST",
        "/api/club/auth/guest",
        None,
        None,
        Some(json!({ "name": "Invitado" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let guest_cookie = cookie_pair_from_set_cookie(&headers);
    let guest_id = guest_body["pilot"]["id"].as_str().unwrap().to_string();
    assert_eq!(guest_body["pilot"]["status"], "guest");

    // GET /api/club/me for guest reports email_verified: false.
    let (st, _, me_body) = send_json(
        &mut router,
        "GET",
        "/api/club/me",
        Some(&guest_cookie),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me_body["pilot"]["status"], "guest");
    assert_eq!(me_body["email_verified"], json!(false));

    // Check in guest to obtain a device bearer token and grant checkout on C152.
    let qr = db.mint_checkin_qr_token(&club_id, now_ms()).await.unwrap();
    let (st, _, checkin_body) = send_json(
        &mut router,
        "POST",
        "/api/checkin",
        Some(&guest_cookie),
        None,
        Some(json!({ "token": qr })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let guest_token = checkin_body["token"].as_str().unwrap().to_string();
    db.add_checkout(&guest_id, "C152", None, None)
        .await
        .unwrap();

    // Guest starting a flight with visibility=public -> 403 email_not_verified.
    let (st, _, body) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&guest_cookie),
        Some(&guest_token),
        Some(json!({ "tail": "N1", "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], EXPECTED_ERROR);
    assert_eq!(body["message"], EXPECTED_MESSAGE);

    // Guest starting with visibility=club succeeds.
    let (st, _, body) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&guest_cookie),
        Some(&guest_token),
        Some(json!({ "tail": "N1", "visibility": "club" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let fid = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["visibility"], "club");

    // Guest PATCHing that flight to visibility=public -> 403 email_not_verified.
    let (st, _, body) = send_json(
        &mut router,
        "PATCH",
        &format!("/api/flights/{fid}"),
        Some(&guest_cookie),
        None,
        Some(json!({ "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], EXPECTED_ERROR);
    assert_eq!(body["message"], EXPECTED_MESSAGE);
    assert_eq!(db.get_flight(&fid).await.unwrap().visibility, "club");

    // Even if email_verified_at were manually populated on a guest row, status='guest' still blocks.
    db.mark_email_verified(&guest_id, now_ms()).await.unwrap();
    assert!(!db.pilot_email_verified(&guest_id).await);
    let (st, _, me_after) = send_json(
        &mut router,
        "GET",
        "/api/club/me",
        Some(&guest_cookie),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me_after["email_verified"], json!(false));
}

// ---------------------------------------------------------------------------
// 4. Verified pilot (pilot row OR linked account) allowed on start & PATCH
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn verified_pilot_and_account_verified_pilot_allowed_public_on_start_and_patch() {
    let db = ClubDb::open_test().await.expect("db");
    let club_id = setup_live_default_club(&db).await;
    let now = now_ms();

    // Case A: pilot verified directly via pilots.email_verified_at.
    let (_p_direct, _d1, tok_direct, cookie_direct) =
        seed_pilot_ready_to_fly(&db, &club_id, "verified-direct@test", true).await;

    // Case B: pilots.email_verified_at IS NULL, linked accounts.email_verified_at IS NOT NULL.
    let (p_acct_ver, _d2, tok_acct_ver, cookie_acct_ver) =
        seed_pilot_ready_to_fly(&db, &club_id, "verified-via-account@test", false).await;
    let acct_ver_id = match db.account_id_for_email("verified-via-account@test").await {
        Some(id) => id,
        None => {
            let id = new_id();
            sqlx::query(
                "INSERT INTO accounts (id, email, email_verified_at, global_role, created_at)
                 VALUES ($1, $2, $3, 'user', $3)",
            )
            .bind(&id)
            .bind("verified-via-account@test")
            .bind(now)
            .execute(&db.pool)
            .await
            .unwrap();
            id
        }
    };
    sqlx::query("UPDATE accounts SET email_verified_at = $1 WHERE id = $2")
        .bind(now)
        .bind(&acct_ver_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE pilots SET account_id = $1, email_verified_at = NULL WHERE id = $2")
        .bind(&acct_ver_id)
        .bind(&p_acct_ver)
        .execute(&db.pool)
        .await
        .unwrap();

    // Case C: linked account where BOTH pilot and account email_verified_at are NULL.
    let (p_both_null, _d3, tok_both_null, cookie_both_null) =
        seed_pilot_ready_to_fly(&db, &club_id, "unverified-both@test", false).await;
    let acct_unver_id = match db.account_id_for_email("unverified-both@test").await {
        Some(id) => id,
        None => {
            let id = new_id();
            sqlx::query(
                "INSERT INTO accounts (id, email, email_verified_at, global_role, created_at)
                 VALUES ($1, $2, NULL, 'user', $3)",
            )
            .bind(&id)
            .bind("unverified-both@test")
            .bind(now)
            .execute(&db.pool)
            .await
            .unwrap();
            id
        }
    };
    sqlx::query("UPDATE accounts SET email_verified_at = NULL WHERE id = $1")
        .bind(&acct_unver_id)
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE pilots SET account_id = $1, email_verified_at = NULL WHERE id = $2")
        .bind(&acct_unver_id)
        .bind(&p_both_null)
        .execute(&db.pool)
        .await
        .unwrap();

    let (_tmp, mut router) = http_stack(db.clone()).await;

    // Case A: direct verified pilot -> public on start and PATCH.
    let (st, _, body) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&cookie_direct),
        Some(&tok_direct),
        Some(json!({ "tail": "N1", "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["visibility"], "public");
    let fid1 = body["id"].as_str().unwrap().to_string();
    assert_eq!(db.get_flight(&fid1).await.unwrap().visibility, "public");

    let (st, _, _) = send_json(
        &mut router,
        "POST",
        &format!("/api/flights/{fid1}/end"),
        Some(&cookie_direct),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _, body) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&cookie_direct),
        Some(&tok_direct),
        Some(json!({ "tail": "N1", "visibility": "club" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let fid2 = body["id"].as_str().unwrap().to_string();

    let (st, _, _) = send_json(
        &mut router,
        "PATCH",
        &format!("/api/flights/{fid2}"),
        Some(&cookie_direct),
        None,
        Some(json!({ "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(db.get_flight(&fid2).await.unwrap().visibility, "public");

    // Case B: pilot verified via linked account -> /api/club/me true, start & PATCH public succeed.
    let (st, _, me_b) = send_json(
        &mut router,
        "GET",
        "/api/club/me",
        Some(&cookie_acct_ver),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me_b["email_verified"], json!(true));

    let (st, _, body) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&cookie_acct_ver),
        Some(&tok_acct_ver),
        Some(json!({ "tail": "N2", "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["visibility"], "public");
    let fid_b1 = body["id"].as_str().unwrap().to_string();
    let (st, _, _) = send_json(
        &mut router,
        "POST",
        &format!("/api/flights/{fid_b1}/end"),
        Some(&cookie_acct_ver),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _, body) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&cookie_acct_ver),
        Some(&tok_acct_ver),
        Some(json!({ "tail": "N2", "visibility": "private" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let fid_b2 = body["id"].as_str().unwrap().to_string();
    let (st, _, _) = send_json(
        &mut router,
        "PATCH",
        &format!("/api/flights/{fid_b2}"),
        Some(&cookie_acct_ver),
        None,
        Some(json!({ "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(db.get_flight(&fid_b2).await.unwrap().visibility, "public");

    // Case C: both pilot and linked account unverified -> /api/club/me false, start public 403.
    let (st, _, me_c) = send_json(
        &mut router,
        "GET",
        "/api/club/me",
        Some(&cookie_both_null),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me_c["email_verified"], json!(false));

    let (st, _, body) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&cookie_both_null),
        Some(&tok_both_null),
        Some(json!({ "tail": "N3", "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], EXPECTED_ERROR);
    assert_eq!(body["message"], EXPECTED_MESSAGE);
}

// ---------------------------------------------------------------------------
// 5. Unverified pilot PATCH to public rejected; existing public flight unaffected
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn unverified_pilot_patch_to_public_rejected_and_existing_public_flight_keeps_working() {
    let db = ClubDb::open_test().await.expect("db");
    let club_id = setup_live_default_club(&db).await;
    let (unver_id, unver_dev_id, unver_token, unver_cookie) =
        seed_pilot_ready_to_fly(&db, &club_id, "unverified-patch@test", false).await;
    let (_other_id, _other_dev_id, other_token, other_cookie) =
        seed_pilot_ready_to_fly(&db, &club_id, "other-verified@test", true).await;
    let (_tmp, mut router) = http_stack(db.clone()).await;

    // 1) Unverified pilot starts a club flight and tries to PATCH to public -> 403.
    let (st, _, body) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&unver_cookie),
        Some(&unver_token),
        Some(json!({ "tail": "N1", "visibility": "club" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let club_fid = body["id"].as_str().unwrap().to_string();

    let (st, _, body) = send_json(
        &mut router,
        "PATCH",
        &format!("/api/flights/{club_fid}"),
        Some(&unver_cookie),
        None,
        Some(json!({ "visibility": "public", "show_pilot_name_public": true })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], EXPECTED_ERROR);
    assert_eq!(body["message"], EXPECTED_MESSAGE);
    let row = db.get_flight(&club_fid).await.unwrap();
    assert_eq!(row.visibility, "club");
    assert!(!row.show_pilot_name_public);

    // Transition club -> private succeeds; private -> public is rejected.
    let (st, _, _) = send_json(
        &mut router,
        "PATCH",
        &format!("/api/flights/{club_fid}"),
        Some(&unver_cookie),
        None,
        Some(json!({ "visibility": "private" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        db.get_flight(&club_fid).await.unwrap().visibility,
        "private"
    );

    let (st, _, body) = send_json(
        &mut router,
        "PATCH",
        &format!("/api/flights/{club_fid}"),
        Some(&unver_cookie),
        None,
        Some(json!({ "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], EXPECTED_ERROR);
    assert_eq!(
        db.get_flight(&club_fid).await.unwrap().visibility,
        "private"
    );

    let (st, _, _) = send_json(
        &mut router,
        "POST",
        &format!("/api/flights/{club_fid}/end"),
        Some(&unver_cookie),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // 2) Pre-existing public flight owned by unverified pilot keeps working untouched.
    let existing_pub_fid = db
        .start_flight(
            &unver_id,
            &unver_dev_id,
            "N1",
            FlightVisibility::Public,
            false,
            now_ms(),
        )
        .await
        .expect("seed existing public flight");
    assert_eq!(
        db.get_flight(&existing_pub_fid).await.unwrap().visibility,
        "public"
    );

    // PATCH trail_minutes on existing public flight -> 200 and visibility unchanged.
    let (st, _, trail_body) = send_json(
        &mut router,
        "PATCH",
        &format!("/api/flights/{existing_pub_fid}/trail"),
        None,
        Some(&unver_token),
        Some(json!({ "trailMinutes": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(trail_body["trailMinutes"], json!(10));
    assert_eq!(
        db.get_flight(&existing_pub_fid).await.unwrap().visibility,
        "public"
    );

    // Re-sending the current value "public" on an already-public flight -> 200.
    let (st, _, _) = send_json(
        &mut router,
        "PATCH",
        &format!("/api/flights/{existing_pub_fid}"),
        Some(&unver_cookie),
        None,
        Some(json!({ "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        db.get_flight(&existing_pub_fid).await.unwrap().visibility,
        "public"
    );

    // Patching show_pilot_name_public on an already-public flight -> 200.
    let (st, _, _) = send_json(
        &mut router,
        "PATCH",
        &format!("/api/flights/{existing_pub_fid}"),
        Some(&unver_cookie),
        None,
        Some(json!({ "show_pilot_name_public": true })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let row = db.get_flight(&existing_pub_fid).await.unwrap();
    assert_eq!(row.visibility, "public");
    assert!(row.show_pilot_name_public);

    // Fixes ingestion on an existing public flight -> 200, visibility unchanged.
    let (st, _, ingest_body) = send_json(
        &mut router,
        "POST",
        "/api/ingest",
        None,
        Some(&unver_token),
        Some(json!({
            "fixes": [{
                "t": now_ms(),
                "lat": -33.45,
                "lon": -70.55,
                "alt": 900.0
            }]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ingest_body["accepted"], json!(1));
    assert_eq!(
        db.get_flight(&existing_pub_fid).await.unwrap().visibility,
        "public"
    );

    // Ending an existing public flight -> 200, visibility unchanged.
    let (st, _, _) = send_json(
        &mut router,
        "POST",
        &format!("/api/flights/{existing_pub_fid}/end"),
        Some(&unver_cookie),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let ended = db.get_flight(&existing_pub_fid).await.unwrap();
    assert_eq!(ended.status, "ended");
    assert_eq!(ended.visibility, "public");

    // 3) Ownership check runs before email_not_verified (no cross-pilot oracle).
    let (st, _, body) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&other_cookie),
        Some(&other_token),
        Some(json!({ "tail": "N2", "visibility": "club" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let other_fid = body["id"].as_str().unwrap().to_string();

    let (st, _, body) = send_json(
        &mut router,
        "PATCH",
        &format!("/api/flights/{other_fid}"),
        Some(&unver_cookie),
        None,
        Some(json!({ "visibility": "public" })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");
}

// ---------------------------------------------------------------------------
// 6. GET /api/club/me (and GET /api/account/me) email_verified field
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn club_me_and_account_me_expose_email_verified_boolean() {
    let db = ClubDb::open_test().await.expect("db");
    let club_id = setup_live_default_club(&db).await;
    let now = now_ms();

    let (_p_ver, _, _, cookie_ver) =
        seed_pilot_ready_to_fly(&db, &club_id, "me-verified@test", true).await;
    let (_p_unver, _, _, cookie_unver) =
        seed_pilot_ready_to_fly(&db, &club_id, "me-unverified@test", false).await;

    // Account sessions for GET /api/account/me (one verified, one unverified).
    let acct_ver_id = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, email_verified_at, global_role, created_at)
         VALUES ($1, 'acct-me-ver@test', $2, 'user', $2)",
    )
    .bind(&acct_ver_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let acct_ver_sess = db
        .create_account_session(&acct_ver_id, None, now)
        .await
        .unwrap();
    let acct_ver_cookie = format!("{ACCOUNT_COOKIE}={acct_ver_sess}");

    let acct_unver_id = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, email_verified_at, global_role, created_at)
         VALUES ($1, 'acct-me-unver@test', NULL, 'user', $2)",
    )
    .bind(&acct_unver_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let acct_unver_sess = db
        .create_account_session(&acct_unver_id, None, now)
        .await
        .unwrap();
    let acct_unver_cookie = format!("{ACCOUNT_COOKIE}={acct_unver_sess}");

    let (_tmp, mut router) = http_stack(db.clone()).await;

    // Anonymous /api/club/me -> pilot: null, email_verified: false.
    let (st, _, anon) = send_json(&mut router, "GET", "/api/club/me", None, None, None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(anon["pilot"].is_null());
    assert_eq!(anon["email_verified"], json!(false));

    // Verified pilot /api/club/me -> email_verified: true, all existing fields preserved.
    let (st, _, ver) = send_json(
        &mut router,
        "GET",
        "/api/club/me",
        Some(&cookie_ver),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ver["email_verified"], json!(true));
    assert_eq!(ver["pilot"]["email"], "me-verified@test");
    assert!(ver["clubSlug"].is_string());
    assert!(ver["clubName"].is_string());
    assert!(ver["clubDefaultLocale"].is_string());
    assert!(ver["clubTrailMinutesDefault"].is_number());
    assert!(ver["clubShowNearbyPublic"].is_boolean());
    assert!(ver["clubNearbyPublicRadiusKm"].is_number());

    // Unverified pilot /api/club/me -> email_verified: false.
    let (st, _, unver) = send_json(
        &mut router,
        "GET",
        "/api/club/me",
        Some(&cookie_unver),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(unver["email_verified"], json!(false));
    assert_eq!(unver["pilot"]["email"], "me-unverified@test");

    // Guest /api/club/me -> email_verified: false.
    let (st, guest_headers, _) = send_json(
        &mut router,
        "POST",
        "/api/club/auth/guest",
        None,
        None,
        Some(json!({ "name": "Guest Me" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let guest_cookie = cookie_pair_from_set_cookie(&guest_headers);
    let (st, _, guest_me) = send_json(
        &mut router,
        "GET",
        "/api/club/me",
        Some(&guest_cookie),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(guest_me["pilot"]["status"], "guest");
    assert_eq!(guest_me["email_verified"], json!(false));

    // Verified & unverified /api/account/me.
    let (st, _, acct_ver) = send_json(
        &mut router,
        "GET",
        "/api/account/me",
        Some(&acct_ver_cookie),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(acct_ver["email_verified"], json!(true));
    assert!(acct_ver["account"]["emailVerifiedAt"].is_number());

    let (st, _, acct_unver) = send_json(
        &mut router,
        "GET",
        "/api/account/me",
        Some(&acct_unver_cookie),
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(acct_unver["email_verified"], json!(false));
    assert!(acct_unver["account"]["emailVerifiedAt"].is_null());
}
