//! Unit and HTTP integration tests for live aircraft trails, club defaults, pilot overrides,
//! CSRF protection, and cross-club isolation.

use crate::{
    auth::Auth,
    build_router,
    club::{
        auth::{hash_password, new_id},
        db::now_ms,
        trail::{
            window_and_downsample_trail, TrailMinutes, MAX_TRAIL_POINTS, MAX_TRAIL_WINDOW_MIN,
        },
        ClubDb,
    },
    device_auth::DeviceAuth,
    live::{LiveEngine, LiveFix},
    store::Store,
    AppState,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use serde_json::{json, Value};
use serial_test::serial;
use std::{collections::HashSet, sync::Arc};
use tower::Service;

// ---------------------------------------------------------------------------
// 1. Pure windowing & downsampling unit tests
// ---------------------------------------------------------------------------

#[test]
fn trail_minutes_validation_accepts_allowed_and_rejects_invalid() {
    for valid in [-1i16, 0, 2, 5, 10, 15, 30] {
        let parsed = TrailMinutes::from_i16(valid).expect("valid");
        assert_eq!(parsed.as_i16(), valid);
        assert_eq!(TrailMinutes::from_json_value(&json!(valid)), Ok(parsed));
    }
    for invalid in [-2i16, 1, 3, 4, 7, 20, 45, 60, 120] {
        assert!(TrailMinutes::from_i16(invalid).is_none());
        assert!(TrailMinutes::from_json_value(&json!(invalid)).is_err());
    }
    for bad_json in [
        json!(null),
        json!("5"),
        json!(5.5),
        json!(true),
        json!([]),
        json!({}),
    ] {
        assert!(TrailMinutes::from_json_value(&bad_json).is_err());
    }
    assert_eq!(
        TrailMinutes::from_optional_json_value(&json!(null)),
        Ok(None)
    );
    assert_eq!(
        TrailMinutes::from_optional_json_value(&json!(10)),
        Ok(Some(TrailMinutes::Min10))
    );
    assert!(TrailMinutes::from_optional_json_value(&json!(7)).is_err());
}

#[test]
fn windowing_off_returns_empty() {
    let base = 1_700_000_000_000i64;
    let pts = vec![(base - 60_000, -33.45, -70.55), (base, -33.44, -70.54)];
    let out = window_and_downsample_trail(&pts, TrailMinutes::Off, MAX_TRAIL_POINTS);
    assert!(out.is_empty());
}

#[test]
fn windowing_applies_cutoff_from_latest_fix() {
    let latest = 1_700_000_000_000i64;
    let pts = vec![
        (latest - 10 * 60_000, -33.50, -70.60),
        (latest - 5 * 60_000 - 1, -33.49, -70.59),
        (latest - 5 * 60_000, -33.48, -70.58),
        (latest - 2 * 60_000, -33.47, -70.57),
        (latest, -33.45, -70.55),
    ];
    let out5 = window_and_downsample_trail(&pts, TrailMinutes::Min5, MAX_TRAIL_POINTS);
    assert_eq!(out5.len(), 3);
    assert_eq!(out5[0].0, latest - 5 * 60_000);
    assert_eq!(out5[2].0, latest);

    let out2 = window_and_downsample_trail(&pts, TrailMinutes::Min2, MAX_TRAIL_POINTS);
    assert_eq!(out2.len(), 2);
    assert_eq!(out2[0].0, latest - 2 * 60_000);
    assert_eq!(out2[1].0, latest);
}

#[test]
fn windowing_all_is_capped_at_max_trail_window_120_min() {
    let latest = 1_700_000_000_000i64;
    let max_ms = MAX_TRAIL_WINDOW_MIN * 60_000;
    let pts = vec![
        (latest - max_ms - 60_000, -33.60, -70.70), // 121 min old -> dropped
        (latest - max_ms, -33.55, -70.65),          // exact 120 min boundary -> kept
        (latest - 30 * 60_000, -33.50, -70.60),
        (latest, -33.45, -70.55),
    ];
    let out = window_and_downsample_trail(&pts, TrailMinutes::All, MAX_TRAIL_POINTS);
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].0, latest - max_ms);
    assert_eq!(out.last().unwrap().0, latest);
}

#[test]
fn downsampling_caps_at_max_points_preserving_first_and_last_and_order() {
    let start = 1_700_000_000_000i64;
    // 1000 points at 1 Hz (~16.6 min) within a 30-min window
    let pts: Vec<(i64, f64, f64)> = (0..1000)
        .map(|i| (start + i * 1_000, -33.5 + i as f64 * 0.0001, -70.6))
        .collect();

    let out = window_and_downsample_trail(&pts, TrailMinutes::Min30, MAX_TRAIL_POINTS);
    assert_eq!(out.len(), MAX_TRAIL_POINTS);
    assert_eq!(out.first().unwrap().0, start);
    assert_eq!(out.last().unwrap().0, start + 999 * 1_000);
    for pair in out.windows(2) {
        assert!(pair[0].0 < pair[1].0, "timestamps must strictly increase");
    }

    // Small cap test
    let small = window_and_downsample_trail(&pts, TrailMinutes::Min30, 5);
    assert_eq!(small.len(), 5);
    assert_eq!(small[0].0, start);
    assert_eq!(small[4].0, start + 999 * 1_000);
}

// ---------------------------------------------------------------------------
// HTTP integration test fixtures & helpers
// ---------------------------------------------------------------------------

async fn http_stack(db: ClubDb) -> (tempfile::TempDir, axum::Router) {
    let data_dir = tempfile::tempdir().unwrap();
    let dist_dir = data_dir.path().join("dist");
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir,
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
        account_password_account_throttle: crate::LoginThrottle::default(),
        public_live_trails_throttle: crate::LoginThrottle::with_policy(
            crate::OverflowPolicy::RejectNew,
        ),
        mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
        webauthn: None,
        sse_limiter: Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails: HashSet::new(),
    };
    (data_dir, build_router(state))
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

async fn seed_member(
    db: &ClubDb,
    club_id: &str,
    name: &str,
    email: &str,
    role: &str,
) -> (String, String) {
    let now = now_ms();
    let hash = hash_password("password12").unwrap();
    let pilot_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, $3, $4, '', 'approved', $5, $6, $7, $7)",
    )
    .bind(&pilot_id)
    .bind(club_id)
    .bind(name)
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
    let sess = db.create_pilot_session(&pilot_id, now).await.unwrap();
    (pilot_id, format!("fv_club_session={sess}"))
}

async fn send_json(
    router: &mut axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    bearer: Option<&str>,
    origin: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    if let Some(t) = bearer {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(o) = origin {
        b = b
            .header(header::ORIGIN, o)
            .header(header::HOST, "127.0.0.1:8080");
    }
    let req = if let Some(json_body) = body {
        b.header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json_body.to_string()))
            .unwrap()
    } else {
        b.body(Body::empty()).unwrap()
    };
    let res = router.call(req).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, parsed)
}

async fn checkin_and_start_flight(
    router: &mut axum::Router,
    db: &ClubDb,
    club_id: &str,
    cookie: &str,
    device_token: &str,
    tail: &str,
    trail_minutes: Option<Value>,
) -> (StatusCode, Value) {
    let qr = db
        .mint_checkin_qr_token(club_id, now_ms())
        .await
        .expect("qr");
    let (st, _) = send_json(
        router,
        "POST",
        "/api/checkin",
        Some(cookie),
        Some(device_token),
        None,
        Some(json!({ "token": qr })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let mut payload = json!({ "tail": tail, "visibility": "club" });
    if let Some(tm) = trail_minutes {
        payload["trailMinutes"] = tm;
    }
    send_json(
        router,
        "POST",
        "/api/flights",
        Some(cookie),
        Some(device_token),
        None,
        Some(payload),
    )
    .await
}

async fn seed_fixes_minutes_back(
    db: &ClubDb,
    flight_id: &str,
    device_id: &str,
    latest_t: i64,
    minutes_back: &[i64],
) {
    let fixes: Vec<LiveFix> = minutes_back
        .iter()
        .enumerate()
        .map(|(idx, &min_ago)| LiveFix {
            t: latest_t - min_ago * 60_000,
            lat: -33.46 + idx as f64 * 0.002,
            lon: -70.56 + idx as f64 * 0.002,
            alt: Some(900.0),
            alt_ref: Some("wgs84_ellipsoid".to_string()),
            speed: Some(35.0),
            course: Some(90.0),
            accuracy: Some(5.0),
        })
        .collect();
    db.append_fixes(flight_id, device_id, &fixes)
        .await
        .expect("append_fixes");
}

// ---------------------------------------------------------------------------
// 2 & 3. Effective value + POST /api/flights with trailMinutes
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn post_flights_trail_minutes_and_effective_resolution() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;
    let (club_id, _) = create_live_club(&db, "trail-eff", "owner-eff@test").await;
    db.upsert_type(&club_id, "C152").await.unwrap();
    db.upsert_aircraft(&club_id, "CC-DEF", "C152", true)
        .await
        .unwrap();
    db.upsert_aircraft(&club_id, "CC-OVR", "C152", true)
        .await
        .unwrap();

    let (_admin_id, admin_cookie) =
        seed_member(&db, &club_id, "Admin", "adm-eff@test", "admin").await;
    let (pilot1, cookie1) = seed_member(&db, &club_id, "Pilot 1", "p1-eff@test", "pilot").await;
    let (pilot2, cookie2) = seed_member(&db, &club_id, "Pilot 2", "p2-eff@test", "pilot").await;
    db.add_checkout(&pilot1, "C152", None, None).await.unwrap();
    db.add_checkout(&pilot2, "C152", None, None).await.unwrap();
    let (dev1, tok1) = db.create_device(&pilot1, "phone1").await.unwrap();
    let (dev2, tok2) = db.create_device(&pilot2, "phone2").await.unwrap();

    // Invalid trailMinutes on POST /api/flights -> 400
    let (st_bad, body_bad) = checkin_and_start_flight(
        &mut router,
        &db,
        &club_id,
        &cookie1,
        &tok1,
        "CC-DEF",
        Some(json!(7)),
    )
    .await;
    assert_eq!(st_bad, StatusCode::BAD_REQUEST, "{body_bad}");

    // Flight 1: omitted trailMinutes (NULL -> inherits club default 5), reusing the live check-in
    let (st1, res1) = send_json(
        &mut router,
        "POST",
        "/api/flights",
        Some(&cookie1),
        Some(&tok1),
        None,
        Some(json!({ "tail": "CC-DEF", "visibility": "club" })),
    )
    .await;
    assert_eq!(st1, StatusCode::OK, "{res1}");
    assert_eq!(res1["trailMinutes"], Value::Null);
    assert_eq!(res1["effectiveTrailMinutes"], json!(5));
    let fid1 = res1["id"].as_str().unwrap().to_string();

    // Flight 2: explicit override 2 min
    let (st2, res2) = checkin_and_start_flight(
        &mut router,
        &db,
        &club_id,
        &cookie2,
        &tok2,
        "CC-OVR",
        Some(json!(2)),
    )
    .await;
    assert_eq!(st2, StatusCode::OK, "{res2}");
    assert_eq!(res2["trailMinutes"], json!(2));
    assert_eq!(res2["effectiveTrailMinutes"], json!(2));
    let fid2 = res2["id"].as_str().unwrap().to_string();

    // Seed fixes spanning 20 minutes back for both flights: 20, 12, 8, 4, 1, 0 min ago
    let latest = now_ms();
    seed_fixes_minutes_back(&db, &fid1, &dev1, latest, &[20, 12, 8, 4, 1, 0]).await;
    seed_fixes_minutes_back(&db, &fid2, &dev2, latest, &[20, 12, 8, 4, 1, 0]).await;

    // Initial check: club default = 5 -> fid1 gets 5 min (4,1,0 => 3 pts); fid2 gets 2 min (1,0 => 2 pts)
    let (st_map, map_body) = send_json(
        &mut router,
        "GET",
        "/api/admin/map",
        Some(&admin_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_map, StatusCode::OK);
    let ac_list = map_body["aircraft"].as_array().unwrap();
    let pos1 = ac_list.iter().find(|a| a["flightId"] == fid1).unwrap();
    let pos2 = ac_list.iter().find(|a| a["flightId"] == fid2).unwrap();
    assert_eq!(pos1["trailMinutes"], json!(5));
    assert_eq!(pos2["trailMinutes"], json!(2));

    let (st_tr, tr_body) = send_json(
        &mut router,
        "GET",
        "/api/admin/map/trails",
        Some(&admin_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_tr, StatusCode::OK);
    let trails = tr_body["trails"].as_array().unwrap();
    let tr1 = trails.iter().find(|t| t["flightId"] == fid1).unwrap();
    let tr2 = trails.iter().find(|t| t["flightId"] == fid2).unwrap();
    assert_eq!(tr1["trailMinutes"], json!(5));
    assert_eq!(tr1["points"].as_array().unwrap().len(), 3);
    assert_eq!(tr2["trailMinutes"], json!(2));
    assert_eq!(tr2["points"].as_array().unwrap().len(), 2);

    // Change club default to 15 min -> fid1 (NULL) updates to 15 min (12,8,4,1,0 => 5 pts), fid2 stays 2 min (2 pts)!
    let (st_patch, patch_body) = send_json(
        &mut router,
        "PATCH",
        "/api/admin/club/trail",
        Some(&admin_cookie),
        None,
        None,
        Some(json!({ "trailMinutes": 15 })),
    )
    .await;
    assert_eq!(st_patch, StatusCode::OK, "{patch_body}");
    assert_eq!(patch_body["trailMinutes"], json!(15));

    let (st_disp, disp_body) = send_json(
        &mut router,
        "GET",
        "/api/display/map/trails",
        Some(&admin_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_disp, StatusCode::OK);
    let disp_trails = disp_body["trails"].as_array().unwrap();
    let dtr1 = disp_trails.iter().find(|t| t["flightId"] == fid1).unwrap();
    let dtr2 = disp_trails.iter().find(|t| t["flightId"] == fid2).unwrap();
    assert_eq!(dtr1["trailMinutes"], json!(15));
    assert_eq!(dtr1["points"].as_array().unwrap().len(), 5);
    assert_eq!(dtr2["trailMinutes"], json!(2));
    assert_eq!(dtr2["points"].as_array().unwrap().len(), 2);

    // Change club default to Off (0) -> fid1 returns 0 points, fid2 override still returns 2 points
    let (st_off, _) = send_json(
        &mut router,
        "PATCH",
        "/api/admin/club/trail",
        Some(&admin_cookie),
        None,
        None,
        Some(json!({ "trailMinutes": 0 })),
    )
    .await;
    assert_eq!(st_off, StatusCode::OK);

    let (_, tr_after_off) = send_json(
        &mut router,
        "GET",
        "/api/admin/map/trails",
        Some(&admin_cookie),
        None,
        None,
        None,
    )
    .await;
    let off_trails = tr_after_off["trails"].as_array().unwrap();
    let otr1 = off_trails.iter().find(|t| t["flightId"] == fid1).unwrap();
    let otr2 = off_trails.iter().find(|t| t["flightId"] == fid2).unwrap();
    assert_eq!(otr1["trailMinutes"], json!(0));
    assert!(otr1["points"].as_array().unwrap().is_empty());
    assert_eq!(otr2["trailMinutes"], json!(2));
    assert_eq!(otr2["points"].as_array().unwrap().len(), 2);
}

// ---------------------------------------------------------------------------
// 4. PATCH /api/flights/{id}/trail auth, ownership, CSRF, ended flight, validation
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn patch_flight_trail_auth_matrix_and_validation() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;

    let (club_a, _) = create_live_club(&db, "trail-pa", "owner-pa@test").await;
    let (club_b, _) = create_live_club(&db, "trail-pb", "owner-pb@test").await;
    db.upsert_type(&club_a, "C152").await.unwrap();
    db.upsert_aircraft(&club_a, "CC-P1", "C152", true)
        .await
        .unwrap();

    let (owner_id, owner_cookie) =
        seed_member(&db, &club_a, "Owner", "owner-p@test", "pilot").await;
    let (_other_id, other_cookie) =
        seed_member(&db, &club_a, "Other", "other-p@test", "pilot").await;
    let (_staff_id, staff_cookie) =
        seed_member(&db, &club_a, "Staff", "staff-p@test", "staff").await;
    let (_admin_a_id, admin_a_cookie) =
        seed_member(&db, &club_a, "Admin A", "admin-a-p@test", "admin").await;
    let (_admin_b_id, admin_b_cookie) =
        seed_member(&db, &club_b, "Admin B", "admin-b-p@test", "admin").await;

    db.add_checkout(&owner_id, "C152", None, None)
        .await
        .unwrap();
    let (_dev_id, owner_token) = db.create_device(&owner_id, "owner-phone").await.unwrap();

    let (st_start, start_body) = checkin_and_start_flight(
        &mut router,
        &db,
        &club_a,
        &owner_cookie,
        &owner_token,
        "CC-P1",
        None,
    )
    .await;
    assert_eq!(st_start, StatusCode::OK, "{start_body}");
    let fid = start_body["id"].as_str().unwrap().to_string();
    let uri = format!("/api/flights/{fid}/trail");

    // Unauthenticated -> 401
    let (st, _) = send_json(
        &mut router,
        "PATCH",
        &uri,
        None,
        None,
        None,
        Some(json!({ "trailMinutes": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Other pilot same club -> 403
    let (st, _) = send_json(
        &mut router,
        "PATCH",
        &uri,
        Some(&other_cookie),
        None,
        None,
        Some(json!({ "trailMinutes": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Same-club staff -> 403
    let (st, _) = send_json(
        &mut router,
        "PATCH",
        &uri,
        Some(&staff_cookie),
        None,
        None,
        Some(json!({ "trailMinutes": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Same-club admin -> 403
    let (st, _) = send_json(
        &mut router,
        "PATCH",
        &uri,
        Some(&admin_a_cookie),
        None,
        None,
        Some(json!({ "trailMinutes": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Other-club admin -> 403
    let (st, _) = send_json(
        &mut router,
        "PATCH",
        &uri,
        Some(&admin_b_cookie),
        None,
        None,
        Some(json!({ "trailMinutes": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Cross-site Origin on cookie auth -> 403 (CSRF)
    let (st, _) = send_json(
        &mut router,
        "PATCH",
        &uri,
        Some(&owner_cookie),
        None,
        Some("https://evil.example"),
        Some(json!({ "trailMinutes": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Owner with invalid value -> 400
    let (st, _) = send_json(
        &mut router,
        "PATCH",
        &uri,
        Some(&owner_cookie),
        None,
        None,
        Some(json!({ "trailMinutes": 7 })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Owner with missing trailMinutes key -> 400
    let (st, _) = send_json(
        &mut router,
        "PATCH",
        &uri,
        Some(&owner_cookie),
        None,
        None,
        Some(json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Owner via cookie -> 200
    let (st, body) = send_json(
        &mut router,
        "PATCH",
        &uri,
        Some(&owner_cookie),
        None,
        None,
        Some(json!({ "trailMinutes": 30 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["trailMinutes"], json!(30));
    assert_eq!(body["effectiveTrailMinutes"], json!(30));

    // Owner via device Bearer token alone (Android app) -> 200, setting null clears override
    let (st, body) = send_json(
        &mut router,
        "PATCH",
        &uri,
        None,
        Some(&owner_token),
        None,
        Some(json!({ "trailMinutes": null })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["trailMinutes"], Value::Null);
    assert_eq!(body["effectiveTrailMinutes"], json!(5));

    // Nonexistent flight -> 404
    let (st, _) = send_json(
        &mut router,
        "PATCH",
        "/api/flights/no-such-flight-id/trail",
        Some(&owner_cookie),
        None,
        None,
        Some(json!({ "trailMinutes": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // End the flight, then attempt PATCH -> 409 Conflict
    let (st_end, _) = send_json(
        &mut router,
        "POST",
        &format!("/api/flights/{fid}/end"),
        Some(&owner_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_end, StatusCode::OK);

    let (st, _) = send_json(
        &mut router,
        "PATCH",
        &uri,
        Some(&owner_cookie),
        None,
        None,
        Some(json!({ "trailMinutes": 15 })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
}

// ---------------------------------------------------------------------------
// 5. PATCH /api/admin/club/trail auth, CSRF, validation
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn patch_admin_club_trail_auth_csrf_and_validation() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;
    let (club_id, _) = create_live_club(&db, "trail-adm", "owner-adm@test").await;

    let (_admin_id, admin_cookie) =
        seed_member(&db, &club_id, "Admin", "adm-tr@test", "admin").await;
    let (_staff_id, staff_cookie) =
        seed_member(&db, &club_id, "Staff", "stf-tr@test", "staff").await;
    let (_pilot_id, pilot_cookie) =
        seed_member(&db, &club_id, "Pilot", "pil-tr@test", "pilot").await;

    // Unauthenticated -> 401
    let (st, _) = send_json(
        &mut router,
        "PATCH",
        "/api/admin/club/trail",
        None,
        None,
        None,
        Some(json!({ "trailMinutes": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Pilot -> 403
    let (st, _) = send_json(
        &mut router,
        "PATCH",
        "/api/admin/club/trail",
        Some(&pilot_cookie),
        None,
        None,
        Some(json!({ "trailMinutes": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Staff -> 403
    let (st, _) = send_json(
        &mut router,
        "PATCH",
        "/api/admin/club/trail",
        Some(&staff_cookie),
        None,
        None,
        Some(json!({ "trailMinutes": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Cross-site Origin -> 403
    let (st, _) = send_json(
        &mut router,
        "PATCH",
        "/api/admin/club/trail",
        Some(&admin_cookie),
        None,
        Some("https://evil.example"),
        Some(json!({ "trailMinutes": 10 })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Invalid values -> 400 (including null for club default)
    for bad in [
        json!({ "trailMinutes": 7 }),
        json!({ "trailMinutes": null }),
        json!({ "trailMinutes": "5" }),
        json!({}),
    ] {
        let (st, _) = send_json(
            &mut router,
            "PATCH",
            "/api/admin/club/trail",
            Some(&admin_cookie),
            None,
            None,
            Some(bad),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }

    // Admin valid update -> 200, reflected in GET /api/admin/club/trail and GET /api/club/me
    let (st, body) = send_json(
        &mut router,
        "PATCH",
        "/api/admin/club/trail",
        Some(&admin_cookie),
        None,
        None,
        Some(json!({ "trailMinutes": -1 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["trailMinutes"], json!(-1));

    let (st_get, get_body) = send_json(
        &mut router,
        "GET",
        "/api/admin/club/trail",
        Some(&admin_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_get, StatusCode::OK);
    assert_eq!(get_body["trailMinutes"], json!(-1));

    let (st_me, me_body) = send_json(
        &mut router,
        "GET",
        "/api/club/me",
        Some(&admin_cookie),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_me, StatusCode::OK);
    assert_eq!(me_body["clubTrailMinutesDefault"], json!(-1));
}

// ---------------------------------------------------------------------------
// 6. Cross-club isolation on /api/admin/map/trails and /api/display/map/trails
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn trails_endpoints_enforce_cross_club_isolation() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;

    let (club_a, _) = create_live_club(&db, "iso-a", "owner-iso-a@test").await;
    let (club_b, _) = create_live_club(&db, "iso-b", "owner-iso-b@test").await;

    db.upsert_type(&club_a, "C152").await.unwrap();
    db.upsert_aircraft(&club_a, "CC-AAA", "C152", true)
        .await
        .unwrap();
    db.upsert_type(&club_b, "C172").await.unwrap();
    db.upsert_aircraft(&club_b, "CC-BBB", "C172", true)
        .await
        .unwrap();

    let (_admin_a, admin_a_cookie) =
        seed_member(&db, &club_a, "Admin A", "adm-iso-a@test", "admin").await;
    let (pilot_a, pilot_a_cookie) =
        seed_member(&db, &club_a, "Pilot A", "pil-iso-a@test", "pilot").await;
    let (pilot_b, pilot_b_cookie) =
        seed_member(&db, &club_b, "Pilot B", "pil-iso-b@test", "pilot").await;

    db.add_checkout(&pilot_a, "C152", None, None).await.unwrap();
    db.add_checkout(&pilot_b, "C172", None, None).await.unwrap();
    let (dev_a, tok_a) = db.create_device(&pilot_a, "dev-a").await.unwrap();
    let (dev_b, tok_b) = db.create_device(&pilot_b, "dev-b").await.unwrap();

    let (st_a, res_a) = checkin_and_start_flight(
        &mut router,
        &db,
        &club_a,
        &pilot_a_cookie,
        &tok_a,
        "CC-AAA",
        Some(json!(10)),
    )
    .await;
    assert_eq!(st_a, StatusCode::OK, "{res_a}");
    let fid_a = res_a["id"].as_str().unwrap().to_string();

    let (st_b, res_b) = checkin_and_start_flight(
        &mut router,
        &db,
        &club_b,
        &pilot_b_cookie,
        &tok_b,
        "CC-BBB",
        Some(json!(10)),
    )
    .await;
    assert_eq!(st_b, StatusCode::OK, "{res_b}");
    let fid_b = res_b["id"].as_str().unwrap().to_string();

    let latest = now_ms();
    seed_fixes_minutes_back(&db, &fid_a, &dev_a, latest, &[5, 2, 0]).await;
    seed_fixes_minutes_back(&db, &fid_b, &dev_b, latest, &[5, 2, 0]).await;

    // Both /api/admin/map/trails and /api/display/map/trails for Club A admin must return ONLY fid_a, never fid_b
    for endpoint in ["/api/admin/map/trails", "/api/display/map/trails"] {
        let (st, body) = send_json(
            &mut router,
            "GET",
            endpoint,
            Some(&admin_a_cookie),
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{endpoint}: {body}");
        let trails = body["trails"].as_array().unwrap();
        let ids: Vec<&str> = trails
            .iter()
            .filter_map(|t| t["flightId"].as_str())
            .collect();
        assert!(
            ids.contains(&fid_a.as_str()),
            "{endpoint} missing club A flight"
        );
        assert!(
            !ids.contains(&fid_b.as_str()),
            "{endpoint} leaked club B flight {fid_b}!"
        );
    }
}
