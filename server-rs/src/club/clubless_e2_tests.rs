//! PR E2 integration & security tests:
//! (a) Public live-trails & `/api/me/live-trails?layers=public` club-less exposure & L-76 fail-closed AGL floor
//! (b) `tail_text` matching a real club aircraft tail: source disambiguation, zero aircraft linkage, club-reader isolation
//! (c) Owner tools for club-less flights (`/api/me/flights`, fixes, GPX, stream, shares + public share view) and isolation
//! (d) Account deletion (`DELETE /api/account`): full personal cleanup, `LiveEngine` purge, membership 409 guard,
//!     detached club retention, re-auth/CSRF/Content-Type/rate-limit enforcement
//! (e) Batch ingest (`POST /api/ingest`): 100 OK, 101 -> 413 atomic, dedup counts, out-of-order + late backfill
//!     monotonic current position, 429 + Retry-After at 120/min cap, malformed atomic reject, club & club-less.

use crate::{
    auth::Auth,
    build_router,
    club::{
        auth::{hash_password, new_id},
        db::now_ms,
        db_account::{
            ACCOUNT_PASSWORD_RECENT_AUTH_MS, ACCOUNT_SESSION_AUTH_MAGIC,
            ACCOUNT_SESSION_AUTH_PASSWORD,
        },
        mail::{MailOutbox, Mailer},
        map::{TAIL_SOURCE_CLUB, TAIL_SOURCE_PILOT},
        map_filter::DEFAULT_PUBLIC_ALTITUDE_MIN_FT,
        pilot_live_trails_tests::FlightDataEnvGuard,
        sse_limit::SseLimiter,
        visibility::FlightVisibility,
        ClubDb,
    },
    device_auth::DeviceAuth,
    live::{
        LiveEngine, LiveFix, MAX_CLUB_INGEST_FIXES_PER_MINUTE, MAX_INGEST_BATCH_FIXES,
        PRE_START_SKEW_MS,
    },
    store::Store,
    AppState, LoginThrottle, OverflowPolicy, ACCOUNT_PASSWORD_MAX_ATTEMPTS,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, Request, StatusCode},
};
use futures_util::StreamExt as _;
use serde_json::{json, Value};
use serial_test::serial;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
};
use tower::Service;

const ACCOUNT_SESSION_COOKIE: &str = "fv_account_session";

struct E2Harness {
    _dir: tempfile::TempDir,
    db: ClubDb,
    live: Arc<LiveEngine>,
    clock_ms: Arc<AtomicI64>,
    router: axum::Router,
}

async fn setup_e2() -> E2Harness {
    let dir = tempfile::tempdir().unwrap();
    let dist = dir.path().join("dist");
    std::fs::create_dir_all(&dist).unwrap();
    std::fs::write(
        dist.join("share-flight.html"),
        "<!doctype html><html><body>share</body></html>",
    )
    .unwrap();
    let locales = crate::i18n::Catalogs::resolve_dir(std::path::Path::new("dist"));
    let _ = crate::i18n::init_global(&locales);
    let db = ClubDb::open_test().await.expect("club db");
    let clock_ms = Arc::new(AtomicI64::new(now_ms()));
    let clock_clone = clock_ms.clone();
    let live = Arc::new(LiveEngine::with_clock(
        dir.path(),
        Arc::new(move || clock_clone.load(Ordering::SeqCst)),
    ));
    let state = AppState {
        dist_dir: dist,
        auth: Auth::with_password("legacy-secret"),
        device_auth: DeviceAuth::empty(),
        live: live.clone(),
        store: Store::new(dir.path()),
        club: Some(db.clone()),
        video_queue: tokio::sync::mpsc::channel(1).0,
        login_throttle: LoginThrottle::default(),
        device_register_throttle: LoginThrottle::default(),
        share_lookup_throttle: LoginThrottle::default(),
        account_password_throttle: LoginThrottle::default(),
        account_password_account_throttle: LoginThrottle::default(),
        public_live_trails_throttle: LoginThrottle::with_policy(OverflowPolicy::RejectNew),
        mailer: Mailer::from_env(MailOutbox::default()),
        webauthn: None,
        sse_limiter: Arc::new(SseLimiter::default()),
        superadmin_emails: Default::default(),
    };
    let router = build_router(state);
    E2Harness {
        _dir: dir,
        db,
        live,
        clock_ms,
        router,
    }
}

fn make_flat_dem(tiles: &[(&str, f64)]) -> (tempfile::TempDir, FlightDataEnvGuard) {
    let dem_root = tempfile::tempdir().unwrap();
    let dem_dir = dem_root.path().join("dem");
    std::fs::create_dir_all(&dem_dir).unwrap();
    for (tile, elev_m) in tiles {
        std::fs::write(
            dem_dir.join(format!("{tile}.asc")),
            format!(
                "# {elev_m} m flat terrain\n{elev_m} {elev_m} {elev_m}\n{elev_m} {elev_m} {elev_m}\n{elev_m} {elev_m} {elev_m}\n"
            ),
        )
        .unwrap();
    }
    let guard = FlightDataEnvGuard::set(dem_root.path());
    (dem_root, guard)
}

#[allow(clippy::too_many_arguments)]
async fn send_raw(
    router: &mut axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    bearer: Option<&str>,
    extra_headers: &[(&str, &str)],
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    if let Some(tok) = bearer {
        b = b.header(header::AUTHORIZATION, format!("Bearer {tok}"));
    }
    if let Some(ct) = content_type {
        b = b.header(header::CONTENT_TYPE, ct);
    }
    for (k, v) in extra_headers {
        b = b.header(*k, *v);
    }
    let req = b.body(Body::from(body)).unwrap();
    let res = router.call(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec();
    (status, headers, bytes)
}

async fn send_json(
    router: &mut axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    bearer: Option<&str>,
    extra_headers: &[(&str, &str)],
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let (ct, bytes) = match body {
        Some(v) => (Some("application/json"), serde_json::to_vec(&v).unwrap()),
        None => (None, Vec::new()),
    };
    let (st, hdrs, raw) = send_raw(
        router,
        method,
        uri,
        cookie,
        bearer,
        extra_headers,
        ct,
        bytes,
    )
    .await;
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (st, hdrs, parsed)
}

async fn create_live_club(
    db: &ClubDb,
    slug: &str,
    owner_email: &str,
    icao: &str,
    lat: f64,
    lon: f64,
) -> (String, String) {
    let now = now_ms();
    let (club_id, tok) = db
        .signup_club(&format!("Club {slug}"), slug, owner_email, Some(icao), now)
        .await
        .expect("signup_club");
    db.try_consume_magic_club_verify(&tok, now)
        .await
        .expect("verify club");
    db.set_club_base_aerodrome(&club_id, icao, lat, lon)
        .await
        .expect("set base aerodrome");
    (club_id, slug.to_string())
}

async fn create_account_session(
    db: &ClubDb,
    email: &str,
    verified: bool,
    password: Option<&str>,
    auth_method: &str,
    created_at: i64,
) -> (String, String, String) {
    let now = now_ms();
    let acct_id = new_id();
    let verified_at = if verified { Some(now) } else { None };
    let pw_hash = password.map(|p| hash_password(p).expect("hash pw"));
    sqlx::query(
        "INSERT INTO accounts (id, email, password_hash, created_at, email_verified_at)
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(&acct_id)
    .bind(email)
    .bind(pw_hash.as_deref())
    .bind(now)
    .bind(verified_at)
    .execute(&db.pool)
    .await
    .unwrap();
    let raw_sess = db
        .create_account_session_auth(&acct_id, None, created_at, auth_method)
        .await
        .unwrap();
    let cookie = format!("{ACCOUNT_SESSION_COOKIE}={raw_sess}");
    (acct_id, raw_sess, cookie)
}

async fn add_club_member(
    db: &ClubDb,
    acct_id: &str,
    email: &str,
    name: &str,
    club_id: &str,
    role: &str,
) -> (String, String, String) {
    let now = now_ms();
    let pid = new_id();
    let mid = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, $3, $4, $5, '', 'approved', $6, '', $7, $7)",
    )
    .bind(&pid)
    .bind(club_id)
    .bind(acct_id)
    .bind(name)
    .bind(email)
    .bind(role)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, $4, 'active', $5)",
    )
    .bind(&mid)
    .bind(&pid)
    .bind(club_id)
    .bind(role)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let raw = db
        .create_account_session_auth(acct_id, Some(&mid), now, ACCOUNT_SESSION_AUTH_PASSWORD)
        .await
        .unwrap();
    (pid, mid, format!("{ACCOUNT_SESSION_COOKIE}={raw}"))
}

async fn seed_personal_flight(
    db: &ClubDb,
    acct_id: &str,
    tail: &str,
    visibility: FlightVisibility,
    show_pilot_name_public: bool,
    fix: LiveFix,
) -> (String, String, String, String) {
    let now = now_ms();
    let pid = db.ensure_personal_pilot(acct_id, now).await.unwrap();
    let (dev_id, dev_tok) = db.create_device(&pid, "personal-dev").await.unwrap();
    let fid = db
        .start_personal_flight_atomic(
            &pid,
            acct_id,
            &dev_id,
            tail,
            visibility,
            show_pilot_name_public,
            None,
            fix.t - 5_000,
        )
        .await
        .unwrap();
    db.append_fix(&fid, &dev_id, &fix).await.unwrap();
    (pid, dev_id, dev_tok, fid)
}

async fn seed_club_flight(
    db: &ClubDb,
    club_id: &str,
    pilot_id: &str,
    tail: &str,
    visibility: FlightVisibility,
    show_pilot_name_public: bool,
    fix: LiveFix,
) -> (String, String, String) {
    db.upsert_type(club_id, "C172").await.unwrap();
    db.upsert_aircraft(club_id, tail, "C172", true)
        .await
        .unwrap();
    db.add_checkout(pilot_id, "C172", None, None).await.unwrap();
    let (dev_id, dev_tok) = db.create_device(pilot_id, "club-dev").await.unwrap();
    db.issue_open_checkin(pilot_id, &dev_id, club_id, fix.t - 10_000)
        .await
        .unwrap();
    let fid = db
        .start_flight(
            pilot_id,
            &dev_id,
            tail,
            visibility,
            show_pilot_name_public,
            fix.t - 5_000,
        )
        .await
        .unwrap();
    db.append_fix(&fid, &dev_id, &fix).await.unwrap();
    (dev_id, dev_tok, fid)
}

// ---------------------------------------------------------------------------
// (a) Public live-trails & /api/me/live-trails?layers=public club-less rules + L-76 floor
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn public_and_me_live_trails_clubless_exposure_and_fail_closed_agl_floor() {
    let mut fx = setup_e2().await;
    let now = now_ms();
    // 600 m flat DEM ONLY on N34W071 (-34..-33, -71..-70). Positions south of -34 have no DEM.
    let (_dem_root, _dem_guard) = make_flat_dem(&[("N34W071", 600.0)]);

    let mk_fix = |lat: f64, lon: f64, alt: Option<f64>| LiveFix {
        t: now - 1_000,
        lat,
        lon,
        alt,
        alt_ref: alt.map(|_| "msl".to_string()),
        speed: Some(38.0),
        course: Some(120.0),
        accuracy: Some(5.0),
    };

    // 1. Verified, airborne (1200 m MSL over 600 m DEM = ~1968 ft AGL), public, show_name=true
    let (acct_ok, _, cookie_ok) = create_account_session(
        &fx.db,
        &format!("e2a-ok-{}@test.example", &new_id()[..6]),
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_, _, _, fid_ok_named) = seed_personal_flight(
        &fx.db,
        &acct_ok,
        "CC-CLOK1",
        FlightVisibility::Public,
        true,
        mk_fix(-33.455, -70.548, Some(1200.0)),
    )
    .await;

    // 2. Verified, airborne, public, show_name=false -> pilotName omitted
    let (acct_anon_name, _, _) = create_account_session(
        &fx.db,
        &format!("e2a-noname-{}@test.example", &new_id()[..6]),
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_, _, _, fid_ok_unnamed) = seed_personal_flight(
        &fx.db,
        &acct_anon_name,
        "CC-CLOK2",
        FlightVisibility::Public,
        false,
        mk_fix(-33.452, -70.545, Some(1200.0)),
    )
    .await;

    // 3. Verified, BELOW 300 ft AGL floor (646 m MSL over 600 m DEM = ~151 ft AGL) -> EXCLUDED
    let (acct_low, _, _) = create_account_session(
        &fx.db,
        &format!("e2a-low-{}@test.example", &new_id()[..6]),
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_, _, _, fid_below_floor) = seed_personal_flight(
        &fx.db,
        &acct_low,
        "CC-CLLOW",
        FlightVisibility::Public,
        true,
        mk_fix(-33.454, -70.549, Some(646.0)),
    )
    .await;

    // 4. Verified, NULL alt -> EXCLUDED (fail-closed)
    let (acct_null, _, _) = create_account_session(
        &fx.db,
        &format!("e2a-null-{}@test.example", &new_id()[..6]),
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_, _, _, fid_null_alt) = seed_personal_flight(
        &fx.db,
        &acct_null,
        "CC-CLNUL",
        FlightVisibility::Public,
        true,
        mk_fix(-33.453, -70.549, None),
    )
    .await;

    // 5. Verified, NO DEM tile coverage (-34.05, -70.548 ~66 km south, 3000 m MSL) -> EXCLUDED
    let (acct_nodem, _, _) = create_account_session(
        &fx.db,
        &format!("e2a-nodem-{}@test.example", &new_id()[..6]),
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_, _, _, fid_no_dem) = seed_personal_flight(
        &fx.db,
        &acct_nodem,
        "CC-CLNDM",
        FlightVisibility::Public,
        true,
        mk_fix(-34.05, -70.548, Some(3000.0)),
    )
    .await;

    // 6. Verified, OUTSIDE 20 km bbox (-33.15, -70.548 ~34 km north, same N34W071 DEM tile) -> EXCLUDED at r=20
    let (acct_far, _, _) = create_account_session(
        &fx.db,
        &format!("e2a-far-{}@test.example", &new_id()[..6]),
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_, _, _, fid_outside_bbox) = seed_personal_flight(
        &fx.db,
        &acct_far,
        "CC-CLFAR",
        FlightVisibility::Public,
        true,
        mk_fix(-33.15, -70.548, Some(1200.0)),
    )
    .await;

    // 7. Verified, PRIVATE, airborne -> EXCLUDED everywhere public
    let (acct_priv, _, _) = create_account_session(
        &fx.db,
        &format!("e2a-priv-{}@test.example", &new_id()[..6]),
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_, _, _, fid_private) = seed_personal_flight(
        &fx.db,
        &acct_priv,
        "CC-CLPRV",
        FlightVisibility::Private,
        true,
        mk_fix(-33.456, -70.548, Some(1200.0)),
    )
    .await;

    // 8. UNVERIFIED owner, public, airborne -> EXCLUDED
    let (acct_unv, _, _) = create_account_session(
        &fx.db,
        &format!("e2a-unv-{}@test.example", &new_id()[..6]),
        false,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_, _, _, fid_unverified) = seed_personal_flight(
        &fx.db,
        &acct_unv,
        "CC-CLUNV",
        FlightVisibility::Public,
        true,
        mk_fix(-33.456, -70.547, Some(1200.0)),
    )
    .await;

    // 9. SUSPENDED verified owner, public, airborne -> EXCLUDED
    let (acct_susp, _, _) = create_account_session(
        &fx.db,
        &format!("e2a-susp-{}@test.example", &new_id()[..6]),
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_, _, _, fid_suspended) = seed_personal_flight(
        &fx.db,
        &acct_susp,
        "CC-CLSUS",
        FlightVisibility::Public,
        true,
        mk_fix(-33.456, -70.546, Some(1200.0)),
    )
    .await;
    fx.db.set_account_suspended(&acct_susp, now).await.unwrap();

    // Anonymous /api/public/live-trails (radius=20 km, default 300 ft AGL floor)
    let (st, _, pub_body) = send_json(
        &mut fx.router,
        "GET",
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=20",
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{pub_body}");
    assert_eq!(
        pub_body["publicArea"]["minAglFt"],
        json!(DEFAULT_PUBLIC_ALTITUDE_MIN_FT)
    );
    let pub_flights: HashMap<&str, &Value> = pub_body["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| Some((f["flightId"].as_str()?, f)))
        .collect();

    assert!(pub_flights.contains_key(fid_ok_named.as_str()));
    assert!(pub_flights.contains_key(fid_ok_unnamed.as_str()));
    assert!(!pub_flights.contains_key(fid_below_floor.as_str()));
    assert!(!pub_flights.contains_key(fid_null_alt.as_str()));
    assert!(!pub_flights.contains_key(fid_no_dem.as_str()));
    assert!(!pub_flights.contains_key(fid_outside_bbox.as_str()));
    assert!(!pub_flights.contains_key(fid_private.as_str()));
    assert!(!pub_flights.contains_key(fid_unverified.as_str()));
    assert!(!pub_flights.contains_key(fid_suspended.as_str()));

    let named = pub_flights[fid_ok_named.as_str()];
    assert_eq!(named["tail"], json!("CC-CLOK1"));
    assert_eq!(named["tailSource"], json!(TAIL_SOURCE_PILOT));
    assert_eq!(named["layer"], json!("public"));
    assert!(named.get("clubName").is_none());
    assert!(named.get("clubSlug").is_none());
    assert!(named.get("pilotId").is_none());
    assert!(named.get("pilotName").and_then(Value::as_str).is_some());

    let unnamed = pub_flights[fid_ok_unnamed.as_str()];
    assert_eq!(unnamed["tailSource"], json!(TAIL_SOURCE_PILOT));
    assert!(unnamed.get("pilotName").is_none());

    // Anonymous /api/public/live-trails at radius=100 km: fid_outside_bbox now enters,
    // fid_no_dem STILL fails closed, min_agl_ft=0 clamps to 300 ft so fid_below_floor stays excluded.
    let (st100, _, pub100) = send_json(
        &mut fx.router,
        "GET",
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=100&min_agl_ft=0",
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st100, StatusCode::OK, "{pub100}");
    assert_eq!(pub100["publicArea"]["minAglFt"], json!(300.0));
    let ids100: HashSet<&str> = pub100["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(ids100.contains(fid_outside_bbox.as_str()));
    assert!(!ids100.contains(fid_no_dem.as_str()));
    assert!(!ids100.contains(fid_below_floor.as_str()));

    // Authenticated /api/me/live-trails?layers=public (L-76: same fail-closed 300 ft AGL floor,
    // plus own-flight exclusion when called by acct_ok, and full set when called by another viewer).
    let (_, _, viewer_cookie) = create_account_session(
        &fx.db,
        &format!("e2a-viewer-{}@test.example", &new_id()[..6]),
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (st_me, _, me_body) = send_json(
        &mut fx.router,
        "GET",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&radius_km=20&min_agl_ft=0",
        Some(&viewer_cookie),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st_me, StatusCode::OK, "{me_body}");
    assert_eq!(
        me_body["publicArea"]["minAglFt"],
        json!(DEFAULT_PUBLIC_ALTITUDE_MIN_FT)
    );
    let me_flights: HashMap<&str, &Value> = me_body["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| Some((f["flightId"].as_str()?, f)))
        .collect();
    assert!(me_flights.contains_key(fid_ok_named.as_str()));
    assert_eq!(
        me_flights[fid_ok_named.as_str()]["tailSource"],
        json!(TAIL_SOURCE_PILOT)
    );
    assert!(me_flights.contains_key(fid_ok_unnamed.as_str()));
    assert!(!me_flights.contains_key(fid_below_floor.as_str()));
    assert!(!me_flights.contains_key(fid_null_alt.as_str()));
    assert!(!me_flights.contains_key(fid_no_dem.as_str()));
    assert!(!me_flights.contains_key(fid_outside_bbox.as_str()));
    assert!(!me_flights.contains_key(fid_private.as_str()));
    assert!(!me_flights.contains_key(fid_unverified.as_str()));
    assert!(!me_flights.contains_key(fid_suspended.as_str()));

    // Own club-less flight is excluded for its owner on /api/me/live-trails?layers=public
    let (st_own, _, own_body) = send_json(
        &mut fx.router,
        "GET",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&radius_km=20",
        Some(&cookie_ok),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st_own, StatusCode::OK, "{own_body}");
    let own_ids: HashSet<&str> = own_body["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(!own_ids.contains(fid_ok_named.as_str()));
    assert!(own_ids.contains(fid_ok_unnamed.as_str()));
}

// ---------------------------------------------------------------------------
// (b) tail_text matching a real club aircraft tail: source disambiguation & club isolation
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn tail_text_equal_to_club_aircraft_is_disambiguated_and_isolated_from_club_readers() {
    let mut fx = setup_e2().await;
    let now = now_ms();
    let (_dem_root, _dem_guard) = make_flat_dem(&[("N34W071", 600.0)]);

    let (club_id, club_slug) = create_live_club(
        &fx.db,
        "e2b-club",
        "own-e2b@test.example",
        "SCTB",
        -33.456,
        -70.548,
    )
    .await;

    let (admin_acct, _, _) = create_account_session(
        &fx.db,
        "adm-e2b@test.example",
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_admin_pid, _, admin_cookie) = add_club_member(
        &fx.db,
        &admin_acct,
        "adm-e2b@test.example",
        "Club Admin",
        &club_id,
        "admin",
    )
    .await;

    let (club_pilot_acct, _, _) = create_account_session(
        &fx.db,
        "cp-e2b@test.example",
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (club_pid, _, club_pilot_cookie) = add_club_member(
        &fx.db,
        &club_pilot_acct,
        "cp-e2b@test.example",
        "Club Pilot",
        &club_id,
        "pilot",
    )
    .await;

    // Seed a real club flight with aircraft tail "CC-SAME"
    let (_, _, fid_club) = seed_club_flight(
        &fx.db,
        &club_id,
        &club_pid,
        "CC-SAME",
        FlightVisibility::Public,
        true,
        LiveFix {
            t: now - 1_000,
            lat: -33.456,
            lon: -70.548,
            alt: Some(1200.0),
            alt_ref: Some("msl".to_string()),
            speed: Some(35.0),
            course: Some(90.0),
            accuracy: Some(5.0),
        },
    )
    .await;

    // Seed a verified club-less flight with the EXACT SAME free-text tail "CC-SAME"
    let (cl_acct, _, _) = create_account_session(
        &fx.db,
        "cl-same@test.example",
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_, _, _, fid_clubless) = seed_personal_flight(
        &fx.db,
        &cl_acct,
        "CC-SAME",
        FlightVisibility::Public,
        true,
        LiveFix {
            t: now - 500,
            lat: -33.460,
            lon: -70.550,
            alt: Some(1300.0),
            alt_ref: Some("msl".to_string()),
            speed: Some(40.0),
            course: Some(270.0),
            accuracy: Some(5.0),
        },
    )
    .await;

    // Verify DB integrity: club-less flight is NEVER linked to the club aircraft row.
    let frow = fx.db.get_flight(&fid_clubless).await.unwrap();
    assert!(frow.aircraft_id.is_none());
    assert!(frow.club_id.is_none());
    assert_eq!(frow.tail, "CC-SAME");

    // 1. Anonymous /api/public/live-trails: BOTH appear separately, distinguished by tailSource
    let (st_plt, _, body_plt) = send_json(
        &mut fx.router,
        "GET",
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=20",
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st_plt, StatusCode::OK, "{body_plt}");
    let plt_rows = body_plt["flights"].as_array().unwrap();
    let r_club = plt_rows
        .iter()
        .find(|f| f["flightId"] == fid_club)
        .expect("club flight missing");
    let r_cl = plt_rows
        .iter()
        .find(|f| f["flightId"] == fid_clubless)
        .expect("club-less flight missing");
    assert_eq!(r_club["tail"], json!("CC-SAME"));
    assert_eq!(r_club["tailSource"], json!(TAIL_SOURCE_CLUB));
    assert_eq!(r_club["clubSlug"], json!(club_slug));
    assert_eq!(r_club["clubName"], json!(format!("Club {club_slug}")));
    assert_eq!(r_cl["tail"], json!("CC-SAME"));
    assert_eq!(r_cl["tailSource"], json!(TAIL_SOURCE_PILOT));
    assert!(r_cl.get("clubSlug").is_none());
    assert!(r_cl.get("clubName").is_none());

    // 2. Anonymous /api/club/map/public: BOTH appear separately (never merged in dedupe_by_tail!)
    let (st_pm, _, body_pm) = send_json(
        &mut fx.router,
        "GET",
        &format!("/api/club/map/public?club={club_slug}"),
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st_pm, StatusCode::OK, "{body_pm}");
    let pm_rows = body_pm["aircraft"].as_array().unwrap();
    assert_eq!(
        pm_rows.len(),
        2,
        "must not collapse club & pilot tails: {body_pm}"
    );
    let pm_club = pm_rows.iter().find(|a| a["flightId"] == fid_club).unwrap();
    let pm_cl = pm_rows
        .iter()
        .find(|a| a["flightId"] == fid_clubless)
        .unwrap();
    assert_eq!(pm_club["tailSource"], json!(TAIL_SOURCE_CLUB));
    assert!(pm_club.get("nonClub").is_none());
    assert!(pm_club.get("otherClub").is_none());
    assert_eq!(pm_club["clubSlug"], json!(club_slug));
    assert_eq!(pm_cl["tailSource"], json!(TAIL_SOURCE_PILOT));
    assert_eq!(pm_cl["nonClub"], json!(true));
    assert!(pm_cl.get("otherClub").is_none());
    assert!(pm_cl.get("clubSlug").is_none());
    assert!(pm_cl.get("clubName").is_none());

    // 3. Pure club readers NEVER include the club-less flight:
    //    /api/club/map, /api/me/live-trails?layers=club, and admin/display maps when nearby is OFF.
    for (uri, cookie) in [
        ("/api/club/map", admin_cookie.as_str()),
        ("/api/club/map", club_pilot_cookie.as_str()),
        ("/api/me/live-trails?layers=club", admin_cookie.as_str()),
    ] {
        let (st, _, b) = send_json(&mut fx.router, "GET", uri, Some(cookie), None, &[], None).await;
        assert_eq!(st, StatusCode::OK, "{uri}: {b}");
        assert!(
            !b.to_string().contains(&fid_clubless),
            "{uri} must never include club-less flight: {b}"
        );
    }

    // Turn off nearby public on club -> admin/display maps & trails MUST exclude club-less
    fx.db
        .set_club_nearby_public_settings(&club_id, Some(false), None)
        .await
        .unwrap();
    for uri in [
        "/api/admin/map",
        "/api/admin/map/trails",
        "/api/display/map",
        "/api/display/map/trails",
    ] {
        let (st, _, b) = send_json(
            &mut fx.router,
            "GET",
            uri,
            Some(&admin_cookie),
            None,
            &[],
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{uri}: {b}");
        assert!(
            !b.to_string().contains(&fid_clubless),
            "{uri} must exclude club-less when show_nearby_public=false: {b}"
        );
    }

    // Re-enable nearby public -> admin/display maps & trails include club-less with tailSource="pilot"
    fx.db
        .set_club_nearby_public_settings(&club_id, Some(true), Some(50))
        .await
        .unwrap();
    for (uri, field) in [
        ("/api/admin/map", "aircraft"),
        ("/api/display/map", "aircraft"),
        ("/api/admin/map/trails", "trails"),
        ("/api/display/map/trails", "trails"),
    ] {
        let (st, _, b) = send_json(
            &mut fx.router,
            "GET",
            uri,
            Some(&admin_cookie),
            None,
            &[],
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{uri}: {b}");
        let items = b[field].as_array().unwrap();
        let cl_item = items
            .iter()
            .find(|x| x["flightId"] == fid_clubless)
            .unwrap_or_else(|| panic!("{uri} missing nearby club-less flight: {b}"));
        assert_eq!(cl_item["tailSource"], json!(TAIL_SOURCE_PILOT));
        assert_eq!(cl_item["nonClub"], json!(true));
        assert!(cl_item.get("otherClub").is_none());
        assert!(cl_item.get("clubSlug").is_none());
        assert!(cl_item.get("clubName").is_none());
    }
}

// ---------------------------------------------------------------------------
// (c) Owner tools & shares for club-less flights + strict owner-only isolation
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn clubless_owner_tools_and_shares_work_for_owner_and_deny_others() {
    let mut fx = setup_e2().await;
    let now = now_ms();

    // Owner: club-less verified account with a PRIVATE flight
    let (owner_acct, _, owner_cookie) = create_account_session(
        &fx.db,
        "e2c-owner@test.example",
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_owner_pid, owner_dev, owner_bearer, fid) = seed_personal_flight(
        &fx.db,
        &owner_acct,
        "CC-MYGPX",
        FlightVisibility::Private,
        false,
        LiveFix {
            t: now - 10_000,
            lat: -33.45,
            lon: -70.55,
            alt: Some(950.0),
            alt_ref: Some("msl".to_string()),
            speed: Some(30.0),
            course: Some(45.0),
            accuracy: Some(4.0),
        },
    )
    .await;
    fx.db
        .append_fix(
            &fid,
            &owner_dev,
            &LiveFix {
                t: now - 5_000,
                lat: -33.44,
                lon: -70.54,
                alt: Some(1050.0),
                alt_ref: Some("msl".to_string()),
                speed: Some(32.0),
                course: Some(50.0),
                accuracy: Some(4.0),
            },
        )
        .await
        .unwrap();

    // Other callers: another club-less account (with & without personal pilot), a club admin, anonymous
    let (other_acct, _, other_cookie) = create_account_session(
        &fx.db,
        "e2c-other@test.example",
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let other_pid = fx.db.ensure_personal_pilot(&other_acct, now).await.unwrap();
    let (_, other_bearer) = fx.db.create_device(&other_pid, "other-dev").await.unwrap();

    let (_, _, other_nopilot_cookie) = create_account_session(
        &fx.db,
        "e2c-nopilot@test.example",
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;

    let (club_id, _) = create_live_club(
        &fx.db,
        "e2c-club",
        "own-e2c@test.example",
        "SCTB",
        -33.456,
        -70.548,
    )
    .await;
    let (adm_acct, _, _) = create_account_session(
        &fx.db,
        "e2c-adm@test.example",
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_, _, adm_cookie) = add_club_member(
        &fx.db,
        &adm_acct,
        "e2c-adm@test.example",
        "Admin",
        &club_id,
        "admin",
    )
    .await;

    // Owner succeeds with BOTH account session cookie AND personal device Bearer token
    for (cookie, bearer) in [
        (Some(owner_cookie.as_str()), None),
        (None, Some(owner_bearer.as_str())),
    ] {
        // /api/me/flights
        let (st, _, b_list) = send_json(
            &mut fx.router,
            "GET",
            "/api/me/flights",
            cookie,
            bearer,
            &[],
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{b_list}");
        let fl = b_list["flights"].as_array().unwrap();
        assert_eq!(fl.len(), 1);
        assert_eq!(fl[0]["id"], json!(fid));
        assert_eq!(fl[0]["tail"], json!("CC-MYGPX"));

        // /api/flights/{id}/fixes
        let (st, _, b_fixes) = send_json(
            &mut fx.router,
            "GET",
            &format!("/api/flights/{fid}/fixes"),
            cookie,
            bearer,
            &[],
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{b_fixes}");
        assert_eq!(b_fixes["fixes"].as_array().unwrap().len(), 2);

        // /api/flights/{id}/gpx
        let (st, hdrs, gpx_bytes) = send_raw(
            &mut fx.router,
            "GET",
            &format!("/api/flights/{fid}/gpx"),
            cookie,
            bearer,
            &[],
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            hdrs.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()),
            Some("application/gpx+xml")
        );
        let gpx_text = String::from_utf8(gpx_bytes).unwrap();
        assert!(gpx_text.contains("<name>CC-MYGPX</name>"), "{gpx_text}");
        assert!(gpx_text.contains("lat=\"-33.45\""), "{gpx_text}");
        assert!(gpx_text.contains("lat=\"-33.44\""), "{gpx_text}");

        // /api/flights/{id}/stream (SSE returns 200 text/event-stream and emits backfill)
        let mut req_b = Request::builder()
            .method("GET")
            .uri(format!("/api/flights/{fid}/stream"));
        if let Some(c) = cookie {
            req_b = req_b.header(header::COOKIE, c);
        }
        if let Some(t) = bearer {
            req_b = req_b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let stream_res = fx
            .router
            .call(req_b.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(stream_res.status(), StatusCode::OK);
        let ct = stream_res
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(ct.starts_with("text/event-stream"), "got {ct}");
        let mut body_stream = stream_res.into_body().into_data_stream();
        let first_chunk =
            tokio::time::timeout(std::time::Duration::from_secs(2), body_stream.next())
                .await
                .expect("backfill SSE chunk")
                .unwrap()
                .unwrap();
        let chunk_str = String::from_utf8_lossy(&first_chunk);
        assert!(
            chunk_str.contains("-33.45"),
            "unexpected SSE backfill: {chunk_str}"
        );
    }

    // Shares: CSRF blocked for cross-site cookie POST
    let (st_csrf, _, b_csrf) = send_json(
        &mut fx.router,
        "POST",
        &format!("/api/me/flights/{fid}/shares"),
        Some(&owner_cookie),
        None,
        &[("sec-fetch-site", "cross-site")],
        None,
    )
    .await;
    assert_eq!(st_csrf, StatusCode::FORBIDDEN, "{b_csrf}");
    assert_eq!(b_csrf["error"], json!("csrf"));

    // Owner creates a share link for club-less flight (DB club_id IS NULL)
    let (st_sh, _, b_sh) = send_json(
        &mut fx.router,
        "POST",
        &format!("/api/me/flights/{fid}/shares"),
        Some(&owner_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        None,
    )
    .await;
    assert_eq!(st_sh, StatusCode::OK, "{b_sh}");
    let share_id = b_sh["id"].as_str().unwrap().to_string();
    let share_token = b_sh["token"].as_str().unwrap().to_string();
    assert!(b_sh["url"]
        .as_str()
        .unwrap()
        .ends_with(&format!("/s/{share_token}")));
    let stored_club_id: Option<String> =
        sqlx::query_scalar("SELECT club_id FROM flight_shares WHERE id = $1")
            .bind(&share_id)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert!(
        stored_club_id.is_none(),
        "club-less share must store NULL club_id"
    );

    // Owner lists shares (via bearer too)
    let (st_ls, _, b_ls) = send_json(
        &mut fx.router,
        "GET",
        &format!("/api/me/flights/{fid}/shares"),
        None,
        Some(&owner_bearer),
        &[],
        None,
    )
    .await;
    assert_eq!(st_ls, StatusCode::OK, "{b_ls}");
    assert_eq!(b_ls["shares"].as_array().unwrap().len(), 1);
    assert_eq!(b_ls["shares"][0]["id"], json!(share_id));

    // Anonymous public share endpoints work for club-less share and carry tailSource="pilot"
    let (st_page, hdrs_page, _) = send_raw(
        &mut fx.router,
        "GET",
        &format!("/s/{share_token}"),
        None,
        None,
        &[],
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(st_page, StatusCode::OK);
    assert_eq!(
        hdrs_page
            .get(header::REFERRER_POLICY)
            .and_then(|v| v.to_str().ok()),
        Some(crate::html_csp::SENSITIVE_REFERRER_POLICY)
    );

    let (st_meta, _, b_meta) = send_json(
        &mut fx.router,
        "GET",
        &format!("/api/share/{share_token}"),
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st_meta, StatusCode::OK, "{b_meta}");
    assert_eq!(b_meta["tail"], json!("CC-MYGPX"));
    assert_eq!(b_meta["tailSource"], json!(TAIL_SOURCE_PILOT));
    assert_eq!(b_meta["status"], json!("active"));

    let (st_sf, _, b_sf) = send_json(
        &mut fx.router,
        "GET",
        &format!("/api/share/{share_token}/fixes"),
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st_sf, StatusCode::OK, "{b_sf}");
    assert_eq!(b_sf["tail"], json!("CC-MYGPX"));
    assert_eq!(b_sf["tailSource"], json!(TAIL_SOURCE_PILOT));
    assert_eq!(b_sf["fixes"].as_array().unwrap().len(), 2);

    // Public share stream returns 200 SSE with backfill
    let sh_stream_res = fx
        .router
        .call(
            Request::builder()
                .method("GET")
                .uri(format!("/api/share/{share_token}/stream"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sh_stream_res.status(), StatusCode::OK);
    let mut sh_stream = sh_stream_res.into_body().into_data_stream();
    let sh_chunk = tokio::time::timeout(std::time::Duration::from_secs(2), sh_stream.next())
        .await
        .expect("share SSE backfill")
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&sh_chunk).contains("-33.45"));

    // Denial checks: other account (cookie & bearer), other account without personal pilot,
    // club admin, and anonymous MUST all be denied on every owner tool & share management route.
    let unauthorized_actors: [(&str, Option<&str>, Option<&str>); 5] = [
        ("other_cookie", Some(&other_cookie), None),
        ("other_bearer", None, Some(&other_bearer)),
        ("other_nopilot", Some(&other_nopilot_cookie), None),
        ("club_admin", Some(&adm_cookie), None),
        ("anonymous", None, None),
    ];
    for (label, cookie, bearer) in unauthorized_actors {
        for uri in [
            format!("/api/flights/{fid}/fixes"),
            format!("/api/flights/{fid}/gpx"),
            format!("/api/flights/{fid}/stream"),
        ] {
            let (st, _, _) = send_raw(
                &mut fx.router,
                "GET",
                &uri,
                cookie,
                bearer,
                &[],
                None,
                Vec::new(),
            )
            .await;
            assert_eq!(
                st,
                StatusCode::NOT_FOUND,
                "{label} must get 404 (no oracle) on {uri}, got {st}"
            );
        }

        let expected_share_status = if label == "anonymous" {
            StatusCode::UNAUTHORIZED
        } else {
            StatusCode::NOT_FOUND
        };
        for (method, uri) in [
            ("GET", format!("/api/me/flights/{fid}/shares")),
            ("POST", format!("/api/me/flights/{fid}/shares")),
            ("DELETE", format!("/api/me/flights/{fid}/shares/{share_id}")),
        ] {
            let (st, _, b) =
                send_json(&mut fx.router, method, &uri, cookie, bearer, &[], None).await;
            assert_eq!(
                st, expected_share_status,
                "{label} on {method} {uri} expected {expected_share_status}, got {st} ({b})"
            );
        }
    }

    // Suspending the owner immediately cuts off public share access (404), unsuspending restores it.
    fx.db.set_account_suspended(&owner_acct, now).await.unwrap();
    let (st_susp, _, _) = send_json(
        &mut fx.router,
        "GET",
        &format!("/api/share/{share_token}"),
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st_susp, StatusCode::NOT_FOUND);
    fx.db.clear_account_suspended(&owner_acct).await.unwrap();

    // Owner revokes share via personal bearer -> 200 OK; share token is now 404
    let (st_rev, _, b_rev) = send_json(
        &mut fx.router,
        "DELETE",
        &format!("/api/me/flights/{fid}/shares/{share_id}"),
        None,
        Some(&owner_bearer),
        &[],
        None,
    )
    .await;
    assert_eq!(st_rev, StatusCode::OK, "{b_rev}");
    let (st_after, _, _) = send_json(
        &mut fx.router,
        "GET",
        &format!("/api/share/{share_token}"),
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st_after, StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// (d) Account deletion: cleanup, LiveEngine purge, memberships 409, retention, security
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn account_deletion_clubless_cleanup_live_purge_memberships_and_security() {
    let mut fx = setup_e2().await;
    let now = now_ms();

    // 1. Club-less account with password + personal pilot + device + flight + fixes + share + magic token + LiveEngine session
    let email = format!("e2d-del-{}@test.example", &new_id()[..6]);
    let pw = "CorrectHorseBattery99!";
    let stale_time = now - ACCOUNT_PASSWORD_RECENT_AUTH_MS - 60_000;
    let (acct_id, _raw_sess, stale_pw_cookie) = create_account_session(
        &fx.db,
        &email,
        true,
        Some(pw),
        ACCOUNT_SESSION_AUTH_PASSWORD,
        stale_time,
    )
    .await;
    let (pid, dev_id, dev_tok, fid) = seed_personal_flight(
        &fx.db,
        &acct_id,
        "CC-DEL01",
        FlightVisibility::Public,
        true,
        LiveFix {
            t: now - 2_000,
            lat: -33.45,
            lon: -70.55,
            alt: Some(1200.0),
            alt_ref: Some("msl".to_string()),
            speed: Some(30.0),
            course: Some(0.0),
            accuracy: Some(5.0),
        },
    )
    .await;
    let (_share_row, share_tok) = fx.db.create_flight_share(&pid, &fid).await.unwrap();
    let _magic_tok = fx
        .db
        .issue_account_magic_login(&email, now)
        .await
        .unwrap()
        .unwrap();

    // Seed an active in-memory LiveEngine session for this device and subscribe an SSE receiver
    let live_key = format!("club:personal:{dev_id}");
    fx.live
        .ingest_fixes(
            &live_key,
            vec![LiveFix {
                t: now - 1_000,
                lat: -33.45,
                lon: -70.55,
                alt: Some(1200.0),
                alt_ref: None,
                speed: None,
                course: None,
                accuracy: None,
            }],
        )
        .await
        .unwrap();
    let active_sessions = fx.live.list_sessions().await;
    let live_sess_id = active_sessions
        .iter()
        .find(|s| s.device == live_key)
        .expect("live session created")
        .id
        .clone();
    let mut live_rx = fx
        .live
        .stream_snapshot(&live_sess_id)
        .await
        .unwrap()
        .live_rx
        .expect("active live broadcast receiver");

    // Security gate checks:
    // - Anonymous -> 401
    let (st, _, _) = send_json(
        &mut fx.router,
        "DELETE",
        "/api/account",
        None,
        None,
        &[],
        Some(json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // - Non-JSON Content-Type -> 415
    let (st, _, _) = send_raw(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&stale_pw_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        Some("text/plain"),
        b"{}".to_vec(),
    )
    .await;
    assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // - Cross-site CSRF -> 403
    let (st, _, b) = send_json(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&stale_pw_cookie),
        None,
        &[("sec-fetch-site", "cross-site")],
        Some(json!({ "password": pw })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{b}");
    assert_eq!(b["error"], json!("csrf"));

    // - Stale password session with no password -> 401 auth required
    let (st, _, b) = send_json(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&stale_pw_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        Some(json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{b}");
    assert_eq!(b["error"], json!("auth required"));

    // - Wrong password -> 401 wrong password
    let (st, _, b) = send_json(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&stale_pw_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        Some(json!({ "password": "wrong-password-123" })),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{b}");
    assert_eq!(b["error"], json!("wrong password"));
    assert!(fx.db.account_by_id(&acct_id).await.is_some());

    // - Valid password -> 200 OK, clears cookies, deletes everything in one transaction, purges LiveEngine!
    let (st, hdrs, b) = send_json(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&stale_pw_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        Some(json!({ "password": pw })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(b, json!({ "ok": true }));
    let set_cookies: Vec<&str> = hdrs
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(
        set_cookies
            .iter()
            .any(|c| c.starts_with(&format!("{ACCOUNT_SESSION_COOKIE}=;"))),
        "must clear account session cookie: {set_cookies:?}"
    );

    // Verify all personal rows are gone
    assert!(fx.db.account_by_id(&acct_id).await.is_none());
    let pilot_cnt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pilots WHERE id = $1")
        .bind(&pid)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    let dev_cnt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM devices WHERE id = $1")
        .bind(&dev_id)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    let flight_cnt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM flights WHERE id = $1")
        .bind(&fid)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    let fix_cnt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM fixes WHERE flight_id = $1")
        .bind(&fid)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    let share_cnt: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM flight_shares WHERE flight_id = $1")
            .bind(&fid)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    let sess_cnt: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM account_sessions WHERE account_id = $1")
            .bind(&acct_id)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    let magic_cnt: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM magic_login_tokens WHERE account_id = $1 OR lower(email) = lower($2)",
    )
    .bind(&acct_id)
    .bind(&email)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(
        (pilot_cnt, dev_cnt, flight_cnt, fix_cnt, share_cnt, sess_cnt, magic_cnt),
        (0, 0, 0, 0, 0, 0, 0)
    );

    // Tokens & sessions immediately unusable
    let (st_me, _, _) = send_json(
        &mut fx.router,
        "GET",
        "/api/account/me",
        Some(&stale_pw_cookie),
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st_me, StatusCode::UNAUTHORIZED);
    let (st_ing, _, _) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&dev_tok),
        &[],
        Some(json!({ "fixes": [{ "t": now, "lat": -33.45, "lon": -70.55 }] })),
    )
    .await;
    assert_eq!(st_ing, StatusCode::UNAUTHORIZED);
    let (st_sh, _, _) = send_json(
        &mut fx.router,
        "GET",
        &format!("/api/share/{share_tok}"),
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st_sh, StatusCode::NOT_FOUND);

    // LiveEngine live session was terminated and purged
    let end_ev = live_rx
        .try_recv()
        .expect("LiveEngine must broadcast End on purge");
    assert!(matches!(end_ev, crate::live::SseSessionEvent::End));
    assert!(
        !fx.live
            .list_sessions()
            .await
            .iter()
            .any(|s| s.id == live_sess_id),
        "LiveEngine must purge deleted device session"
    );

    // 2. Passwordless account: stale magic session -> 401 auth required; fresh magic session -> 200 OK
    let (pl_acct, _, stale_pl_cookie) = create_account_session(
        &fx.db,
        &format!("e2d-pl-{}@test.example", &new_id()[..6]),
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        stale_time,
    )
    .await;
    let (st_stale_pl, _, b_stale_pl) = send_json(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&stale_pl_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        Some(json!({})),
    )
    .await;
    assert_eq!(st_stale_pl, StatusCode::UNAUTHORIZED, "{b_stale_pl}");
    assert_eq!(b_stale_pl["error"], json!("auth required"));

    let fresh_pl_raw = fx
        .db
        .create_account_session_auth(&pl_acct, None, now, ACCOUNT_SESSION_AUTH_MAGIC)
        .await
        .unwrap();
    let fresh_pl_cookie = format!("{ACCOUNT_SESSION_COOKIE}={fresh_pl_raw}");
    let (st_fresh_pl, _, b_fresh_pl) = send_json(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&fresh_pl_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        Some(json!({})),
    )
    .await;
    assert_eq!(st_fresh_pl, StatusCode::OK, "{b_fresh_pl}");
    assert!(fx.db.account_by_id(&pl_acct).await.is_none());

    // 3. Account holding club memberships -> 409 {"error":"leave_clubs_first","memberships":[...]},
    //    club data untouched; after leaving club, account deletion detaches club records (never deletes them).
    let (club_id, club_slug) = create_live_club(
        &fx.db,
        "e2d-club",
        "own-e2d@test.example",
        "SCTB",
        -33.456,
        -70.548,
    )
    .await;
    let mem_email = format!("e2d-mem-{}@test.example", &new_id()[..6]);
    let (mem_acct, _, _) = create_account_session(
        &fx.db,
        &mem_email,
        true,
        Some(pw),
        ACCOUNT_SESSION_AUTH_PASSWORD,
        now,
    )
    .await;
    let (club_pid, mem_id, mem_cookie) = add_club_member(
        &fx.db,
        &mem_acct,
        &mem_email,
        "Club Member",
        &club_id,
        "pilot",
    )
    .await;
    let (_, _, club_fid) = seed_club_flight(
        &fx.db,
        &club_id,
        &club_pid,
        "CC-KEEP1",
        FlightVisibility::Club,
        false,
        LiveFix {
            t: now - 3_000,
            lat: -33.45,
            lon: -70.55,
            alt: Some(1100.0),
            alt_ref: Some("msl".to_string()),
            speed: Some(30.0),
            course: Some(0.0),
            accuracy: Some(5.0),
        },
    )
    .await;
    // Also ensure owner_account_id is set on the club flight to prove FK detachment works
    sqlx::query("UPDATE flights SET owner_account_id = $1 WHERE id = $2")
        .bind(&mem_acct)
        .bind(&club_fid)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    let (_, _, _, personal_fid) = seed_personal_flight(
        &fx.db,
        &mem_acct,
        "CC-DROP1",
        FlightVisibility::Private,
        false,
        LiveFix {
            t: now - 2_000,
            lat: -33.45,
            lon: -70.55,
            alt: Some(1100.0),
            alt_ref: Some("msl".to_string()),
            speed: Some(30.0),
            course: Some(0.0),
            accuracy: Some(5.0),
        },
    )
    .await;

    let (st_409, _, b_409) = send_json(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&mem_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        Some(json!({ "password": pw })),
    )
    .await;
    assert_eq!(st_409, StatusCode::CONFLICT, "{b_409}");
    assert_eq!(b_409["error"], json!("leave_clubs_first"));
    let ms = b_409["memberships"].as_array().unwrap();
    assert_eq!(ms.len(), 1);
    assert_eq!(ms[0]["id"], json!(mem_id));
    assert_eq!(ms[0]["slug"], json!(club_slug));
    // Nothing deleted!
    assert!(fx.db.account_by_id(&mem_acct).await.is_some());
    assert!(fx.db.get_flight(&club_fid).await.is_some());
    assert!(fx.db.get_flight(&personal_fid).await.is_some());

    // Remove the club membership row (leaving historical club pilot + club flight behind) ->
    // now DELETE /api/account succeeds, deletes personal flight & account, and RETAINS club flight/pilot!
    sqlx::query("DELETE FROM club_memberships WHERE id = $1")
        .bind(&mem_id)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    let (st_ok_after, _, b_ok_after) = send_json(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&mem_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        Some(json!({ "password": pw })),
    )
    .await;
    assert_eq!(st_ok_after, StatusCode::OK, "{b_ok_after}");
    assert!(fx.db.account_by_id(&mem_acct).await.is_none());
    assert!(
        fx.db.get_flight(&personal_fid).await.is_none(),
        "personal club-less flight must be deleted"
    );
    let kept_club_flight = fx
        .db
        .get_flight(&club_fid)
        .await
        .expect("club-owned flight MUST be retained");
    assert_eq!(kept_club_flight.tail, "CC-KEEP1");
    assert_eq!(fx.db.fixes_for_flight(&club_fid).await.len(), 1);
    let detached_owner: Option<String> =
        sqlx::query_scalar("SELECT owner_account_id FROM flights WHERE id = $1")
            .bind(&club_fid)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert!(detached_owner.is_none());
    let detached_pilot_acct: Option<String> =
        sqlx::query_scalar("SELECT account_id FROM pilots WHERE id = $1")
            .bind(&club_pid)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert!(detached_pilot_acct.is_none());
}

#[tokio::test]
#[serial]
async fn account_deletion_enforces_rate_limit_with_retry_after() {
    let mut fx = setup_e2().await;
    let now = now_ms();
    let (_, _, cookie) = create_account_session(
        &fx.db,
        "e2d-rl@test.example",
        true,
        Some("ValidPassword123!"),
        ACCOUNT_SESSION_AUTH_PASSWORD,
        now,
    )
    .await;

    let mut hit_429 = false;
    for _ in 0..=ACCOUNT_PASSWORD_MAX_ATTEMPTS {
        let (st, hdrs, b) = send_json(
            &mut fx.router,
            "DELETE",
            "/api/account",
            Some(&cookie),
            None,
            &[("sec-fetch-site", "same-origin")],
            Some(json!({ "password": "wrong-password" })),
        )
        .await;
        if st == StatusCode::TOO_MANY_REQUESTS {
            assert_eq!(b["error"], json!("rate limit"));
            let ra: u64 = hdrs
                .get(header::RETRY_AFTER)
                .expect("Retry-After required on 429")
                .to_str()
                .unwrap()
                .parse()
                .unwrap();
            assert!((1..=60).contains(&ra));
            hit_429 = true;
            break;
        }
        assert_eq!(st, StatusCode::UNAUTHORIZED);
    }
    assert!(
        hit_429,
        "expected 429 rate limit on repeated DELETE /api/account"
    );
}

// ---------------------------------------------------------------------------
// (e) Batch ingest hardening: limits (100/101), dedup, out-of-order, late backfill,
//     monotonic position, 429 + Retry-After, malformed atomic reject, club & club-less
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn batch_ingest_limits_dedup_ordering_monotonic_position_and_rate_limit_club_and_clubless() {
    let mut fx = setup_e2().await;
    let t0 = now_ms();
    fx.clock_ms.store(t0, Ordering::SeqCst);

    // Setup 1: Club-less flight
    let (cl_acct, _, cl_cookie) = create_account_session(
        &fx.db,
        "e2e-cl@test.example",
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        t0,
    )
    .await;
    let cl_pid = fx.db.ensure_personal_pilot(&cl_acct, t0).await.unwrap();
    let (cl_dev, cl_tok) = fx.db.create_device(&cl_pid, "cl-phone").await.unwrap();
    let cl_fid = fx
        .db
        .start_personal_flight_atomic(
            &cl_pid,
            &cl_acct,
            &cl_dev,
            "CC-BCH1",
            FlightVisibility::Private,
            false,
            None,
            t0 - 600_000,
        )
        .await
        .unwrap();

    // Setup 2: Club flight
    let (club_id, _) = create_live_club(
        &fx.db,
        "e2e-club",
        "own-e2e@test.example",
        "SCTB",
        -33.456,
        -70.548,
    )
    .await;
    fx.db.upsert_type(&club_id, "C172").await.unwrap();
    fx.db
        .upsert_aircraft(&club_id, "CC-BCH2", "C172", true)
        .await
        .unwrap();
    let (c_acct, _, _) = create_account_session(
        &fx.db,
        "e2e-clubp@test.example",
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        t0,
    )
    .await;
    let (c_pid, _, c_cookie) = add_club_member(
        &fx.db,
        &c_acct,
        "e2e-clubp@test.example",
        "Club Pilot",
        &club_id,
        "pilot",
    )
    .await;
    fx.db
        .add_checkout(&c_pid, "C172", None, None)
        .await
        .unwrap();
    let (c_dev, c_tok) = fx.db.create_device(&c_pid, "club-phone").await.unwrap();
    fx.db
        .issue_open_checkin(&c_pid, &c_dev, &club_id, t0 - 600_000)
        .await
        .unwrap();
    let c_fid = fx
        .db
        .start_flight(
            &c_pid,
            &c_dev,
            "CC-BCH2",
            FlightVisibility::Club,
            false,
            t0 - 600_000,
        )
        .await
        .unwrap();

    // Run identical hardening matrix for BOTH club-less and club flights!
    for (kind, pid, dev_id, tok, cookie, fid) in [
        ("clubless", &cl_pid, &cl_dev, &cl_tok, &cl_cookie, &cl_fid),
        ("club", &c_pid, &c_dev, &c_tok, &c_cookie, &c_fid),
    ] {
        let base_t = t0 - 300_000;
        fx.clock_ms.store(t0, Ordering::SeqCst);

        // 1. 101 fixes -> 413 {"error":"batch too large","max":100}, 0 DB writes, 0 rate-limit hits burned
        let batch_101: Vec<Value> = (0..=MAX_INGEST_BATCH_FIXES)
            .map(|i| json!({ "t": base_t + i as i64 * 100, "lat": -33.45, "lon": -70.55, "alt": 900.0 }))
            .collect();
        let (st, _, b) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({ "fixes": batch_101 })),
        )
        .await;
        assert_eq!(st, StatusCode::PAYLOAD_TOO_LARGE, "{kind}: {b}");
        assert_eq!(
            b,
            json!({ "error": "batch too large", "max": MAX_INGEST_BATCH_FIXES })
        );
        assert_eq!(fx.db.fixes_for_flight(fid).await.len(), 0);

        // 2. Malformed fix inside batch -> 400, atomic reject (valid first fix NOT written)
        let (st_bad, _, _) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({
                "fixes": [
                    { "t": base_t + 1000, "lat": -33.45, "lon": -70.55, "alt": 900.0 },
                    { "t": base_t + 2000, "lat": 95.0, "lon": -70.55, "alt": 900.0 }
                ]
            })),
        )
        .await;
        assert_eq!(st_bad, StatusCode::BAD_REQUEST, "{kind}");
        assert_eq!(fx.db.fixes_for_flight(fid).await.len(), 0);

        // 3. Out-of-order batch of 5 fixes -> stored in ascending order, current position = max timestamp
        let ooo_fixes = json!({
            "fixes": [
                { "t": base_t + 30_000, "lat": -33.43, "lon": -70.53, "alt": 930.0 },
                { "t": base_t + 10_000, "lat": -33.41, "lon": -70.51, "alt": 910.0 },
                { "t": base_t + 50_000, "lat": -33.45, "lon": -70.55, "alt": 950.0 },
                { "t": base_t + 20_000, "lat": -33.42, "lon": -70.52, "alt": 920.0 },
                { "t": base_t + 40_000, "lat": -33.44, "lon": -70.54, "alt": 940.0 }
            ]
        });
        let (st_ooo, _, b_ooo) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(ooo_fixes.clone()),
        )
        .await;
        assert_eq!(st_ooo, StatusCode::OK, "{kind}: {b_ooo}");
        assert_eq!(
            b_ooo,
            json!({ "ok": true, "accepted": 5, "duplicates": 0, "droppedPreStart": 0 })
        );

        let frow = fx.db.get_flight(fid).await.unwrap();
        assert_eq!(frow.last_fix_at, Some(base_t + 50_000), "{kind}");
        let (p_lat, p_lon, p_pos_t): (Option<f64>, Option<f64>, Option<i64>) = sqlx::query_as(
            "SELECT last_latitude, last_longitude, last_position_at FROM pilots WHERE id = $1",
        )
        .bind(pid)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
        assert_eq!(p_pos_t, Some(base_t + 50_000), "{kind}");
        assert_eq!(p_lat, Some(-33.45), "{kind}");
        assert_eq!(p_lon, Some(-70.55), "{kind}");

        // 4. Duplicate batch replay -> 200 {"ok":true,"accepted":0,"duplicates":5,"droppedPreStart":0}, no duplicate rows
        let (st_dup, _, b_dup) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(ooo_fixes),
        )
        .await;
        assert_eq!(st_dup, StatusCode::OK, "{kind}: {b_dup}");
        assert_eq!(
            b_dup,
            json!({ "ok": true, "accepted": 0, "duplicates": 5, "droppedPreStart": 0 })
        );
        assert_eq!(fx.db.fixes_for_flight(fid).await.len(), 5);

        // 5. Live newer fix at base_t + 100s, followed by LATE buffered batch at base_t + [60s, 70s, 80s]
        //    + 1 duplicate (base_t + 50s): late fixes inserted in time order, current position NEVER moves back!
        let (st_live, _, b_live) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({
                "fixes": [{ "t": base_t + 100_000, "lat": -33.30, "lon": -70.40, "alt": 1400.0 }]
            })),
        )
        .await;
        assert_eq!(st_live, StatusCode::OK, "{kind}: {b_live}");
        assert_eq!(
            b_live,
            json!({ "ok": true, "accepted": 1, "duplicates": 0, "droppedPreStart": 0 })
        );

        let (st_late, _, b_late) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({
                "fixes": [
                    { "t": base_t + 80_000, "lat": -33.48, "lon": -70.58, "alt": 1100.0 },
                    { "t": base_t + 50_000, "lat": -33.45, "lon": -70.55, "alt": 950.0 },
                    { "t": base_t + 60_000, "lat": -33.46, "lon": -70.56, "alt": 1000.0 },
                    { "t": base_t + 70_000, "lat": -33.47, "lon": -70.57, "alt": 1050.0 }
                ]
            })),
        )
        .await;
        assert_eq!(st_late, StatusCode::OK, "{kind}: {b_late}");
        assert_eq!(
            b_late,
            json!({ "ok": true, "accepted": 3, "duplicates": 1, "droppedPreStart": 0 })
        );

        // Verify /fixes and /gpx are strictly ascending and latest position stayed at base_t + 100s
        let (st_gf, _, b_gf) = send_json(
            &mut fx.router,
            "GET",
            &format!("/api/flights/{fid}/fixes"),
            Some(cookie),
            None,
            &[],
            None,
        )
        .await;
        assert_eq!(st_gf, StatusCode::OK, "{kind}: {b_gf}");
        let ts: Vec<i64> = b_gf["fixes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["t"].as_i64().unwrap())
            .collect();
        assert_eq!(
            ts,
            vec![
                base_t + 10_000,
                base_t + 20_000,
                base_t + 30_000,
                base_t + 40_000,
                base_t + 50_000,
                base_t + 60_000,
                base_t + 70_000,
                base_t + 80_000,
                base_t + 100_000,
            ],
            "{kind} fixes must be strictly time-ordered"
        );
        let frow_after = fx.db.get_flight(fid).await.unwrap();
        assert_eq!(
            frow_after.last_fix_at,
            Some(base_t + 100_000),
            "{kind}: late batch must not move flights.last_fix_at backwards"
        );
        let (p_lat2, p_lon2, p_pos_t2): (Option<f64>, Option<f64>, Option<i64>) = sqlx::query_as(
            "SELECT last_latitude, last_longitude, last_position_at FROM pilots WHERE id = $1",
        )
        .bind(pid)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
        assert_eq!(
            (p_lat2, p_lon2, p_pos_t2),
            (Some(-33.30), Some(-70.40), Some(base_t + 100_000)),
            "{kind}: late batch must not move pilot position backwards"
        );
        // Latest device fix on map also equals the max-timestamp fix!
        let latest_all = fx.db.latest_device_fixes().await;
        let mine = latest_all
            .iter()
            .find(|r| r.flight_id == *fid)
            .expect("active flight present in latest_device_fixes");
        assert_eq!(mine.fix.t, base_t + 100_000);
        assert_eq!(mine.fix.lat, -33.30);

        // 6. Advance window cleanly, test exact 100-fix batch OK, + 20 = 120 OK, then 5-fix batch -> 429 + Retry-After (atomic!)
        let t_win = t0 + 120_000;
        fx.clock_ms.store(t_win, Ordering::SeqCst);
        let batch_100: Vec<Value> = (0..MAX_INGEST_BATCH_FIXES)
            .map(|i| json!({ "t": t_win - 50_000 + i as i64 * 100, "lat": -33.31, "lon": -70.41, "alt": 1410.0 }))
            .collect();
        let (st_100, _, b_100) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({ "fixes": batch_100 })),
        )
        .await;
        assert_eq!(st_100, StatusCode::OK, "{kind}: {b_100}");
        assert_eq!(
            b_100,
            json!({
                "ok": true,
                "accepted": MAX_INGEST_BATCH_FIXES,
                "duplicates": 0,
                "droppedPreStart": 0,
            })
        );

        let rem = MAX_CLUB_INGEST_FIXES_PER_MINUTE - MAX_INGEST_BATCH_FIXES;
        let batch_20: Vec<Value> = (0..rem)
            .map(|i| json!({ "t": t_win - 30_000 + i as i64 * 100, "lat": -33.32, "lon": -70.42, "alt": 1420.0 }))
            .collect();
        let (st_20, _, b_20) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({ "fixes": batch_20 })),
        )
        .await;
        assert_eq!(st_20, StatusCode::OK, "{kind}: {b_20}");
        assert_eq!(
            b_20,
            json!({
                "ok": true,
                "accepted": rem,
                "duplicates": 0,
                "droppedPreStart": 0,
            })
        );

        let before_429_count = fx.db.fixes_for_flight(fid).await.len();
        let batch_over: Vec<Value> = (0..5)
            .map(|i| json!({ "t": t_win - 10_000 + i as i64 * 100, "lat": -33.33, "lon": -70.43, "alt": 1430.0 }))
            .collect();
        let (st_429, hdrs_429, b_429) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({ "fixes": batch_over })),
        )
        .await;
        assert_eq!(st_429, StatusCode::TOO_MANY_REQUESTS, "{kind}: {b_429}");
        assert_eq!(b_429["error"], json!("rate limited"));
        let ra: u64 = hdrs_429
            .get(header::RETRY_AFTER)
            .expect("Retry-After header required on 429")
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=60).contains(&ra), "{kind}: Retry-After={ra}");
        assert_eq!(
            fx.db.fixes_for_flight(fid).await.len(),
            before_429_count,
            "{kind}: 429 must reject atomically with zero partial writes"
        );
        assert_eq!(
            fx.db.open_flight_for_device(dev_id).await.as_deref(),
            Some(fid.as_str()),
            "{kind}: 429 must keep flight active"
        );
    }
}

// ---------------------------------------------------------------------------
// (f) H-8 flight binding & pre-start skew filtering on /api/ingest and /api/osmand
//     (club & club-less): 409 flight_mismatch oracle-free & zero writes/cap/broadcast,
//     pre-start drop (started_at - 120_000 ms), cap interaction, seq validation,
//     single & batch forms, camelCase & snake_case, backward-compatible legacy clients
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn ingest_h8_flight_binding_pre_start_skew_seq_and_osmand_club_and_clubless() {
    let mut fx = setup_e2().await;
    let t0 = now_ms();
    fx.clock_ms.store(t0, Ordering::SeqCst);
    assert_eq!(PRE_START_SKEW_MS, 120_000);

    // --- Club-less pilot with a closed earlier flight and a new open flight ---
    let (cl_acct, _, _) = create_account_session(
        &fx.db,
        "h8-cl@test.example",
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        t0,
    )
    .await;
    let cl_pid = fx.db.ensure_personal_pilot(&cl_acct, t0).await.unwrap();
    let (cl_dev, cl_tok) = fx.db.create_device(&cl_pid, "h8-cl-phone").await.unwrap();
    let cl_closed_fid = fx
        .db
        .start_personal_flight_atomic(
            &cl_pid,
            &cl_acct,
            &cl_dev,
            "CC-H8CL1",
            FlightVisibility::Private,
            false,
            None,
            t0 - 900_000,
        )
        .await
        .unwrap();
    fx.db
        .end_flight(&cl_pid, &cl_closed_fid, t0 - 700_000)
        .await
        .unwrap();
    let cl_start_t = t0 - 300_000;
    let cl_open_fid = fx
        .db
        .start_personal_flight_atomic(
            &cl_pid,
            &cl_acct,
            &cl_dev,
            "CC-H8CL2",
            FlightVisibility::Private,
            false,
            None,
            cl_start_t,
        )
        .await
        .unwrap();

    // Foreign club-less pilot with an active flight & a closed flight
    let (foreign_cl_acct, _, _) = create_account_session(
        &fx.db,
        "h8-foreign-cl@test.example",
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        t0,
    )
    .await;
    let foreign_cl_pid = fx
        .db
        .ensure_personal_pilot(&foreign_cl_acct, t0)
        .await
        .unwrap();
    let (foreign_cl_dev, _) = fx
        .db
        .create_device(&foreign_cl_pid, "h8-fcl-phone")
        .await
        .unwrap();
    let foreign_cl_closed_fid = fx
        .db
        .start_personal_flight_atomic(
            &foreign_cl_pid,
            &foreign_cl_acct,
            &foreign_cl_dev,
            "CC-H8F0",
            FlightVisibility::Private,
            false,
            None,
            t0 - 800_000,
        )
        .await
        .unwrap();
    fx.db
        .end_flight(&foreign_cl_pid, &foreign_cl_closed_fid, t0 - 750_000)
        .await
        .unwrap();
    let foreign_cl_active_fid = fx
        .db
        .start_personal_flight_atomic(
            &foreign_cl_pid,
            &foreign_cl_acct,
            &foreign_cl_dev,
            "CC-H8F1",
            FlightVisibility::Private,
            false,
            None,
            t0 - 250_000,
        )
        .await
        .unwrap();

    // --- Club pilot with a closed earlier flight and a new open flight ---
    let (club_id, _) = create_live_club(
        &fx.db,
        "h8-club",
        "own-h8@test.example",
        "SCTB",
        -33.456,
        -70.548,
    )
    .await;
    fx.db.upsert_type(&club_id, "C172").await.unwrap();
    fx.db
        .upsert_aircraft(&club_id, "CC-H8C1", "C172", true)
        .await
        .unwrap();
    fx.db
        .upsert_aircraft(&club_id, "CC-H8C2", "C172", true)
        .await
        .unwrap();
    let (c_acct, _, _) = create_account_session(
        &fx.db,
        "h8-clubp@test.example",
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        t0,
    )
    .await;
    let (c_pid, _, _) = add_club_member(
        &fx.db,
        &c_acct,
        "h8-clubp@test.example",
        "H8 Club Pilot",
        &club_id,
        "pilot",
    )
    .await;
    fx.db
        .add_checkout(&c_pid, "C172", None, None)
        .await
        .unwrap();
    let (c_dev, c_tok) = fx.db.create_device(&c_pid, "h8-club-phone").await.unwrap();
    fx.db
        .issue_open_checkin(&c_pid, &c_dev, &club_id, t0 - 950_000)
        .await
        .unwrap();
    let c_closed_fid = fx
        .db
        .start_flight(
            &c_pid,
            &c_dev,
            "CC-H8C1",
            FlightVisibility::Club,
            false,
            t0 - 900_000,
        )
        .await
        .unwrap();
    fx.db
        .end_flight(&c_pid, &c_closed_fid, t0 - 700_000)
        .await
        .unwrap();
    let c_start_t = t0 - 300_000;
    let c_open_fid = fx
        .db
        .start_flight(
            &c_pid,
            &c_dev,
            "CC-H8C1",
            FlightVisibility::Club,
            false,
            c_start_t,
        )
        .await
        .unwrap();

    // Foreign pilot in the SAME club with an active flight
    let (fc_acct, _, _) = create_account_session(
        &fx.db,
        "h8-fclubp@test.example",
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        t0,
    )
    .await;
    let (fc_pid, _, _) = add_club_member(
        &fx.db,
        &fc_acct,
        "h8-fclubp@test.example",
        "H8 Foreign Club Pilot",
        &club_id,
        "pilot",
    )
    .await;
    fx.db
        .add_checkout(&fc_pid, "C172", None, None)
        .await
        .unwrap();
    let (fc_dev, _) = fx.db.create_device(&fc_pid, "h8-fc-phone").await.unwrap();
    fx.db
        .issue_open_checkin(&fc_pid, &fc_dev, &club_id, t0 - 350_000)
        .await
        .unwrap();
    let foreign_club_active_fid = fx
        .db
        .start_flight(
            &fc_pid,
            &fc_dev,
            "CC-H8C2",
            FlightVisibility::Club,
            false,
            t0 - 250_000,
        )
        .await
        .unwrap();

    let mismatch_expected = json!({
        "error": "flight_mismatch",
        "reason": "flight_mismatch",
    });

    for (kind, pid, dev_id, tok, open_fid, closed_fid, foreign_active_fid, start_t) in [
        (
            "clubless",
            &cl_pid,
            &cl_dev,
            &cl_tok,
            &cl_open_fid,
            &cl_closed_fid,
            &foreign_cl_active_fid,
            cl_start_t,
        ),
        (
            "club",
            &c_pid,
            &c_dev,
            &c_tok,
            &c_open_fid,
            &c_closed_fid,
            &foreign_club_active_fid,
            c_start_t,
        ),
    ] {
        fx.clock_ms.store(t0, Ordering::SeqCst);

        // 1. Mismatches: own closed flight, foreign active flight, foreign closed flight,
        //    unknown ID, empty string, single-fix form, camelCase flightId, mixed per-fix ID.
        //    ALL return identical 409 flight_mismatch (no existence oracle), write 0 rows,
        //    and consume 0 rate-limit hits.
        let mismatch_payloads = [
            json!({
                "flight_id": closed_fid,
                "seq": 1,
                "fixes": [{ "t": start_t + 1_000, "lat": -33.45, "lon": -70.55, "alt": 900.0 }]
            }),
            json!({
                "flightId": closed_fid,
                "fixes": [{ "t": start_t - 500_000, "lat": -33.45, "lon": -70.55, "alt": 900.0 }]
            }),
            json!({
                "flight_id": foreign_active_fid,
                "fixes": [{ "t": start_t + 1_000, "lat": -33.45, "lon": -70.55, "alt": 900.0 }]
            }),
            json!({
                "flight_id": foreign_cl_closed_fid,
                "fixes": [{ "t": start_t + 1_000, "lat": -33.45, "lon": -70.55, "alt": 900.0 }]
            }),
            json!({
                "flight_id": "nonexistent-flight-id-xyz",
                "fixes": [{ "t": start_t + 1_000, "lat": -33.45, "lon": -70.55, "alt": 900.0 }]
            }),
            json!({
                "flight_id": "",
                "fixes": [{ "t": start_t + 1_000, "lat": -33.45, "lon": -70.55, "alt": 900.0 }]
            }),
            // Single-fix form mismatch
            json!({
                "flight_id": closed_fid,
                "seq": 2,
                "t": start_t + 1_000,
                "lat": -33.45,
                "lon": -70.55,
                "alt": 900.0
            }),
            // Mixed batch: batch flight_id matches, second fix carries closed_fid -> whole batch 409!
            json!({
                "flight_id": open_fid,
                "fixes": [
                    { "t": start_t + 1_000, "lat": -33.45, "lon": -70.55, "alt": 900.0, "flight_id": open_fid },
                    { "t": start_t + 2_000, "lat": -33.46, "lon": -70.56, "alt": 905.0, "flightId": closed_fid }
                ]
            }),
        ];
        for (idx, payload) in mismatch_payloads.iter().enumerate() {
            let (st, _, b) = send_json(
                &mut fx.router,
                "POST",
                "/api/ingest",
                None,
                Some(tok),
                &[],
                Some(payload.clone()),
            )
            .await;
            assert_eq!(st, StatusCode::CONFLICT, "{kind} mismatch[{idx}]: {b}");
            assert_eq!(b, mismatch_expected, "{kind} mismatch[{idx}]");
        }
        assert_eq!(fx.db.fixes_for_flight(open_fid).await.len(), 0, "{kind}");
        assert_eq!(fx.db.fixes_for_flight(closed_fid).await.len(), 0, "{kind}");
        assert_eq!(
            fx.db.fixes_for_flight(foreign_active_fid).await.len(),
            0,
            "{kind}"
        );

        // /api/osmand mismatch (snake_case query & camelCase form body) -> 409, zero writes
        let (st_os_m1, _, raw_os_m1) = send_raw(
            &mut fx.router,
            "POST",
            &format!(
                "/api/osmand?id={tok}&lat=-33.45&lon=-70.55&timestamp={}&flight_id={closed_fid}",
                (start_t + 1_000) / 1000
            ),
            None,
            None,
            &[],
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(st_os_m1, StatusCode::CONFLICT, "{kind}");
        assert_eq!(
            serde_json::from_slice::<Value>(&raw_os_m1).unwrap(),
            mismatch_expected
        );
        let (st_os_m2, _, raw_os_m2) = send_raw(
            &mut fx.router,
            "POST",
            "/api/osmand",
            None,
            None,
            &[],
            Some("application/x-www-form-urlencoded"),
            format!(
                "id={tok}&lat=-33.45&lon=-70.55&timestamp={}&flightId={foreign_active_fid}",
                (start_t + 2_000) / 1000
            )
            .into_bytes(),
        )
        .await;
        assert_eq!(st_os_m2, StatusCode::CONFLICT, "{kind}");
        assert_eq!(
            serde_json::from_slice::<Value>(&raw_os_m2).unwrap(),
            mismatch_expected
        );
        assert_eq!(fx.db.fixes_for_flight(open_fid).await.len(), 0, "{kind}");

        // 2. seq validation: negative seq -> 400 invalid seq; non-integer seq -> 400 invalid json
        for bad_seq in [
            json!({ "flight_id": open_fid, "seq": -1, "fixes": [{ "t": start_t + 1_000, "lat": -33.45, "lon": -70.55 }] }),
            json!({ "flight_id": open_fid, "fixes": [{ "t": start_t + 1_000, "lat": -33.45, "lon": -70.55, "seq": -5 }] }),
            json!({ "flight_id": open_fid, "seq": -3, "t": start_t + 1_000, "lat": -33.45, "lon": -70.55 }),
        ] {
            let (st, _, b) = send_json(
                &mut fx.router,
                "POST",
                "/api/ingest",
                None,
                Some(tok),
                &[],
                Some(bad_seq),
            )
            .await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{kind}: {b}");
            assert_eq!(b["error"], json!("invalid seq"), "{kind}");
        }
        for bad_type in [
            json!({ "flight_id": open_fid, "seq": "42", "fixes": [{ "t": start_t + 1_000, "lat": -33.45, "lon": -70.55 }] }),
            json!({ "flight_id": open_fid, "seq": 1.5, "fixes": [{ "t": start_t + 1_000, "lat": -33.45, "lon": -70.55 }] }),
        ] {
            let (st, _, b) = send_json(
                &mut fx.router,
                "POST",
                "/api/ingest",
                None,
                Some(tok),
                &[],
                Some(bad_type),
            )
            .await;
            assert_eq!(st, StatusCode::BAD_REQUEST, "{kind}: {b}");
            assert_eq!(b["error"], json!("invalid json"), "{kind}");
        }
        assert_eq!(fx.db.fixes_for_flight(open_fid).await.len(), 0, "{kind}");

        // 3. Pre-start skew filter (started_at - 120_000 ms):
        //    - All-pre-start batch returns 200 accepted=0, droppedPreStart=2, 0 writes, 0 cap used.
        let (st_all_pre, _, b_all_pre) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({
                "flight_id": open_fid,
                "seq": 0,
                "fixes": [
                    { "t": start_t - PRE_START_SKEW_MS - 60_000, "lat": -33.41, "lon": -70.51, "seq": 0 },
                    { "t": start_t - PRE_START_SKEW_MS - 1, "lat": -33.42, "lon": -70.52, "seq": 1 }
                ]
            })),
        )
        .await;
        assert_eq!(st_all_pre, StatusCode::OK, "{kind}: {b_all_pre}");
        assert_eq!(
            b_all_pre,
            json!({ "ok": true, "accepted": 0, "duplicates": 0, "droppedPreStart": 2 })
        );
        assert_eq!(fx.db.fixes_for_flight(open_fid).await.len(), 0, "{kind}");
        assert_eq!(fx.db.get_flight(open_fid).await.unwrap().last_fix_at, None);

        //    - Mixed batch: 2 pre-start dropped (< start - 120s), exact boundary (start - 120s),
        //      within-skew (start - 60s), and in-flight (start + 10s) KEPT!
        let (st_mix, _, b_mix) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({
                "flightId": open_fid,
                "seq": 10,
                "fixes": [
                    { "t": start_t - PRE_START_SKEW_MS - 120_000, "lat": -33.40, "lon": -70.50, "seq": 10 },
                    { "t": start_t - PRE_START_SKEW_MS - 1, "lat": -33.41, "lon": -70.51, "seq": 11 },
                    { "t": start_t - PRE_START_SKEW_MS, "lat": -33.42, "lon": -70.52, "seq": 12 },
                    { "t": start_t - 60_000, "lat": -33.43, "lon": -70.53, "seq": 13 },
                    { "t": start_t + 10_000, "lat": -33.44, "lon": -70.54, "seq": 14 }
                ]
            })),
        )
        .await;
        assert_eq!(st_mix, StatusCode::OK, "{kind}: {b_mix}");
        assert_eq!(
            b_mix,
            json!({ "ok": true, "accepted": 3, "duplicates": 0, "droppedPreStart": 2 })
        );
        let stored_ts: Vec<i64> = fx
            .db
            .fixes_for_flight(open_fid)
            .await
            .into_iter()
            .map(|f| f.t)
            .collect();
        assert_eq!(
            stored_ts,
            vec![
                start_t - PRE_START_SKEW_MS,
                start_t - 60_000,
                start_t + 10_000
            ],
            "{kind}"
        );

        // 4. Single-fix matching flight_id + seq -> 200; old client without flight_id -> 200
        //    (and old client still gets pre-start drop!).
        let (st_single, _, b_single) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({
                "flight_id": open_fid,
                "seq": 15,
                "t": start_t + 20_000,
                "lat": -33.45,
                "lon": -70.55,
                "alt": 950.0
            })),
        )
        .await;
        assert_eq!(st_single, StatusCode::OK, "{kind}: {b_single}");
        assert_eq!(
            b_single,
            json!({ "ok": true, "accepted": 1, "duplicates": 0, "droppedPreStart": 0 })
        );

        let (st_legacy, _, b_legacy) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({
                "fixes": [
                    { "t": start_t - PRE_START_SKEW_MS - 5_000, "lat": -33.39, "lon": -70.49 },
                    { "t": start_t + 30_000, "lat": -33.46, "lon": -70.56 }
                ]
            })),
        )
        .await;
        assert_eq!(st_legacy, StatusCode::OK, "{kind}: {b_legacy}");
        assert_eq!(
            b_legacy,
            json!({ "ok": true, "accepted": 1, "duplicates": 0, "droppedPreStart": 1 })
        );

        // /api/osmand with matching flight_id -> 200 OK (stored); pre-start fix -> 200 OK (dropped)
        let os_ok_t_sec = (start_t + 40_000) / 1000;
        let (st_os_ok, _, _) = send_raw(
            &mut fx.router,
            "POST",
            &format!(
                "/api/osmand?id={tok}&lat=-33.47&lon=-70.57&timestamp={os_ok_t_sec}&flight_id={open_fid}"
            ),
            None,
            None,
            &[],
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(st_os_ok, StatusCode::OK, "{kind}");
        let count_after_os_ok = fx.db.fixes_for_flight(open_fid).await.len();
        assert_eq!(count_after_os_ok, 6, "{kind}");

        let os_pre_t_sec = (start_t - PRE_START_SKEW_MS - 10_000) / 1000;
        let (st_os_pre, _, _) = send_raw(
            &mut fx.router,
            "POST",
            &format!("/api/osmand?id={tok}&lat=-33.38&lon=-70.48&timestamp={os_pre_t_sec}"),
            None,
            None,
            &[],
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(st_os_pre, StatusCode::OK, "{kind}");
        assert_eq!(
            fx.db.fixes_for_flight(open_fid).await.len(),
            count_after_os_ok,
            "{kind}: /api/osmand must drop pre-start fix without DB write"
        );

        // 5. Rate-limit cap interaction:
        //    Advance window cleanly -> send 409 mismatch (0 cap), then 50 all-pre-start (0 cap),
        //    then 50 pre-start + 50 in-flight (50 cap), then 70 in-flight (total 120 cap -> 200!),
        //    then 1 more in-flight -> 429; while at 120/120 cap, another all-pre-start still gets 200!
        let t_cap = t0 + 120_000;
        fx.clock_ms.store(t_cap, Ordering::SeqCst);

        let (st_m_cap, _, _) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({
                "flight_id": closed_fid,
                "fixes": [{ "t": t_cap - 1_000, "lat": -33.45, "lon": -70.55 }]
            })),
        )
        .await;
        assert_eq!(st_m_cap, StatusCode::CONFLICT, "{kind}");

        let all_pre_50: Vec<Value> = (0..50)
            .map(|i| json!({ "t": start_t - PRE_START_SKEW_MS - 10_000 - i as i64 * 100, "lat": -33.45, "lon": -70.55 }))
            .collect();
        let (st_p50, _, b_p50) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({ "flight_id": open_fid, "fixes": all_pre_50 })),
        )
        .await;
        assert_eq!(st_p50, StatusCode::OK, "{kind}: {b_p50}");
        assert_eq!(
            b_p50,
            json!({ "ok": true, "accepted": 0, "duplicates": 0, "droppedPreStart": 50 })
        );

        let mut mixed_100: Vec<Value> = (0..50)
            .map(|i| json!({ "t": start_t - PRE_START_SKEW_MS - 20_000 - i as i64 * 100, "lat": -33.45, "lon": -70.55 }))
            .collect();
        mixed_100.extend((0..50).map(
            |i| json!({ "t": t_cap - 40_000 + i as i64 * 100, "lat": -33.45, "lon": -70.55 }),
        ));
        let (st_m100, _, b_m100) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({ "flight_id": open_fid, "fixes": mixed_100 })),
        )
        .await;
        assert_eq!(st_m100, StatusCode::OK, "{kind}: {b_m100}");
        assert_eq!(
            b_m100,
            json!({ "ok": true, "accepted": 50, "duplicates": 0, "droppedPreStart": 50 })
        );

        let rem_70: Vec<Value> = (0..70)
            .map(|i| json!({ "t": t_cap - 20_000 + i as i64 * 100, "lat": -33.46, "lon": -70.56 }))
            .collect();
        let (st_r70, _, b_r70) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({ "flight_id": open_fid, "fixes": rem_70 })),
        )
        .await;
        assert_eq!(st_r70, StatusCode::OK, "{kind}: {b_r70}");
        assert_eq!(
            b_r70,
            json!({ "ok": true, "accepted": 70, "duplicates": 0, "droppedPreStart": 0 })
        );

        // 121st in-flight fix -> 429
        let (st_over, hdrs_over, _) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({
                "flight_id": open_fid,
                "fixes": [{ "t": t_cap - 5_000, "lat": -33.47, "lon": -70.57 }]
            })),
        )
        .await;
        assert_eq!(st_over, StatusCode::TOO_MANY_REQUESTS, "{kind}");
        assert!(hdrs_over.get(header::RETRY_AFTER).is_some(), "{kind}");

        // While at 120/120 cap, an all-pre-start batch still returns 200 droppedPreStart (0 cap cost)
        let (st_pre_at_cap, _, b_pre_at_cap) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({
                "flight_id": open_fid,
                "fixes": [{ "t": start_t - PRE_START_SKEW_MS - 30_000, "lat": -33.45, "lon": -70.55 }]
            })),
        )
        .await;
        assert_eq!(st_pre_at_cap, StatusCode::OK, "{kind}: {b_pre_at_cap}");
        assert_eq!(
            b_pre_at_cap,
            json!({ "ok": true, "accepted": 0, "duplicates": 0, "droppedPreStart": 1 })
        );

        // 6. Close open flight -> sending open_fid now returns 409 flight_mismatch (NOT 400!),
        //    while old client without flight_id gets 400 no active flight (or guest check-in).
        fx.db
            .end_flight(pid, open_fid, t_cap + 1_000)
            .await
            .unwrap();
        let count_closed = fx.db.fixes_for_flight(open_fid).await.len();
        let (st_after_end, _, b_after_end) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({
                "flight_id": open_fid,
                "fixes": [{ "t": t_cap, "lat": -33.48, "lon": -70.58 }]
            })),
        )
        .await;
        assert_eq!(st_after_end, StatusCode::CONFLICT, "{kind}: {b_after_end}");
        assert_eq!(b_after_end, mismatch_expected, "{kind}");
        assert_eq!(fx.db.fixes_for_flight(open_fid).await.len(), count_closed);

        // Defense-in-depth ownership check: even if flight_devices erroneously links this device
        // to a foreign active flight, ingest rejects with 409 flight_mismatch and writes 0 rows!
        sqlx::query("INSERT INTO flight_devices (flight_id, device_id) VALUES ($1, $2)")
            .bind(foreign_active_fid)
            .bind(dev_id)
            .execute(&fx.db.pool)
            .await
            .unwrap();
        let (st_tamper, _, b_tamper) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(tok),
            &[],
            Some(json!({
                "flight_id": foreign_active_fid,
                "fixes": [{ "t": t_cap, "lat": -33.49, "lon": -70.59 }]
            })),
        )
        .await;
        assert_eq!(st_tamper, StatusCode::CONFLICT, "{kind}: {b_tamper}");
        assert_eq!(b_tamper, mismatch_expected, "{kind}");
        assert_eq!(
            fx.db.fixes_for_flight(foreign_active_fid).await.len(),
            0,
            "{kind}: foreign flight must receive zero fixes even with rogue flight_devices link"
        );
        sqlx::query("DELETE FROM flight_devices WHERE flight_id = $1 AND device_id = $2")
            .bind(foreign_active_fid)
            .bind(dev_id)
            .execute(&fx.db.pool)
            .await
            .unwrap();
    }

    // 7. Guest check-in device sending a flight_id gets 409 flight_mismatch with zero live broadcast
    let (_gs_id, _gs_tok, guest_pilot) = fx
        .db
        .create_guest_session(&club_id, "Guest H8", t0)
        .await
        .unwrap();
    let (guest_dev, guest_tok) = fx
        .db
        .create_device(&guest_pilot.id, "guest-phone")
        .await
        .unwrap();
    fx.db
        .issue_open_checkin(&guest_pilot.id, &guest_dev, &club_id, t0)
        .await
        .unwrap();
    let (st_guest_m, _, b_guest_m) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&guest_tok),
        &[],
        Some(json!({
            "flight_id": foreign_club_active_fid,
            "fixes": [{ "t": t0, "lat": -33.45, "lon": -70.55 }]
        })),
    )
    .await;
    assert_eq!(st_guest_m, StatusCode::CONFLICT, "{b_guest_m}");
    assert_eq!(b_guest_m, mismatch_expected);
    assert!(
        fx.live.list_sessions().await.is_empty(),
        "409 flight_mismatch must never create/broadcast a LiveEngine session"
    );
}
