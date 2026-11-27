//! Integration & unit tests for:
//! - Part 1: `GET /api/me/live-trails?layers=club,public` (auth, isolation, visibility,
//!   membership lifecycle, club suspension, multi-club own-flight exclusion, effective trail
//!   minutes, Off latest-point-only, caps, validation, weak ETag/304).
//! - Part 2: Club nearby public flights (`GET/PATCH /api/admin/club/nearby-public`,
//!   `/api/admin/map`, `/api/admin/map/trails`, `/api/display/map`, `/api/display/map/trails`,
//!   toggle, radius, base requirement, visibility/suspension isolation, cap, admin auth + CSRF).

use crate::{
    auth::Auth,
    build_router,
    club::{
        auth::{hash_password, new_id},
        db::now_ms,
        geo::haversine_km,
        pilot_live_trails::{
            LiveTrailsLayers, LiveTrailsQuery, PublicArea,
            PILOT_LIVE_TRAILS_DEFAULT_PUBLIC_RADIUS_KM, PILOT_LIVE_TRAILS_MAX_FLIGHTS,
            PILOT_LIVE_TRAILS_MAX_PUBLIC_RADIUS_KM, PILOT_LIVE_TRAILS_MIN_PUBLIC_RADIUS_KM,
        },
        trail::{MAX_NEARBY_PUBLIC_FLIGHTS, MAX_TRAIL_POINTS, MAX_TRAIL_WINDOW_MIN},
        visibility::FlightVisibility,
        ClubDb,
    },
    device_auth::DeviceAuth,
    live::{LiveEngine, LiveFix},
    store::Store,
    AppState,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, Request, StatusCode},
};
use serde_json::{json, Value};
use serial_test::serial;
use std::{collections::HashSet, sync::Arc};
use tower::Service;

// ---------------------------------------------------------------------------
// Fixtures & HTTP helpers
// ---------------------------------------------------------------------------

pub(super) async fn http_stack_with_throttle(
    db: ClubDb,
    public_live_trails_throttle: crate::LoginThrottle,
) -> (tempfile::TempDir, axum::Router) {
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
        public_live_trails_throttle,
        mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
        webauthn: None,
        sse_limiter: Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails: HashSet::new(),
    };
    (data_dir, build_router(state))
}

pub(super) async fn http_stack(db: ClubDb) -> (tempfile::TempDir, axum::Router) {
    http_stack_with_throttle(
        db,
        crate::LoginThrottle::with_policy(crate::OverflowPolicy::RejectNew),
    )
    .await
}

pub(super) async fn create_live_club(
    db: &ClubDb,
    prefix: &str,
    owner_email: &str,
    base: Option<(&str, f64, f64)>,
) -> (String, String) {
    let now = now_ms();
    let slug = format!("{prefix}-{}", &new_id()[..8]);
    let (club_id, token) = db
        .signup_club(
            &format!("Club {prefix}"),
            &slug,
            owner_email,
            base.map(|b| b.0),
            now,
        )
        .await
        .expect("signup_club");
    db.try_consume_magic_club_verify(&token, now)
        .await
        .expect("verify club");
    match base {
        Some((icao, lat, lon)) => {
            db.set_club_base_aerodrome(&club_id, icao, lat, lon)
                .await
                .expect("set base");
        }
        None => {
            sqlx::query(
                "UPDATE clubs SET aerodrome_icao = NULL, latitude = NULL, longitude = NULL WHERE id = $1",
            )
            .bind(&club_id)
            .execute(&db.pool)
            .await
            .unwrap();
        }
    }
    (club_id, slug)
}

#[allow(clippy::too_many_arguments)]
async fn seed_member_with_status(
    db: &ClubDb,
    club_id: &str,
    name: &str,
    email: &str,
    role: &str,
    pilot_status: &str,
    membership_status: &str,
    account_id: Option<&str>,
) -> (String, String) {
    let now = now_ms();
    let hash = hash_password("password12").unwrap();
    let pilot_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at, account_id)
         VALUES ($1, $2, $3, $4, '', $5, $6, $7, $8, $8, $9)",
    )
    .bind(&pilot_id)
    .bind(club_id)
    .bind(name)
    .bind(email)
    .bind(pilot_status)
    .bind(role)
    .bind(&hash)
    .bind(now)
    .bind(account_id)
    .execute(&db.pool)
    .await
    .unwrap();
    db.ensure_membership_row(&pilot_id, club_id, role, membership_status)
        .await
        .unwrap();
    let sess = db.create_pilot_session(&pilot_id, now).await.unwrap();
    (pilot_id, format!("fv_club_session={sess}"))
}

pub(super) async fn seed_member(
    db: &ClubDb,
    club_id: &str,
    name: &str,
    email: &str,
    role: &str,
) -> (String, String) {
    seed_member_with_status(db, club_id, name, email, role, "approved", "active", None).await
}

async fn seed_account_session(db: &ClubDb, email: &str) -> (String, String) {
    let now = now_ms();
    let acct_id = new_id();
    sqlx::query(
        "INSERT INTO accounts (id, email, password_hash, email_verified_at, global_role, created_at)
         VALUES ($1, $2, NULL, $3, '', $3)",
    )
    .bind(&acct_id)
    .bind(email)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let sess = db
        .create_account_session_auth(&acct_id, None, now, "magic")
        .await
        .unwrap();
    (acct_id, format!("fv_account_session={sess}"))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn seed_live_flight(
    db: &ClubDb,
    club_id: &str,
    pilot_id: &str,
    tail: &str,
    visibility: FlightVisibility,
    show_pilot_name_public: bool,
    trail_override: Option<crate::club::trail::TrailMinutes>,
    fixes: &[LiveFix],
) -> (String, String) {
    let now = now_ms();
    db.upsert_type(club_id, "C152").await.unwrap();
    db.upsert_aircraft(club_id, tail, "C152", true)
        .await
        .unwrap();
    db.add_checkout(pilot_id, "C152", None, None).await.unwrap();
    let (dev_id, dev_tok) = db.create_device(pilot_id, "phone").await.unwrap();
    db.issue_open_checkin(pilot_id, &dev_id, club_id, now)
        .await
        .unwrap();
    let fid = db
        .start_flight_with_trail(
            pilot_id,
            &dev_id,
            tail,
            visibility,
            show_pilot_name_public,
            trail_override,
            now - 10_000,
        )
        .await
        .expect("start_flight_with_trail");
    db.append_fixes(&fid, &dev_id, fixes)
        .await
        .expect("append_fixes");
    (fid, dev_tok)
}

pub(super) fn make_fixes(latest_t: i64, lat: f64, lon: f64, minutes_back: &[i64]) -> Vec<LiveFix> {
    minutes_back
        .iter()
        .enumerate()
        .map(|(i, &min_ago)| LiveFix {
            t: latest_t - min_ago * 60_000,
            lat: lat + i as f64 * 0.001,
            lon: lon + i as f64 * 0.001,
            alt: Some(900.0 + i as f64 * 10.0),
            alt_ref: Some("wgs84_ellipsoid".to_string()),
            speed: Some(32.0),
            course: Some(90.0),
            accuracy: Some(5.0),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn send_req(
    router: &mut axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    bearer: Option<&str>,
    if_none_match: Option<&str>,
    origin: Option<&str>,
    content_type: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, HeaderMap, Value) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    if let Some(t) = bearer {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(inm) = if_none_match {
        b = b.header(header::IF_NONE_MATCH, inm);
    }
    if let Some(o) = origin {
        b = b
            .header(header::ORIGIN, o)
            .header(header::HOST, "127.0.0.1:8080");
    }
    let req = if let Some(json_body) = body {
        b.header(
            header::CONTENT_TYPE,
            content_type.unwrap_or("application/json"),
        )
        .body(Body::from(json_body.to_string()))
        .unwrap()
    } else {
        if let Some(ct) = content_type {
            b = b.header(header::CONTENT_TYPE, ct);
        }
        b.body(Body::empty()).unwrap()
    };
    let res = router.call(req).await.unwrap();
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

// ---------------------------------------------------------------------------
// PART 1 — Unit test for layers query parser
// ---------------------------------------------------------------------------

#[test]
fn layers_query_parser_validates_strictly() {
    assert_eq!(
        LiveTrailsLayers::parse_query(None),
        Ok(LiveTrailsLayers::CLUB_ONLY)
    );
    assert_eq!(
        LiveTrailsLayers::parse_query(Some("other=1")),
        Ok(LiveTrailsLayers::CLUB_ONLY)
    );
    assert_eq!(
        LiveTrailsLayers::parse_query(Some("layers=club")),
        Ok(LiveTrailsLayers {
            club: true,
            public: false
        })
    );
    assert_eq!(
        LiveTrailsLayers::parse_query(Some("layers=public")),
        Ok(LiveTrailsLayers {
            club: false,
            public: true
        })
    );
    assert_eq!(
        LiveTrailsLayers::parse_query(Some("layers=club,public")),
        Ok(LiveTrailsLayers {
            club: true,
            public: true
        })
    );
    assert_eq!(
        LiveTrailsLayers::parse_query(Some("layers=public,club")),
        Ok(LiveTrailsLayers {
            club: true,
            public: true
        })
    );

    for bad in [
        "layers=",
        "layers=,",
        "layers=club,",
        "layers=,public",
        "layers=club,,public",
        "layers=club,club",
        "layers=public,public",
        "layers=private",
        "layers=club,private",
        "layers=CLUB",
        "layers=club&layers=public",
        "layers=foo",
    ] {
        assert!(
            LiveTrailsLayers::parse_query(Some(bad)).is_err(),
            "expected error for query `{bad}`"
        );
    }
}

// ---------------------------------------------------------------------------
// PART 1 — Tests 1, 2, 4, 5: cross-club isolation, private never returned,
// public layer opt-in & deduplication, own flight excluded (single & multi-club)
// ---------------------------------------------------------------------------

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

#[tokio::test]
#[serial]
async fn pilot_live_trails_isolation_visibility_dedup_and_own_exclusion() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;
    let (_dem_root, _dem_guard) = make_flat_dem(&[("N34W071", 600.0), ("N33W072", 600.0)]);

    let (club_a, _) = create_live_club(
        &db,
        "plt-a",
        "own-plt-a@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    let (club_b, _) = create_live_club(
        &db,
        "plt-b",
        "own-plt-b@test",
        Some(("SCEL", -33.393, -70.786)),
    )
    .await;

    // Multi-club caller account with active pilots in Club A AND Club B
    let (caller_acct, _) = seed_account_session(&db, "multi-caller@test").await;
    let (caller_a, caller_a_cookie) = seed_member_with_status(
        &db,
        &club_a,
        "Caller A",
        "multi-caller@test",
        "pilot",
        "approved",
        "active",
        Some(&caller_acct),
    )
    .await;
    let (caller_b, _) = seed_member_with_status(
        &db,
        &club_b,
        "Caller B",
        "multi-caller@test",
        "pilot",
        "approved",
        "active",
        Some(&caller_acct),
    )
    .await;

    // Other pilots in Club A and Club B
    let (peer_a_club, _) = seed_member(&db, &club_a, "Peer A Club", "pac@test", "pilot").await;
    let (peer_a_pub, _) = seed_member(&db, &club_a, "Peer A Pub", "pap@test", "pilot").await;
    let (peer_a_priv, _) = seed_member(&db, &club_a, "Peer A Priv", "papr@test", "pilot").await;

    // Club C: caller is NOT a member of Club C
    let (club_c, _) = create_live_club(
        &db,
        "plt-c",
        "own-plt-c@test",
        Some(("SCVM", -32.95, -71.48)),
    )
    .await;
    let (peer_c_club, _) = seed_member(&db, &club_c, "Peer C Club", "pcc@test", "pilot").await;
    let (peer_c_pub, _) = seed_member(&db, &club_c, "Peer C Pub", "pcp@test", "pilot").await;
    let (peer_c_priv, _) = seed_member(&db, &club_c, "Peer C Priv", "pcpr@test", "pilot").await;

    let now = now_ms();
    // Caller's OWN flights (in Club A and in Club B via same account) — BOTH must be excluded!
    let (fid_own_a, caller_a_bearer) = seed_live_flight(
        &db,
        &club_a,
        &caller_a,
        "CC-OWNA",
        FlightVisibility::Public,
        true,
        None,
        &make_fixes(now, -33.45, -70.55, &[2, 0]),
    )
    .await;
    let (fid_own_b, _) = seed_live_flight(
        &db,
        &club_b,
        &caller_b,
        "CC-OWNB",
        FlightVisibility::Public,
        true,
        None,
        &make_fixes(now, -33.40, -70.78, &[2, 0]),
    )
    .await;

    // Peer flights in Club A
    let (fid_a_club, _) = seed_live_flight(
        &db,
        &club_a,
        &peer_a_club,
        "CC-ACLUB",
        FlightVisibility::Club,
        true,
        None,
        &make_fixes(now, -33.46, -70.54, &[3, 0]),
    )
    .await;
    let (fid_a_pub, _) = seed_live_flight(
        &db,
        &club_a,
        &peer_a_pub,
        "CC-APUB",
        FlightVisibility::Public,
        true,
        None,
        &make_fixes(now, -33.47, -70.53, &[3, 0]),
    )
    .await;
    let (fid_a_priv, _) = seed_live_flight(
        &db,
        &club_a,
        &peer_a_priv,
        "CC-APRIV",
        FlightVisibility::Private,
        true,
        None,
        &make_fixes(now, -33.48, -70.52, &[3, 0]),
    )
    .await;

    // Peer flights in Club C (non-member club)
    let (fid_c_club, _) = seed_live_flight(
        &db,
        &club_c,
        &peer_c_club,
        "CC-CCLUB",
        FlightVisibility::Club,
        true,
        None,
        &make_fixes(now, -32.95, -71.48, &[3, 0]),
    )
    .await;
    let (fid_c_pub, _) = seed_live_flight(
        &db,
        &club_c,
        &peer_c_pub,
        "CC-CPUB",
        FlightVisibility::Public,
        true,
        None,
        &make_fixes(now, -32.96, -71.47, &[3, 0]),
    )
    .await;
    let (fid_c_priv, _) = seed_live_flight(
        &db,
        &club_c,
        &peer_c_priv,
        "CC-CPRIV",
        FlightVisibility::Private,
        true,
        None,
        &make_fixes(now, -32.97, -71.46, &[3, 0]),
    )
    .await;

    // 1) Default (no layers param) AND layers=club: only Club A's club+public peer flights!
    // Never own flights (neither Club A nor Club B), never private, never Club C (club or public).
    for (uri, cookie, bearer) in [
        ("/api/me/live-trails", Some(caller_a_cookie.as_str()), None),
        ("/api/me/live-trails", None, Some(caller_a_bearer.as_str())),
        (
            "/api/me/live-trails?layers=club",
            Some(caller_a_cookie.as_str()),
            Some(caller_a_bearer.as_str()),
        ),
    ] {
        let (st, headers, body) = send_req(
            &mut router,
            "GET",
            uri,
            cookie,
            bearer,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "uri={uri} body={body}");
        assert!(
            headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none(),
            "must not set CORS headers"
        );
        let flights = body["flights"].as_array().unwrap();
        let ids: HashSet<&str> = flights
            .iter()
            .filter_map(|f| f["flightId"].as_str())
            .collect();
        assert!(
            ids.contains(fid_a_club.as_str()),
            "missing Club A club flight"
        );
        assert!(
            ids.contains(fid_a_pub.as_str()),
            "missing Club A public flight"
        );
        assert!(
            !ids.contains(fid_own_a.as_str()),
            "leaked own Club A flight"
        );
        assert!(
            !ids.contains(fid_own_b.as_str()),
            "leaked own Club B flight across account"
        );
        assert!(
            !ids.contains(fid_a_priv.as_str()),
            "leaked Club A private flight"
        );
        assert!(
            !ids.contains(fid_c_club.as_str()),
            "leaked Club C club-only flight"
        );
        assert!(
            !ids.contains(fid_c_pub.as_str()),
            "leaked Club C public flight when public not requested"
        );
        assert!(
            !ids.contains(fid_c_priv.as_str()),
            "leaked Club C private flight"
        );

        // Verify minimal fields & zero PII
        for f in flights {
            assert_eq!(f["layer"], json!("club"));
            assert!(f.get("pilotId").is_none());
            assert!(f.get("pilotName").is_none());
            assert!(f.get("email").is_none());
            assert!(f.get("accountId").is_none());
            assert!(f.get("deviceId").is_none());
            let first_pt = f["points"].as_array().unwrap()[0].as_array().unwrap();
            assert_eq!(
                first_pt.len(),
                4,
                "point must be [t_ms, lat, lon, alt|null]"
            );
        }
        assert_eq!(
            body["caps"]["maxFlights"],
            json!(PILOT_LIVE_TRAILS_MAX_FLIGHTS)
        );
        assert_eq!(body["caps"]["maxPointsPerFlight"], json!(MAX_TRAIL_POINTS));
        assert_eq!(
            body["caps"]["maxWindowMinutes"],
            json!(MAX_TRAIL_WINDOW_MIN)
        );
        assert!(
            body.get("publicArea").is_none(),
            "club-only response must omit publicArea"
        );
    }

    // 2) layers=club,public: Club C public (~102 km from SCTB, within radius_km=150) now included with layer="public";
    // Club A public appears ONCE with layer="club"; private & Club C club-only still excluded; own still excluded!
    let (st, _, body) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=club,public&lat=-33.456&lon=-70.548&radius_km=150",
        None,
        Some(&caller_a_bearer),
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        body["publicArea"],
        json!({ "lat": -33.456, "lon": -70.548, "radiusKm": 150.0, "minAglFt": 300.0 })
    );
    let flights = body["flights"].as_array().unwrap();
    let count_a_pub = flights
        .iter()
        .filter(|f| f["flightId"] == fid_a_pub)
        .count();
    assert_eq!(count_a_pub, 1, "dual-qualifying flight must appear once");
    let a_pub = flights.iter().find(|f| f["flightId"] == fid_a_pub).unwrap();
    assert_eq!(
        a_pub["layer"],
        json!("club"),
        "dual-qualifying flight must have layer=club"
    );

    let c_pub = flights
        .iter()
        .find(|f| f["flightId"] == fid_c_pub)
        .expect("Club C public flight must appear with layers=club,public");
    assert_eq!(c_pub["layer"], json!("public"));

    let ids: HashSet<&str> = flights
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(!ids.contains(fid_own_a.as_str()));
    assert!(!ids.contains(fid_own_b.as_str()));
    assert!(!ids.contains(fid_a_priv.as_str()));
    assert!(!ids.contains(fid_c_club.as_str()));
    assert!(!ids.contains(fid_c_priv.as_str()));
}

// ---------------------------------------------------------------------------
// PART 1 — Test 3 + Safety tests: membership states (none, pending, suspended,
// removed), suspended club, and NULL club_id flights
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn pilot_live_trails_membership_states_suspended_club_and_null_club_id() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;
    let (_dem_root, _dem_guard) = make_flat_dem(&[("N34W071", 600.0)]);

    let (club_a, _) = create_live_club(
        &db,
        "plt-ms-a",
        "own-ms-a@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    let (club_susp, _) = create_live_club(
        &db,
        "plt-ms-s",
        "own-ms-s@test",
        Some(("SCEL", -33.393, -70.786)),
    )
    .await;

    // Seed an active club-only flight AND a public flight in Club A
    let (peer_a, _) = seed_member(&db, &club_a, "Peer A", "pms-a@test", "pilot").await;
    let (peer_a_pub, _) = seed_member(&db, &club_a, "Peer A Pub", "pms-ap@test", "pilot").await;
    let (peer_null, _) = seed_member(&db, &club_a, "Peer Null", "pms-null@test", "pilot").await;
    let (peer_susp, _) = seed_member(&db, &club_susp, "Peer Susp", "pms-s@test", "pilot").await;
    let (active_viewer, active_cookie) =
        seed_member(&db, &club_a, "Active Viewer", "act-v@test", "pilot").await;
    let _ = active_viewer;

    let now = now_ms();
    let (fid_a_club, _) = seed_live_flight(
        &db,
        &club_a,
        &peer_a,
        "CC-MSA1",
        FlightVisibility::Club,
        false,
        None,
        &make_fixes(now, -33.45, -70.55, &[2, 0]),
    )
    .await;
    let (fid_a_pub, _) = seed_live_flight(
        &db,
        &club_a,
        &peer_a_pub,
        "CC-MSA2",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes(now, -33.46, -70.56, &[2, 0]),
    )
    .await;
    let (fid_null_club, _) = seed_live_flight(
        &db,
        &club_a,
        &peer_null,
        "CC-NULL",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes(now, -33.47, -70.57, &[2, 0]),
    )
    .await;
    // Simulate a club-less flight row (club_id NULL, aircraft_id NULL, free-text tail)
    sqlx::query(
        "UPDATE flights SET club_id = NULL, aircraft_id = NULL, tail_text = 'CC-NULL' WHERE id = $1",
    )
        .bind(&fid_null_club)
        .execute(&db.pool)
        .await
        .unwrap();

    let (fid_susp_pub, _) = seed_live_flight(
        &db,
        &club_susp,
        &peer_susp,
        "CC-SUSP",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes(now, -33.39, -70.78, &[2, 0]),
    )
    .await;

    // Suspend club_susp
    db.set_club_suspended(&club_susp, now).await.unwrap();

    // 1) Active viewer in Club A with layers=club,public (50 km covers both Club A and suspended Club S):
    //    NEVER sees suspended club's public flight; NEVER sees NULL club_id flight.
    let (st, _, body) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=club,public&lat=-33.456&lon=-70.548&radius_km=50",
        Some(&active_cookie),
        None,
        None,
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
    assert!(ids.contains(fid_a_club.as_str()));
    assert!(ids.contains(fid_a_pub.as_str()));
    assert!(
        !ids.contains(fid_susp_pub.as_str()),
        "suspended club flight must be excluded"
    );
    assert!(
        !ids.contains(fid_null_club.as_str()),
        "NULL club_id flight must be excluded"
    );

    // 2) Viewer with NO membership at all (account session, 0 memberships) -> zero club rows (NO default_club_id fallback!)
    let (_acct_none, no_mem_cookie) = seed_account_session(&db, "nomem@test").await;
    let (st, _, body_club) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=club",
        Some(&no_mem_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body_club}");
    assert!(
        body_club["flights"].as_array().unwrap().is_empty(),
        "no-membership viewer must get zero club rows"
    );

    // Same no-membership viewer with layers=club,public -> zero club rows, gets public rows only
    let (st, _, body_both) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=club,public&lat=-33.456&lon=-70.548",
        Some(&no_mem_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body_both}");
    let flights_both = body_both["flights"].as_array().unwrap();
    assert!(
        flights_both.iter().all(|f| f["layer"] == "public"),
        "no-membership viewer must never get layer=club rows"
    );
    assert!(flights_both.iter().any(|f| f["flightId"] == fid_a_pub));
    assert!(!flights_both.iter().any(|f| f["flightId"] == fid_a_club));

    // 3) PENDING member -> zero club rows
    let (_pend_id, pend_cookie) = seed_member_with_status(
        &db,
        &club_a,
        "Pending",
        "pend-ms@test",
        "pilot",
        "pending",
        "pending",
        None,
    )
    .await;
    let (st, _, body_pend) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=club",
        Some(&pend_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        body_pend["flights"].as_array().unwrap().is_empty(),
        "pending member must get zero club rows"
    );

    // 4) SUSPENDED member -> zero club rows
    let (_susp_id, susp_cookie) = seed_member_with_status(
        &db,
        &club_a,
        "Suspended",
        "susp-ms@test",
        "pilot",
        "suspended",
        "suspended",
        None,
    )
    .await;
    let (st, _, body_susp) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=club",
        Some(&susp_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        body_susp["flights"].as_array().unwrap().is_empty(),
        "suspended member must get zero club rows"
    );

    // 5) REMOVED membership -> zero club rows
    let (rem_id, rem_cookie) = seed_member(&db, &club_a, "Removed", "rem-ms@test", "pilot").await;
    sqlx::query("DELETE FROM club_memberships WHERE pilot_id = $1 AND club_id = $2")
        .bind(&rem_id)
        .bind(&club_a)
        .execute(&db.pool)
        .await
        .unwrap();
    let (st, _, body_rem) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=club",
        Some(&rem_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        body_rem["flights"].as_array().unwrap().is_empty(),
        "removed member must get zero club rows"
    );
}

// ---------------------------------------------------------------------------
// PART 1 — Tests 6, 8, 9: effective trail minutes, Off latest-point-only,
// stale exclusion, 400/401 validation, and weak ETag / 304 Not Modified
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn pilot_live_trails_effective_windows_off_validation_auth_and_etag_304() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;

    let (club_a, _) = create_live_club(
        &db,
        "plt-win",
        "own-win@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    // Set club default to 10 min
    db.set_club_trail_minutes_default(&club_a, crate::club::trail::TrailMinutes::Min10)
        .await
        .unwrap();

    let (_viewer_id, viewer_cookie) =
        seed_member(&db, &club_a, "Viewer", "v-win@test", "pilot").await;
    let (p_def, _) = seed_member(&db, &club_a, "P Def", "pdef@test", "pilot").await;
    let (p_ovr, _) = seed_member(&db, &club_a, "P Ovr", "povr@test", "pilot").await;
    let (p_off, _) = seed_member(&db, &club_a, "P Off", "poff@test", "pilot").await;
    let (p_stale, _) = seed_member(&db, &club_a, "P Stale", "pstale@test", "pilot").await;

    let now = now_ms();
    // Fixes at 20, 8, 3, 1, 0 min ago -> 10-min club default gets 4 pts; 2-min override gets 2 pts; Off gets 1 latest pt
    let (fid_def, _) = seed_live_flight(
        &db,
        &club_a,
        &p_def,
        "CC-WDEF",
        FlightVisibility::Club,
        false,
        None,
        &make_fixes(now, -33.45, -70.55, &[20, 8, 3, 1, 0]),
    )
    .await;
    let (fid_ovr, _) = seed_live_flight(
        &db,
        &club_a,
        &p_ovr,
        "CC-WOVR",
        FlightVisibility::Club,
        false,
        Some(crate::club::trail::TrailMinutes::Min2),
        &make_fixes(now, -33.46, -70.56, &[20, 8, 3, 1, 0]),
    )
    .await;
    let (fid_off, _) = seed_live_flight(
        &db,
        &club_a,
        &p_off,
        "CC-WOFF",
        FlightVisibility::Club,
        false,
        Some(crate::club::trail::TrailMinutes::Off),
        &make_fixes(now, -33.47, -70.57, &[10, 5, 0]),
    )
    .await;
    // Stale flight whose latest fix is 15 min old (> 10 min live threshold) -> excluded
    let (fid_stale, _) = seed_live_flight(
        &db,
        &club_a,
        &p_stale,
        "CC-STALE",
        FlightVisibility::Club,
        false,
        None,
        &make_fixes(now - 15 * 60_000, -33.48, -70.58, &[2, 0]),
    )
    .await;

    // Unauthenticated -> 401
    let (st, _, _) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails",
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Invalid Bearer token -> 401
    let (st, _, _) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails",
        None,
        Some("bad-device-token"),
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Unknown / malformed layers -> 400
    for bad_uri in [
        "/api/me/live-trails?layers=bogus",
        "/api/me/live-trails?layers=",
        "/api/me/live-trails?layers=club,",
        "/api/me/live-trails?layers=club,club",
        "/api/me/live-trails?layers=private",
    ] {
        let (st, _, body) = send_req(
            &mut router,
            "GET",
            bad_uri,
            Some(&viewer_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "uri={bad_uri} body={body}");
        assert!(body.get("error").and_then(Value::as_str).is_some());
    }

    // Valid GET -> 200 + weak ETag + effective trail windows + Off latest point only
    let (st, headers, body) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails",
        Some(&viewer_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let etag1 = headers
        .get(header::ETAG)
        .expect("ETag header")
        .to_str()
        .unwrap()
        .to_string();
    assert!(etag1.starts_with("W/\""), "expected weak ETag, got {etag1}");

    let flights = body["flights"].as_array().unwrap();
    assert!(
        !flights.iter().any(|f| f["flightId"] == fid_stale),
        "stale flight must be excluded"
    );

    let f_def = flights.iter().find(|f| f["flightId"] == fid_def).unwrap();
    assert_eq!(f_def["trailMinutes"], json!(10));
    assert_eq!(f_def["points"].as_array().unwrap().len(), 4);

    let f_ovr = flights.iter().find(|f| f["flightId"] == fid_ovr).unwrap();
    assert_eq!(f_ovr["trailMinutes"], json!(2));
    assert_eq!(f_ovr["points"].as_array().unwrap().len(), 2);

    let f_off = flights.iter().find(|f| f["flightId"] == fid_off).unwrap();
    assert_eq!(f_off["trailMinutes"], json!(0));
    let off_pts = f_off["points"].as_array().unwrap();
    assert_eq!(off_pts.len(), 1, "Off flight must return latest point only");
    assert_eq!(off_pts[0][0], json!(now));

    // Repeat with matching If-None-Match -> 304 Not Modified
    let (st304, headers304, body304) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails",
        Some(&viewer_cookie),
        None,
        Some(&etag1),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st304, StatusCode::NOT_MODIFIED);
    assert_eq!(body304, Value::Null);
    assert_eq!(
        headers304.get(header::ETAG).unwrap().to_str().unwrap(),
        etag1
    );

    // Changing trail setting changes ETag -> old If-None-Match gets 200 with a new ETag
    db.set_club_trail_minutes_default(&club_a, crate::club::trail::TrailMinutes::Min5)
        .await
        .unwrap();
    let (st_after, headers_after, body_after) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails",
        Some(&viewer_cookie),
        None,
        Some(&etag1),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_after, StatusCode::OK, "{body_after}");
    let etag2 = headers_after
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_ne!(etag2, etag1);
}

// ---------------------------------------------------------------------------
// PART 1 — Test 7: Caps (> MAX_FLIGHTS truncated; > MAX_TRAIL_POINTS downsampled keeping first/last)
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn pilot_live_trails_enforces_flight_and_point_caps() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;

    let (club_a, _) = create_live_club(
        &db,
        "plt-cap",
        "own-cap@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    let (_viewer_id, viewer_cookie) =
        seed_member(&db, &club_a, "Cap Viewer", "capv@test", "pilot").await;
    db.upsert_type(&club_a, "C152").await.unwrap();

    let now = now_ms();
    // Create 53 live club flights with strictly increasing last_fix_at so we verify truncation keeps the newest 50!
    let total_flights = PILOT_LIVE_TRAILS_MAX_FLIGHTS + 3;
    let mut created_fids = Vec::with_capacity(total_flights);

    for i in 0..total_flights {
        let tail = format!("CC-{i:03}");
        let email = format!("cap-p-{i}@test");
        let (pid, _) = seed_member(&db, &club_a, &format!("Pilot {i}"), &email, "pilot").await;
        db.upsert_aircraft(&club_a, &tail, "C152", true)
            .await
            .unwrap();
        db.add_checkout(&pid, "C152", None, None).await.unwrap();
        let (dev_id, _) = db.create_device(&pid, "phone").await.unwrap();
        db.issue_open_checkin(&pid, &dev_id, &club_a, now)
            .await
            .unwrap();
        let fid = db
            .start_flight_with_trail(
                &pid,
                &dev_id,
                &tail,
                FlightVisibility::Club,
                false,
                Some(crate::club::trail::TrailMinutes::Min30),
                now - 60_000,
            )
            .await
            .unwrap();

        let last_t = now - ((total_flights - 1 - i) as i64) * 1_000;
        let fixes = if i == total_flights - 1 {
            // Newest flight gets 450 fixes within 15 minutes (> MAX_TRAIL_POINTS = 300)
            (0..450i64)
                .map(|k| LiveFix {
                    t: last_t - (449 - k) * 2_000,
                    lat: -33.45 + k as f64 * 0.0001,
                    lon: -70.55 + k as f64 * 0.0001,
                    alt: Some(1000.0 + k as f64),
                    alt_ref: None,
                    speed: None,
                    course: None,
                    accuracy: Some(5.0),
                })
                .collect::<Vec<_>>()
        } else {
            vec![LiveFix {
                t: last_t,
                lat: -33.45,
                lon: -70.55,
                alt: Some(900.0),
                alt_ref: None,
                speed: None,
                course: None,
                accuracy: Some(5.0),
            }]
        };
        db.append_fixes(&fid, &dev_id, &fixes).await.unwrap();
        created_fids.push((fid, last_t));
    }

    let (st, _, body) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=club",
        Some(&viewer_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let flights = body["flights"].as_array().unwrap();
    assert_eq!(flights.len(), PILOT_LIVE_TRAILS_MAX_FLIGHTS);

    // Newest flight must be first, downsampled to MAX_TRAIL_POINTS keeping exact first & last timestamps!
    let (newest_fid, newest_last_t) = &created_fids[total_flights - 1];
    assert_eq!(flights[0]["flightId"], json!(newest_fid));
    let pts = flights[0]["points"].as_array().unwrap();
    assert_eq!(pts.len(), MAX_TRAIL_POINTS);
    assert_eq!(pts.first().unwrap()[0], json!(newest_last_t - 449 * 2_000));
    assert_eq!(pts.last().unwrap()[0], json!(*newest_last_t));

    // Oldest 3 flights must be truncated out
    let returned_ids: HashSet<&str> = flights
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    for (old_fid, _) in &created_fids[..3] {
        assert!(!returned_ids.contains(old_fid.as_str()));
    }
}

// ---------------------------------------------------------------------------
// PART 2 — Nearby public flights on club view (admin map & club screen)
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn club_view_nearby_public_default_toggle_radius_base_visibility_and_suspension() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;

    // Club A at SCTB (-33.456, -70.548) — show_nearby_public defaults to TRUE, radius = 50 km
    let (club_a, _) = create_live_club(
        &db,
        "np-a",
        "own-np-a@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    // Club B (active, trail default = 15 min)
    let (club_b, _) = create_live_club(
        &db,
        "np-b",
        "own-np-b@test",
        Some(("SCEL", -33.393, -70.786)),
    )
    .await;
    db.set_club_trail_minutes_default(&club_b, crate::club::trail::TrailMinutes::Min15)
        .await
        .unwrap();

    // Club S (will be suspended)
    let (club_s, _) = create_live_club(
        &db,
        "np-s",
        "own-np-s@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;

    let (_adm_a, adm_a_cookie) = seed_member(&db, &club_a, "Admin A", "adm-np@test", "admin").await;
    let (pb_near_anon, _) = seed_member(&db, &club_b, "Hidden Name", "pb1@test", "pilot").await;
    let (pb_near_named, _) = seed_member(&db, &club_b, "Public Name", "pb2@test", "pilot").await;
    let (pb_mid, _) = seed_member(&db, &club_b, "Mid Dist", "pb3@test", "pilot").await;
    let (pb_far, _) = seed_member(&db, &club_b, "Far Out", "pb4@test", "pilot").await;
    let (pb_club_only, _) = seed_member(&db, &club_b, "Club Only", "pb5@test", "pilot").await;
    let (pb_priv, _) = seed_member(&db, &club_b, "Private", "pb6@test", "pilot").await;
    let (ps_pub, _) = seed_member(&db, &club_s, "Susp Pub", "ps1@test", "pilot").await;

    let now = now_ms();
    // ~5 km north of SCTB (inside 10 km and 50 km): public, show_pilot_name_public = false, inherits Club B's 15-min default
    let (fid_near_anon, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_near_anon,
        "CC-NEAR1",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes(now, -33.41, -70.548, &[12, 6, 0]),
    )
    .await;
    // ~8 km south of SCTB: public, show_pilot_name_public = true, explicit 2-min override
    let (fid_near_named, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_near_named,
        "CC-NEAR2",
        FlightVisibility::Public,
        true,
        Some(crate::club::trail::TrailMinutes::Min2),
        &make_fixes(now, -33.528, -70.548, &[12, 1, 0]),
    )
    .await;
    // ~30 km north of SCTB (outside 25 km, inside 50 km)
    let (fid_mid, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_mid,
        "CC-MID30",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes(now, -33.186, -70.548, &[4, 0]),
    )
    .await;
    // ~65 km north of SCTB (outside default 50 km, inside 100 km)
    let (fid_far, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_far,
        "CC-FAR65",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes(now, -32.871, -70.548, &[4, 0]),
    )
    .await;
    // ~2 km from SCTB but club-only & private -> NEVER returned
    let (fid_club_only, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_club_only,
        "CC-BCLUB",
        FlightVisibility::Club,
        true,
        None,
        &make_fixes(now, -33.44, -70.548, &[2, 0]),
    )
    .await;
    let (fid_priv, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_priv,
        "CC-BPRIV",
        FlightVisibility::Private,
        true,
        None,
        &make_fixes(now, -33.44, -70.55, &[2, 0]),
    )
    .await;
    // ~2 km from SCTB, public, but Club S is suspended -> NEVER returned
    let (fid_susp, _) = seed_live_flight(
        &db,
        &club_s,
        &ps_pub,
        "CC-BSUSP",
        FlightVisibility::Public,
        true,
        None,
        &make_fixes(now, -33.44, -70.54, &[2, 0]),
    )
    .await;
    db.set_club_suspended(&club_s, now).await.unwrap();

    // 1) Default settings (on = true, radius = 50 km):
    //    GET /api/club/me and GET /api/admin/club/nearby-public expose defaults
    let (st_me, _, me_body) = send_req(
        &mut router,
        "GET",
        "/api/club/me",
        Some(&adm_a_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_me, StatusCode::OK);
    assert_eq!(me_body["clubShowNearbyPublic"], json!(true));
    assert_eq!(me_body["clubNearbyPublicRadiusKm"], json!(50));

    // Check ALL FOUR map & trail endpoints (admin + display)
    for map_uri in ["/api/admin/map", "/api/display/map"] {
        let (st, _, body) = send_req(
            &mut router,
            "GET",
            map_uri,
            Some(&adm_a_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{map_uri}: {body}");
        let ac = body["aircraft"].as_array().unwrap();
        let ids: HashSet<&str> = ac.iter().filter_map(|x| x["flightId"].as_str()).collect();
        assert!(
            ids.contains(fid_near_anon.as_str()),
            "{map_uri} missing near anon"
        );
        assert!(
            ids.contains(fid_near_named.as_str()),
            "{map_uri} missing near named"
        );
        assert!(
            ids.contains(fid_mid.as_str()),
            "{map_uri} missing 30 km flight"
        );
        assert!(
            !ids.contains(fid_far.as_str()),
            "{map_uri} leaked 65 km flight (> 50 km)"
        );
        assert!(
            !ids.contains(fid_club_only.as_str()),
            "{map_uri} leaked other club's club-only"
        );
        assert!(
            !ids.contains(fid_priv.as_str()),
            "{map_uri} leaked other club's private"
        );
        assert!(
            !ids.contains(fid_susp.as_str()),
            "{map_uri} leaked suspended club flight"
        );

        let near1 = ac.iter().find(|x| x["flightId"] == fid_near_anon).unwrap();
        assert_eq!(near1["tailSource"], json!("club"));
        assert!(
            near1.get("nonClub").is_none(),
            "L-92: other-club aircraft must have nonClub=false (omitted): {near1}"
        );
        assert_eq!(
            near1["otherClub"],
            json!(true),
            "L-92: other-club aircraft must set otherClub=true: {near1}"
        );
        assert!(
            near1["pilotName"].is_null(),
            "pilotName must be null when show_pilot_name_public=false"
        );
        assert!(
            near1.get("pilotId").is_none(),
            "pilotId must never be exposed on non-club flights"
        );
        assert_eq!(
            near1["trailMinutes"],
            json!(15),
            "must use ITS club's default (15)"
        );

        let near2 = ac.iter().find(|x| x["flightId"] == fid_near_named).unwrap();
        assert_eq!(near2["tailSource"], json!("club"));
        assert!(
            near2.get("nonClub").is_none(),
            "L-92: other-club aircraft must have nonClub=false (omitted): {near2}"
        );
        assert_eq!(
            near2["otherClub"],
            json!(true),
            "L-92: other-club aircraft must set otherClub=true: {near2}"
        );
        assert_eq!(near2["pilotName"], json!("Public Name"));
        assert!(near2.get("pilotId").is_none());
        assert_eq!(
            near2["trailMinutes"],
            json!(2),
            "must use flight override (2)"
        );
    }

    for tr_uri in ["/api/admin/map/trails", "/api/display/map/trails"] {
        let (st, _, body) = send_req(
            &mut router,
            "GET",
            tr_uri,
            Some(&adm_a_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{tr_uri}: {body}");
        let trails = body["trails"].as_array().unwrap();
        let t1 = trails
            .iter()
            .find(|t| t["flightId"] == fid_near_anon)
            .unwrap();
        assert_eq!(t1["tailSource"], json!("club"));
        assert!(t1.get("nonClub").is_none(), "L-92: {t1}");
        assert_eq!(t1["otherClub"], json!(true), "L-92: {t1}");
        assert_eq!(t1["trailMinutes"], json!(15));
        assert_eq!(t1["points"].as_array().unwrap().len(), 3);

        let t2 = trails
            .iter()
            .find(|t| t["flightId"] == fid_near_named)
            .unwrap();
        assert_eq!(t2["tailSource"], json!("club"));
        assert!(t2.get("nonClub").is_none(), "L-92: {t2}");
        assert_eq!(t2["otherClub"], json!(true), "L-92: {t2}");
        assert_eq!(t2["trailMinutes"], json!(2));
        assert_eq!(t2["points"].as_array().unwrap().len(), 2);

        assert!(!trails.iter().any(|t| t["flightId"] == fid_far));
        assert!(!trails.iter().any(|t| t["flightId"] == fid_club_only));
        assert!(!trails.iter().any(|t| t["flightId"] == fid_priv));
        assert!(!trails.iter().any(|t| t["flightId"] == fid_susp));
    }

    // 2) Shrink radius to 25 km -> 30 km flight excluded; expand to 100 km -> 65 km flight included
    let (st, _, body) = send_req(
        &mut router,
        "PATCH",
        "/api/admin/club/nearby-public",
        Some(&adm_a_cookie),
        None,
        None,
        None,
        None,
        Some(json!({ "nearbyPublicRadiusKm": 25 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["showNearbyPublic"], json!(true));
    assert_eq!(body["nearbyPublicRadiusKm"], json!(25));

    let (_, _, map_25) = send_req(
        &mut router,
        "GET",
        "/api/display/map",
        Some(&adm_a_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    let ids_25: HashSet<&str> = map_25["aircraft"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x["flightId"].as_str())
        .collect();
    assert!(ids_25.contains(fid_near_anon.as_str()));
    assert!(
        !ids_25.contains(fid_mid.as_str()),
        "30 km flight must be excluded at 25 km radius"
    );

    let (st, _, _) = send_req(
        &mut router,
        "PATCH",
        "/api/admin/club/nearby-public",
        Some(&adm_a_cookie),
        None,
        None,
        None,
        None,
        Some(json!({ "nearbyPublicRadiusKm": 100 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (_, _, map_100) = send_req(
        &mut router,
        "GET",
        "/api/display/map",
        Some(&adm_a_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    let ids_100: HashSet<&str> = map_100["aircraft"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x["flightId"].as_str())
        .collect();
    assert!(
        ids_100.contains(fid_far.as_str()),
        "65 km flight must be included at 100 km radius"
    );

    // 3) Toggle OFF -> server returns ZERO nearby public flights on ALL 4 endpoints
    let (st, _, body_off) = send_req(
        &mut router,
        "PATCH",
        "/api/admin/club/nearby-public",
        Some(&adm_a_cookie),
        None,
        None,
        None,
        None,
        Some(json!({ "showNearbyPublic": false })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body_off}");
    assert_eq!(body_off["showNearbyPublic"], json!(false));

    for uri in ["/api/admin/map", "/api/display/map"] {
        let (_, _, m) = send_req(
            &mut router,
            "GET",
            uri,
            Some(&adm_a_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert!(
            m["aircraft"].as_array().unwrap().is_empty(),
            "{uri} must return zero nearby flights when toggled off"
        );
    }
    for uri in ["/api/admin/map/trails", "/api/display/map/trails"] {
        let (_, _, tr) = send_req(
            &mut router,
            "GET",
            uri,
            Some(&adm_a_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert!(
            tr["trails"].as_array().unwrap().is_empty(),
            "{uri} must return zero nearby trails when toggled off"
        );
    }

    // 4) Toggle back ON, then remove Club A's base coordinates -> ZERO nearby flights returned
    let (st, _, _) = send_req(
        &mut router,
        "PATCH",
        "/api/admin/club/nearby-public",
        Some(&adm_a_cookie),
        None,
        None,
        None,
        None,
        Some(json!({ "showNearbyPublic": true })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    sqlx::query(
        "UPDATE clubs SET aerodrome_icao = NULL, latitude = NULL, longitude = NULL WHERE id = $1",
    )
    .bind(&club_a)
    .execute(&db.pool)
    .await
    .unwrap();

    for uri in ["/api/admin/map", "/api/display/map"] {
        let (_, _, m) = send_req(
            &mut router,
            "GET",
            uri,
            Some(&adm_a_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert!(
            m["aircraft"].as_array().unwrap().is_empty(),
            "{uri} must return zero nearby flights when club has no base"
        );
    }
}

#[tokio::test]
#[serial]
async fn club_view_nearby_public_cap_keeps_nearest_30() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;

    let (club_a, _) = create_live_club(
        &db,
        "np-cap-a",
        "own-npca@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    let (club_b, _) = create_live_club(
        &db,
        "np-cap-b",
        "own-npcb@test",
        Some(("SCEL", -33.393, -70.786)),
    )
    .await;

    let (_adm_a, adm_a_cookie) =
        seed_member(&db, &club_a, "Admin A", "adm-npcap@test", "admin").await;
    db.upsert_type(&club_b, "C152").await.unwrap();

    let now = now_ms();
    let total = MAX_NEARBY_PUBLIC_FLIGHTS + 4;
    let mut fids_by_dist = Vec::with_capacity(total);
    for i in 0..total {
        let tail = format!("CC-NP{i:02}");
        let (pid, _) = seed_member(
            &db,
            &club_b,
            &format!("NP {i}"),
            &format!("npcap-{i}@test"),
            "pilot",
        )
        .await;
        // Spread from ~1 km to ~34 km north of SCTB (all within 50 km default radius)
        let lat = -33.456 + (i + 1) as f64 * 0.009;
        let (fid, _) = seed_live_flight(
            &db,
            &club_b,
            &pid,
            &tail,
            FlightVisibility::Public,
            false,
            None,
            &make_fixes(now, lat, -70.548, &[2, 0]),
        )
        .await;
        fids_by_dist.push(fid);
    }

    let (st, _, body) = send_req(
        &mut router,
        "GET",
        "/api/display/map",
        Some(&adm_a_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let ac = body["aircraft"].as_array().unwrap();
    assert_eq!(ac.len(), MAX_NEARBY_PUBLIC_FLIGHTS);
    let returned: HashSet<&str> = ac.iter().filter_map(|x| x["flightId"].as_str()).collect();
    // Nearest 30 kept, farthest 4 dropped
    for fid in &fids_by_dist[..MAX_NEARBY_PUBLIC_FLIGHTS] {
        assert!(returned.contains(fid.as_str()));
    }
    for fid in &fids_by_dist[MAX_NEARBY_PUBLIC_FLIGHTS..] {
        assert!(!returned.contains(fid.as_str()));
    }
}

#[tokio::test]
#[serial]
async fn patch_admin_club_nearby_public_auth_csrf_and_validation() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;

    let (club_a, _) = create_live_club(
        &db,
        "np-auth",
        "own-npa@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    let (_adm, adm_cookie) = seed_member(&db, &club_a, "Admin", "adm-npa@test", "admin").await;
    let (_staff, staff_cookie) = seed_member(&db, &club_a, "Staff", "stf-npa@test", "staff").await;
    let (_pilot, pilot_cookie) = seed_member(&db, &club_a, "Pilot", "pil-npa@test", "pilot").await;

    // Unauthenticated -> 401
    let (st, _, _) = send_req(
        &mut router,
        "PATCH",
        "/api/admin/club/nearby-public",
        None,
        None,
        None,
        None,
        None,
        Some(json!({ "showNearbyPublic": false })),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Pilot -> 403
    let (st, _, _) = send_req(
        &mut router,
        "PATCH",
        "/api/admin/club/nearby-public",
        Some(&pilot_cookie),
        None,
        None,
        None,
        None,
        Some(json!({ "showNearbyPublic": false })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Staff -> 403
    let (st, _, _) = send_req(
        &mut router,
        "PATCH",
        "/api/admin/club/nearby-public",
        Some(&staff_cookie),
        None,
        None,
        None,
        None,
        Some(json!({ "showNearbyPublic": false })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Cross-site Origin -> 403
    let (st, _, _) = send_req(
        &mut router,
        "PATCH",
        "/api/admin/club/nearby-public",
        Some(&adm_cookie),
        None,
        None,
        Some("https://evil.example"),
        None,
        Some(json!({ "showNearbyPublic": false })),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // Wrong Content-Type -> 415
    let (st, _, _) = send_req(
        &mut router,
        "PATCH",
        "/api/admin/club/nearby-public",
        Some(&adm_cookie),
        None,
        None,
        None,
        Some("text/plain"),
        Some(json!({ "showNearbyPublic": false })),
    )
    .await;
    assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE);

    // Invalid payloads -> 400
    for bad in [
        json!({}),
        json!({ "showNearbyPublic": "yes" }),
        json!({ "nearbyPublicRadiusKm": 4 }),
        json!({ "nearbyPublicRadiusKm": 501 }),
        json!({ "nearbyPublicRadiusKm": "50" }),
        json!({ "nearbyPublicRadiusKm": 50.5 }),
        json!({ "nearbyPublicRadiusKm": null }),
    ] {
        let (st, _, _) = send_req(
            &mut router,
            "PATCH",
            "/api/admin/club/nearby-public",
            Some(&adm_cookie),
            None,
            None,
            None,
            None,
            Some(bad),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
    }

    // Valid GET & PATCH -> 200
    let (st, _, body) = send_req(
        &mut router,
        "PATCH",
        "/api/admin/club/nearby-public",
        Some(&adm_cookie),
        None,
        None,
        None,
        None,
        Some(json!({ "showNearbyPublic": false, "nearbyPublicRadiusKm": 100 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["showNearbyPublic"], json!(false));
    assert_eq!(body["nearbyPublicRadiusKm"], json!(100));

    let (st_get, _, get_body) = send_req(
        &mut router,
        "GET",
        "/api/admin/club/nearby-public",
        Some(&adm_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_get, StatusCode::OK);
    assert_eq!(get_body["showNearbyPublic"], json!(false));
    assert_eq!(get_body["nearbyPublicRadiusKm"], json!(100));
}

pub(super) fn make_fixes_with_alt(
    latest_t: i64,
    lat: f64,
    lon: f64,
    specs: &[(i64, Option<f64>, Option<&str>)],
) -> Vec<LiveFix> {
    specs
        .iter()
        .enumerate()
        .map(|(i, &(min_ago, alt, alt_ref))| LiveFix {
            t: latest_t - min_ago * 60_000,
            lat: lat + i as f64 * 0.001,
            lon: lon + i as f64 * 0.001,
            alt,
            alt_ref: alt_ref.map(str::to_string),
            speed: Some(32.0),
            course: Some(90.0),
            accuracy: Some(5.0),
        })
        .collect()
}

pub(super) struct FlightDataEnvGuard {
    prev: Option<std::ffi::OsString>,
}

impl FlightDataEnvGuard {
    pub(super) fn set(path: &std::path::Path) -> Self {
        let prev = std::env::var_os("FLIGHT_DATA");
        std::env::set_var("FLIGHT_DATA", path);
        Self { prev }
    }

    pub(super) fn update(&self, path: &std::path::Path) {
        std::env::set_var("FLIGHT_DATA", path);
    }
}

impl Drop for FlightDataEnvGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var("FLIGHT_DATA", v),
            None => std::env::remove_var("FLIGHT_DATA"),
        }
    }
}

#[tokio::test]
#[serial]
async fn club_view_nearby_public_altitude_filter_msl_alt_ref_missing_and_legacy_ignored() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;

    let (club_a, _) = create_live_club(
        &db,
        "np-alt-a",
        "own-npalta@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    let (club_b, _) = create_live_club(
        &db,
        "np-alt-b",
        "own-npaltb@test",
        Some(("SCEL", -33.393, -70.786)),
    )
    .await;

    // Configure Club A's public altitude filter to MSL >= 1500 ft, and set legacy
    // show_public_tracks = false / tiny legacy radius & cap to verify only the altitude
    // filter (and NOT legacy show_public_tracks / public_radius_km / public_flights_cap)
    // governs nearby public flights.
    sqlx::query(
        "UPDATE clubs
         SET public_altitude_mode = 'msl',
             public_altitude_min_ft = 1500.0,
             show_public_tracks = FALSE,
             public_radius_km = 1.0,
             public_flights_cap = 1
         WHERE id = $1",
    )
    .bind(&club_a)
    .execute(&db.pool)
    .await
    .unwrap();

    let (_adm_a, adm_a_cookie) =
        seed_member(&db, &club_a, "Admin A", "adm-npalt@test", "admin").await;
    let (p_own_low, _) = seed_member(&db, &club_a, "Own Low", "ownlow@test", "pilot").await;
    let (pb_above, _) = seed_member(&db, &club_b, "Above MSL", "pba@test", "pilot").await;
    let (pb_below, _) = seed_member(&db, &club_b, "Below MSL", "pbb@test", "pilot").await;
    let (pb_missing, _) = seed_member(&db, &club_b, "No Alt", "pbm@test", "pilot").await;
    let (pb_hae_pass, _) = seed_member(&db, &club_b, "HAE Pass", "pbh@test", "pilot").await;
    let (pb_msl_fail, _) = seed_member(&db, &club_b, "MSL Fail", "pbf@test", "pilot").await;

    let now = now_ms();
    // Own-club flight well below 1500 ft MSL -> exempt from public altitude filter.
    let (fid_own_low, _) = seed_live_flight(
        &db,
        &club_a,
        &p_own_low,
        "CC-OWNLO",
        FlightVisibility::Club,
        false,
        None,
        &make_fixes_with_alt(
            now,
            -33.455,
            -70.548,
            &[(4, Some(100.0), Some("msl")), (0, Some(100.0), Some("msl"))],
        ),
    )
    .await;

    // Nearby public flight above 1500 ft MSL (600 m = ~1968.5 ft) -> included.
    let (fid_above_msl, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_above,
        "CC-ALTUP",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(
            now,
            -33.42,
            -70.548,
            &[(4, Some(500.0), Some("msl")), (0, Some(600.0), Some("msl"))],
        ),
    )
    .await;

    // Nearby public flight whose earlier fix was above min (800 m) but LATEST fix is below
    // 1500 ft MSL (300 m = ~984.3 ft) -> excluded.
    let (fid_below_msl, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_below,
        "CC-ALTLO",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(
            now,
            -33.49,
            -70.548,
            &[(4, Some(800.0), Some("msl")), (0, Some(300.0), Some("msl"))],
        ),
    )
    .await;

    // Nearby public flight whose latest fix has missing altitude -> excluded.
    let (fid_missing_alt, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_missing,
        "CC-NOALT",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(
            now,
            -33.41,
            -70.548,
            &[(4, Some(800.0), Some("msl")), (0, None, None)],
        ),
    )
    .await;

    // 440 m HAE at lat -33.40 has undulation ~ -30.06 m -> ~470.06 m MSL (~1542 ft >= 1500 ft)
    // -> included; proves alt_ref is carried from latest fix.
    let (fid_hae_pass, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_hae_pass,
        "CC-HAEOK",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(
            now,
            -33.40,
            -70.548,
            &[(4, Some(440.0), Some("hae")), (0, Some(440.0), Some("hae"))],
        ),
    )
    .await;

    // Same 440 m with alt_ref = "msl" is ~1443.6 ft < 1500 ft -> excluded.
    let (fid_msl_fail, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_msl_fail,
        "CC-MSLLO",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(
            now,
            -33.395,
            -70.548,
            &[(4, Some(440.0), Some("msl")), (0, Some(440.0), Some("msl"))],
        ),
    )
    .await;

    for map_uri in ["/api/admin/map", "/api/display/map"] {
        let (st, _, body) = send_req(
            &mut router,
            "GET",
            map_uri,
            Some(&adm_a_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{map_uri}: {body}");
        let ids: HashSet<&str> = body["aircraft"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x["flightId"].as_str())
            .collect();
        assert!(
            ids.contains(fid_own_low.as_str()),
            "{map_uri} own-club flight must be exempt from public altitude filter"
        );
        assert!(
            ids.contains(fid_above_msl.as_str()),
            "{map_uri} missing nearby public flight above 1500 ft MSL"
        );
        assert!(
            ids.contains(fid_hae_pass.as_str()),
            "{map_uri} missing nearby public HAE flight above 1500 ft MSL after undulation"
        );
        assert!(
            !ids.contains(fid_below_msl.as_str()),
            "{map_uri} leaked nearby public flight below 1500 ft MSL"
        );
        assert!(
            !ids.contains(fid_missing_alt.as_str()),
            "{map_uri} leaked nearby public flight with missing altitude"
        );
        assert!(
            !ids.contains(fid_msl_fail.as_str()),
            "{map_uri} leaked nearby public 440 m MSL flight below 1500 ft"
        );
    }

    for tr_uri in ["/api/admin/map/trails", "/api/display/map/trails"] {
        let (st, _, body) = send_req(
            &mut router,
            "GET",
            tr_uri,
            Some(&adm_a_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{tr_uri}: {body}");
        let trails = body["trails"].as_array().unwrap();
        let ids: HashSet<&str> = trails
            .iter()
            .filter_map(|t| t["flightId"].as_str())
            .collect();
        assert!(
            ids.contains(fid_own_low.as_str()),
            "{tr_uri} own-club trail must be exempt"
        );
        assert!(
            ids.contains(fid_above_msl.as_str()),
            "{tr_uri} missing trail for nearby public flight above min"
        );
        assert!(
            ids.contains(fid_hae_pass.as_str()),
            "{tr_uri} missing trail for nearby public HAE flight above min"
        );
        assert!(
            !ids.contains(fid_below_msl.as_str()),
            "{tr_uri} leaked trail for filtered-out low-altitude flight"
        );
        assert!(
            !ids.contains(fid_missing_alt.as_str()),
            "{tr_uri} leaked trail for filtered-out missing-altitude flight"
        );
        assert!(
            !ids.contains(fid_msl_fail.as_str()),
            "{tr_uri} leaked trail for filtered-out 440 m MSL flight"
        );
    }
}

#[tokio::test]
#[serial]
async fn club_view_nearby_public_altitude_filter_runs_before_nearest_30_cap() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;

    let (club_a, _) = create_live_club(
        &db,
        "np-altcap-a",
        "own-npaca@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    let (club_b, _) = create_live_club(
        &db,
        "np-altcap-b",
        "own-npacb@test",
        Some(("SCEL", -33.393, -70.786)),
    )
    .await;

    sqlx::query(
        "UPDATE clubs
         SET public_altitude_mode = 'msl',
             public_altitude_min_ft = 1500.0
         WHERE id = $1",
    )
    .bind(&club_a)
    .execute(&db.pool)
    .await
    .unwrap();

    let (_adm_a, adm_a_cookie) =
        seed_member(&db, &club_a, "Admin A", "adm-npac@test", "admin").await;

    let now = now_ms();
    // Seed 30 closer nearby public flights (~1..28 km N of SCTB) below 1500 ft MSL.
    for i in 0..MAX_NEARBY_PUBLIC_FLIGHTS {
        let tail = format!("CC-LO{i:02}");
        let (pid, _) = seed_member(
            &db,
            &club_b,
            &format!("Low {i}"),
            &format!("npac-lo-{i}@test"),
            "pilot",
        )
        .await;
        let lat = -33.456 + (i + 1) as f64 * 0.008;
        seed_live_flight(
            &db,
            &club_b,
            &pid,
            &tail,
            FlightVisibility::Public,
            false,
            None,
            &make_fixes_with_alt(
                now,
                lat,
                -70.548,
                &[(3, Some(300.0), Some("msl")), (0, Some(300.0), Some("msl"))],
            ),
        )
        .await;
    }

    // Seed 1 farther nearby public flight (~35 km N, still within 50 km) above 1500 ft MSL.
    let (p_far_high, _) = seed_member(&db, &club_b, "Far High", "npac-hi@test", "pilot").await;
    let (fid_far_high, _) = seed_live_flight(
        &db,
        &club_b,
        &p_far_high,
        "CC-FARHI",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(
            now,
            -33.14,
            -70.548,
            &[(3, Some(800.0), Some("msl")), (0, Some(800.0), Some("msl"))],
        ),
    )
    .await;

    for (uri, field) in [
        ("/api/admin/map", "aircraft"),
        ("/api/display/map", "aircraft"),
        ("/api/admin/map/trails", "trails"),
        ("/api/display/map/trails", "trails"),
    ] {
        let (st, _, body) = send_req(
            &mut router,
            "GET",
            uri,
            Some(&adm_a_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{uri}: {body}");
        let items = body[field].as_array().unwrap();
        assert_eq!(
            items.len(),
            1,
            "{uri} must drop 30 low flights before cap and keep the farther high flight"
        );
        assert_eq!(items[0]["flightId"], json!(fid_far_high));
    }
}

#[tokio::test]
#[serial]
async fn club_view_nearby_public_altitude_filter_agl_with_dem_and_fallback() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;

    // Deterministic 600 m flat DEM tile covering (-34..-33, -71..-70) -> N34W071.asc.
    let dem_root = tempfile::tempdir().unwrap();
    let dem_dir = dem_root.path().join("dem");
    std::fs::create_dir_all(&dem_dir).unwrap();
    std::fs::write(
        dem_dir.join("N34W071.asc"),
        "# 600 m flat terrain\n600 600 600\n600 600 600\n600 600 600\n",
    )
    .unwrap();
    let empty_root = tempfile::tempdir().unwrap();
    let env_guard = FlightDataEnvGuard::set(dem_root.path());

    let (club_a, _) = create_live_club(
        &db,
        "np-agl-a",
        "own-npagla@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    let (club_b, _) = create_live_club(
        &db,
        "np-agl-b",
        "own-npaglb@test",
        Some(("SCEL", -33.393, -70.786)),
    )
    .await;

    sqlx::query(
        "UPDATE clubs
         SET public_altitude_mode = 'agl',
             public_altitude_min_ft = 1000.0
         WHERE id = $1",
    )
    .bind(&club_a)
    .execute(&db.pool)
    .await
    .unwrap();

    let (_adm_a, adm_a_cookie) =
        seed_member(&db, &club_a, "Admin A", "adm-npagl@test", "admin").await;
    let (pb_low_agl, _) = seed_member(&db, &club_b, "Low AGL", "pbal@test", "pilot").await;
    let (pb_high_agl, _) = seed_member(&db, &club_b, "High AGL", "pbah@test", "pilot").await;

    let now = now_ms();
    // 800 m MSL (~2625 ft MSL) over 600 m terrain = 200 m AGL (~656 ft < 1000 ft AGL) ->
    // excluded when DEM tile is present, included via NoDemFallbackMsl when DEM is absent.
    let (fid_low_agl, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_low_agl,
        "CC-AGLLO",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(
            now,
            -33.42,
            -70.55,
            &[(3, Some(800.0), Some("msl")), (0, Some(800.0), Some("msl"))],
        ),
    )
    .await;

    // 1000 m MSL over 600 m terrain = 400 m AGL (~1312 ft >= 1000 ft AGL) -> included.
    let (fid_high_agl, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_high_agl,
        "CC-AGLHI",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(
            now,
            -33.43,
            -70.55,
            &[
                (3, Some(1000.0), Some("msl")),
                (0, Some(1000.0), Some("msl")),
            ],
        ),
    )
    .await;

    // 1) With DEM tile present -> AGL check drops fid_low_agl and keeps fid_high_agl on all 4 endpoints.
    for (uri, field) in [
        ("/api/admin/map", "aircraft"),
        ("/api/display/map", "aircraft"),
        ("/api/admin/map/trails", "trails"),
        ("/api/display/map/trails", "trails"),
    ] {
        let (st, _, body) = send_req(
            &mut router,
            "GET",
            uri,
            Some(&adm_a_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{uri}: {body}");
        let ids: HashSet<&str> = body[field]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x["flightId"].as_str())
            .collect();
        assert!(
            ids.contains(fid_high_agl.as_str()),
            "{uri} missing flight above 1000 ft AGL"
        );
        assert!(
            !ids.contains(fid_low_agl.as_str()),
            "{uri} leaked flight below 1000 ft AGL over 600 m DEM"
        );
    }

    // 2) Without DEM coverage -> NoDemFallbackMsl compares MSL (~2625 ft >= 1000 ft), keeping both.
    env_guard.update(empty_root.path());
    for (uri, field) in [
        ("/api/admin/map", "aircraft"),
        ("/api/display/map", "aircraft"),
        ("/api/admin/map/trails", "trails"),
        ("/api/display/map/trails", "trails"),
    ] {
        let (st, _, body) = send_req(
            &mut router,
            "GET",
            uri,
            Some(&adm_a_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{uri}: {body}");
        let ids: HashSet<&str> = body[field]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x["flightId"].as_str())
            .collect();
        assert!(ids.contains(fid_high_agl.as_str()));
        assert!(
            ids.contains(fid_low_agl.as_str()),
            "{uri} must keep flight via NoDemFallbackMsl when DEM tile is absent"
        );
    }
}

#[test]
fn live_trails_query_parser_validates_public_location_and_radius() {
    // Club-only ignores coordinates and radius (even malformed values)
    for ok_club in [
        None,
        Some("layers=club"),
        Some("layers=club&lat=-33.456&lon=-70.548"),
        Some("layers=club&lat=999&lon=abc&radius_km=NaN"),
    ] {
        let parsed = LiveTrailsQuery::parse_query(ok_club).expect("club-only query must succeed");
        assert_eq!(parsed.layers, LiveTrailsLayers::CLUB_ONLY);
        assert!(parsed.public_area.is_none());
    }

    // Default radius = 20 km and default fail-closed AGL floor = 300 ft when omitted (L-76)
    let def = LiveTrailsQuery::parse_query(Some("layers=public&lat=-33.456&lon=-70.548")).unwrap();
    assert_eq!(
        def.public_area,
        Some(PublicArea {
            lat: -33.456,
            lon: -70.548,
            radius_km: PILOT_LIVE_TRAILS_DEFAULT_PUBLIC_RADIUS_KM,
            min_agl_ft: Some(crate::club::map_filter::DEFAULT_PUBLIC_ALTITUDE_MIN_FT),
        })
    );

    // Server-side clamping to 5..=500
    let low = LiveTrailsQuery::parse_query(Some("layers=public&lat=0&lon=0&radius_km=1")).unwrap();
    assert_eq!(
        low.public_area.unwrap().radius_km,
        PILOT_LIVE_TRAILS_MIN_PUBLIC_RADIUS_KM
    );
    let neg = LiveTrailsQuery::parse_query(Some("layers=public&lat=-0.0&lon=-0.0&radius_km=-10"))
        .unwrap();
    assert_eq!(
        neg.public_area,
        Some(PublicArea {
            lat: 0.0,
            lon: 0.0,
            radius_km: PILOT_LIVE_TRAILS_MIN_PUBLIC_RADIUS_KM,
            min_agl_ft: Some(crate::club::map_filter::DEFAULT_PUBLIC_ALTITUDE_MIN_FT),
        })
    );
    let high =
        LiveTrailsQuery::parse_query(Some("layers=club,public&lat=90&lon=-180&radius_km=9999"))
            .unwrap();
    assert_eq!(
        high.public_area.unwrap().radius_km,
        PILOT_LIVE_TRAILS_MAX_PUBLIC_RADIUS_KM
    );

    // Invalid / missing location when public is requested -> "invalid location"
    for bad_loc in [
        "layers=public",
        "layers=club,public",
        "layers=public&lat=-33.456",
        "layers=public&lon=-70.548",
        "layers=public&lat=91&lon=-70.548",
        "layers=public&lat=-91&lon=-70.548",
        "layers=public&lat=-33.456&lon=181",
        "layers=public&lat=-33.456&lon=-181",
        "layers=public&lat=NaN&lon=-70.548",
        "layers=public&lat=inf&lon=-70.548",
        "layers=public&lat=-33.456&lon=-inf",
        "layers=public&lat=abc&lon=-70.548",
        "layers=public&lat=&lon=-70.548",
        "layers=public&lat=0&lat=1&lon=0",
    ] {
        assert_eq!(
            LiveTrailsQuery::parse_query(Some(bad_loc)),
            Err("invalid location"),
            "expected invalid location for `{bad_loc}`"
        );
    }

    // Invalid radius_km -> "invalid radius_km"
    for bad_radius in [
        "layers=public&lat=0&lon=0&radius_km=abc",
        "layers=public&lat=0&lon=0&radius_km=NaN",
        "layers=public&lat=0&lon=0&radius_km=inf",
        "layers=public&lat=0&lon=0&radius_km=-inf",
        "layers=public&lat=0&lon=0&radius_km=",
        "layers=public&lat=0&lon=0&radius_km=10&radius_km=20",
    ] {
        assert_eq!(
            LiveTrailsQuery::parse_query(Some(bad_radius)),
            Err("invalid radius_km"),
            "expected invalid radius_km for `{bad_radius}`"
        );
    }
}

#[tokio::test]
#[serial]
async fn pilot_live_trails_public_layer_geo_radius_clamping_validation_and_etag() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;
    let (_dem_root, _dem_guard) = make_flat_dem(&[("N34W071", 600.0), ("N29W071", 600.0)]);

    let center_lat = -33.456;
    let center_lon = -70.548;

    let (club_a, _) = create_live_club(
        &db,
        "plt-geo-a",
        "own-pga@test",
        Some(("SCTB", center_lat, center_lon)),
    )
    .await;
    let (club_b, _) = create_live_club(
        &db,
        "plt-geo-b",
        "own-pgb@test",
        Some(("SCEL", -33.393, -70.786)),
    )
    .await;

    let (_viewer_a, viewer_cookie) =
        seed_member(&db, &club_a, "Geo Viewer", "pga-v@test", "pilot").await;
    let (peer_a_far, _) = seed_member(&db, &club_a, "Far Club Peer", "pga-far@test", "pilot").await;

    let now = now_ms();
    let exact_fix = |lat: f64, lon: f64, t: i64| LiveFix {
        t,
        lat,
        lon,
        alt: Some(1200.0),
        alt_ref: Some("msl".to_string()),
        speed: Some(32.0),
        course: Some(0.0),
        accuracy: Some(5.0),
    };

    // Own-club peer > 600 km away: club layer is NOT geo-filtered, so it always appears on layers=club.
    let (fid_club_far, _) = seed_live_flight(
        &db,
        &club_a,
        &peer_a_far,
        "CC-CLFAR",
        FlightVisibility::Club,
        false,
        None,
        &[exact_fix(-27.0, center_lon, now)],
    )
    .await;

    // Seed public flights from Club B at exact distances north of (center_lat, center_lon):
    // 1 deg lat ≈ 111.1949 km.
    let specs: [(&str, &str, f64, f64, f64, f64); 6] = [
        ("CC-D004", "pg-d4@test", -33.4200, center_lon, 3.9, 4.1),
        ("CC-D006", "pg-d6@test", -33.4020, center_lon, 5.9, 6.1),
        ("CC-D019", "pg-d19@test", -33.2850, center_lon, 18.9, 19.2),
        ("CC-D021", "pg-d21@test", -33.2670, center_lon, 20.9, 21.2),
        (
            "CC-D499",
            "pg-d499@test",
            -28.9685,
            center_lon,
            498.5,
            499.5,
        ),
        (
            "CC-D501",
            "pg-d501@test",
            -28.9505,
            center_lon,
            500.5,
            501.5,
        ),
    ];
    let mut fids_by_tail = std::collections::HashMap::new();
    for (tail, email, lat, lon, min_d, max_d) in specs {
        let d = haversine_km(center_lat, center_lon, lat, lon);
        assert!(
            (min_d..=max_d).contains(&d),
            "{tail} expected in {min_d}..={max_d} km, got {d:.3} km"
        );
        let (pid, _) = seed_member(&db, &club_b, tail, email, "pilot").await;
        let (fid, _) = seed_live_flight(
            &db,
            &club_b,
            &pid,
            tail,
            FlightVisibility::Public,
            false,
            None,
            &[exact_fix(lat, lon, now - 60_000), exact_fix(lat, lon, now)],
        )
        .await;
        fids_by_tail.insert(tail, fid);
    }

    // Flight whose earlier fix was AT center (0 km), but LATEST fix moved ~28.5 km away (> 20 km).
    let (p_moved, _) = seed_member(&db, &club_b, "Moved Out", "pg-mv@test", "pilot").await;
    let moved_lat = -33.200;
    assert!(haversine_km(center_lat, center_lon, moved_lat, center_lon) > 25.0);
    let (fid_moved_out, _) = seed_live_flight(
        &db,
        &club_b,
        &p_moved,
        "CC-MVOUT",
        FlightVisibility::Public,
        false,
        None,
        &[
            exact_fix(center_lat, center_lon, now - 120_000),
            exact_fix(moved_lat, center_lon, now),
        ],
    )
    .await;

    // Diagonal bounding-box corner: inside 20 km SQL lat/lon bbox, but great-circle distance ~22.6 km > 20 km!
    let corner_lat = -33.312;
    let corner_lon = -70.375;
    let corner_dist = haversine_km(center_lat, center_lon, corner_lat, corner_lon);
    assert!(
        corner_dist > 21.0 && corner_dist < 24.0,
        "corner_dist={corner_dist}"
    );
    let (p_corner, _) = seed_member(&db, &club_b, "Corner", "pg-cn@test", "pilot").await;
    let (fid_corner, _) = seed_live_flight(
        &db,
        &club_b,
        &p_corner,
        "CC-CRNR",
        FlightVisibility::Public,
        false,
        None,
        &[exact_fix(corner_lat, corner_lon, now)],
    )
    .await;

    // 1) Missing or invalid lat/lon when public is requested -> 400 {"error":"invalid location"}
    for bad_loc_uri in [
        "/api/me/live-trails?layers=public",
        "/api/me/live-trails?layers=club,public",
        "/api/me/live-trails?layers=public&lat=-33.456",
        "/api/me/live-trails?layers=public&lon=-70.548",
        "/api/me/live-trails?layers=public&lat=91&lon=-70.548",
        "/api/me/live-trails?layers=public&lat=-91&lon=-70.548",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=181",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-181",
        "/api/me/live-trails?layers=public&lat=NaN&lon=-70.548",
        "/api/me/live-trails?layers=public&lat=inf&lon=-70.548",
    ] {
        let (st, _, body) = send_req(
            &mut router,
            "GET",
            bad_loc_uri,
            Some(&viewer_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "uri={bad_loc_uri} body={body}");
        assert_eq!(
            body,
            json!({ "error": "invalid location" }),
            "uri={bad_loc_uri}"
        );
    }

    // Invalid radius_km -> 400
    for bad_rad_uri in [
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&radius_km=abc",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&radius_km=NaN",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&radius_km=inf",
    ] {
        let (st, _, body) = send_req(
            &mut router,
            "GET",
            bad_rad_uri,
            Some(&viewer_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "uri={bad_rad_uri} body={body}");
        assert!(body.get("error").and_then(Value::as_str).is_some());
    }

    // 2) layers=club works without coordinates AND ignores coordinates when sent (never 400)
    for club_uri in [
        "/api/me/live-trails",
        "/api/me/live-trails?layers=club",
        "/api/me/live-trails?layers=club&lat=91&lon=abc&radius_km=NaN",
    ] {
        let (st, _, body) = send_req(
            &mut router,
            "GET",
            club_uri,
            Some(&viewer_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "uri={club_uri} body={body}");
        assert!(body.get("publicArea").is_none());
        let ids: HashSet<&str> = body["flights"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|f| f["flightId"].as_str())
            .collect();
        assert!(
            ids.contains(fid_club_far.as_str()),
            "club layer must stay unfiltered"
        );
        assert!(
            !ids.contains(fids_by_tail["CC-D004"].as_str()),
            "club-only must never leak public flights"
        );
    }

    // 3) Default radius (20 km when omitted): shows 4, 6, 19 km; hides 21, 499, 501, moved_out, corner
    let (st, headers_def, body_def) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548",
        Some(&viewer_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body_def}");
    assert_eq!(
        body_def["publicArea"],
        json!({ "lat": center_lat, "lon": center_lon, "radiusKm": 20.0, "minAglFt": 300.0 })
    );
    let ids_def: HashSet<&str> = body_def["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(ids_def.contains(fids_by_tail["CC-D004"].as_str()));
    assert!(ids_def.contains(fids_by_tail["CC-D006"].as_str()));
    assert!(ids_def.contains(fids_by_tail["CC-D019"].as_str()));
    assert!(!ids_def.contains(fids_by_tail["CC-D021"].as_str()));
    assert!(!ids_def.contains(fids_by_tail["CC-D499"].as_str()));
    assert!(!ids_def.contains(fids_by_tail["CC-D501"].as_str()));
    assert!(
        !ids_def.contains(fid_moved_out.as_str()),
        "latest fix outside 20 km must be hidden"
    );
    assert!(
        !ids_def.contains(fid_corner.as_str()),
        "bbox corner outside 20 km circle must be hidden by exact haversine"
    );

    // 4) Lower clamping: radius_km=1 behaves as 5 km (4 km shown, 6 km hidden)
    let (st, headers_r1, body_r1) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&radius_km=1",
        Some(&viewer_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body_r1}");
    assert_eq!(
        body_r1["publicArea"],
        json!({ "lat": center_lat, "lon": center_lon, "radiusKm": 5.0, "minAglFt": 300.0 })
    );
    let ids_r1: HashSet<&str> = body_r1["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(ids_r1.contains(fids_by_tail["CC-D004"].as_str()));
    assert!(!ids_r1.contains(fids_by_tail["CC-D006"].as_str()));

    // 5) Upper clamping: radius_km=9999 behaves as 500 km (499 km shown, 501 km hidden)
    let (st, _, body_r9999) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&radius_km=9999",
        Some(&viewer_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body_r9999}");
    assert_eq!(
        body_r9999["publicArea"],
        json!({ "lat": center_lat, "lon": center_lon, "radiusKm": 500.0, "minAglFt": 300.0 })
    );
    let ids_r9999: HashSet<&str> = body_r9999["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(ids_r9999.contains(fids_by_tail["CC-D499"].as_str()));
    assert!(!ids_r9999.contains(fids_by_tail["CC-D501"].as_str()));

    // 6) ETag incorporates (lat, lon, effective radius_km):
    //    - radius_km=1 and radius_km=5 share ETag -> 304
    //    - omitted radius_km and radius_km=20 share ETag -> 304
    //    - different radius (10 vs 15, same flights!) or different lat produces a different ETag
    let etag_def = headers_def
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let etag_r1 = headers_r1
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let (st_304_def, _, _) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&radius_km=20",
        Some(&viewer_cookie),
        None,
        Some(&etag_def),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_304_def, StatusCode::NOT_MODIFIED);

    let (st_304_r5, _, _) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&radius_km=5",
        Some(&viewer_cookie),
        None,
        Some(&etag_r1),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_304_r5, StatusCode::NOT_MODIFIED);

    let (st_r10, h_r10, _) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&radius_km=10",
        Some(&viewer_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_r10, StatusCode::OK);
    let etag_r10 = h_r10
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let (st_r15, h_r15, _) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&radius_km=15",
        Some(&viewer_cookie),
        None,
        Some(&etag_r10),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(
        st_r15,
        StatusCode::OK,
        "different radius_km must not 304 against old ETag"
    );
    let etag_r15 = h_r15
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_ne!(etag_r10, etag_r15);

    let (st_shifted, h_shifted, _) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=public&lat=-33.455&lon=-70.548&radius_km=10",
        Some(&viewer_cookie),
        None,
        Some(&etag_r10),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(
        st_shifted,
        StatusCode::OK,
        "different lat/lon must not 304 against old ETag"
    );
    assert_ne!(
        h_shifted.get(header::ETAG).unwrap().to_str().unwrap(),
        etag_r10
    );
}

#[tokio::test]
#[serial]
async fn pilot_live_trails_public_layer_handles_antimeridian_wrap() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;
    let (_dem_root, _dem_guard) = make_flat_dem(&[("N00W180", 0.0), ("N00W179", 0.0)]);

    let (club_a, _) = create_live_club(&db, "plt-am-a", "own-ama@test", None).await;
    let (club_b, _) = create_live_club(&db, "plt-am-b", "own-amb@test", None).await;
    let (_viewer, viewer_cookie) =
        seed_member(&db, &club_a, "AM Viewer", "am-v@test", "pilot").await;
    let (p_across, _) = seed_member(&db, &club_b, "Across AM", "am-in@test", "pilot").await;
    let (p_far, _) = seed_member(&db, &club_b, "Far AM", "am-out@test", "pilot").await;

    let now = now_ms();
    let fix_at = |lat: f64, lon: f64| LiveFix {
        t: now,
        lat,
        lon,
        alt: Some(1200.0),
        alt_ref: Some("msl".to_string()),
        speed: Some(32.0),
        course: Some(90.0),
        accuracy: Some(5.0),
    };

    // Query at (0.0, 179.95), radius 20 km:
    // - (0.0, -179.95) is ~11.1 km east across the antimeridian -> INCLUDED
    // - (0.0, 179.50) is ~50.0 km west -> EXCLUDED
    let (fid_across, _) = seed_live_flight(
        &db,
        &club_b,
        &p_across,
        "CC-AM01",
        FlightVisibility::Public,
        false,
        None,
        &[fix_at(0.0, -179.95)],
    )
    .await;
    let (fid_far, _) = seed_live_flight(
        &db,
        &club_b,
        &p_far,
        "CC-AM02",
        FlightVisibility::Public,
        false,
        None,
        &[fix_at(0.0, 179.50)],
    )
    .await;

    let (st, _, body) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=public&lat=0&lon=179.95&radius_km=20",
        Some(&viewer_cookie),
        None,
        None,
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
    assert!(
        ids.contains(fid_across.as_str()),
        "flight 11 km across antimeridian must be returned"
    );
    assert!(
        !ids.contains(fid_far.as_str()),
        "flight 50 km away must be excluded"
    );
}

#[tokio::test]
#[serial]
async fn club_view_nearby_public_hides_ground_taxi_at_terrain_default_agl() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;

    // Deterministic 600 m flat DEM tile covering (-34..-33, -71..-70) -> N34W071.asc.
    let dem_root = tempfile::tempdir().unwrap();
    let dem_dir = dem_root.path().join("dem");
    std::fs::create_dir_all(&dem_dir).unwrap();
    std::fs::write(
        dem_dir.join("N34W071.asc"),
        "# 600 m flat terrain\n600 600 600\n600 600 600\n600 600 600\n",
    )
    .unwrap();
    let _env_guard = FlightDataEnvGuard::set(dem_root.path());

    // Leave club_a at DB defaults (public_altitude_mode = 'agl', public_altitude_min_ft = 300.0).
    let (club_a, _) = create_live_club(
        &db,
        "np-taxi-a",
        "own-nptxa@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    let (club_b, _) = create_live_club(
        &db,
        "np-taxi-b",
        "own-nptxb@test",
        Some(("SCEL", -33.393, -70.786)),
    )
    .await;

    let (_adm_a, adm_a_cookie) =
        seed_member(&db, &club_a, "Admin A", "adm-nptx@test", "admin").await;
    let (pb_taxi, _) = seed_member(&db, &club_b, "Ground Taxi", "pbtx@test", "pilot").await;
    let (pb_air, _) = seed_member(&db, &club_b, "Airborne", "pbair@test", "pilot").await;

    let now = now_ms();
    // Nearby public flight from Club B at terrain level: 600 m MSL over 600 m DEM = 0 ft AGL, speed 5 m/s.
    let taxi_fixes = vec![
        LiveFix {
            t: now - 120_000,
            lat: -33.420,
            lon: -70.550,
            alt: Some(600.0),
            alt_ref: Some("msl".to_string()),
            speed: Some(5.0),
            course: Some(90.0),
            accuracy: Some(5.0),
        },
        LiveFix {
            t: now,
            lat: -33.421,
            lon: -70.551,
            alt: Some(600.0),
            alt_ref: Some("msl".to_string()),
            speed: Some(5.0),
            course: Some(90.0),
            accuracy: Some(5.0),
        },
    ];
    let (fid_taxi, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_taxi,
        "CC-TAXI0",
        FlightVisibility::Public,
        false,
        None,
        &taxi_fixes,
    )
    .await;

    // Control airborne nearby public flight: 800 m MSL over 600 m DEM = 200 m (~656 ft >= default 300 ft AGL).
    let (fid_air, _) = seed_live_flight(
        &db,
        &club_b,
        &pb_air,
        "CC-AIR01",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(
            now,
            -33.430,
            -70.550,
            &[(2, Some(800.0), Some("msl")), (0, Some(800.0), Some("msl"))],
        ),
    )
    .await;

    for (uri, field) in [
        ("/api/admin/map", "aircraft"),
        ("/api/display/map", "aircraft"),
        ("/api/admin/map/trails", "trails"),
        ("/api/display/map/trails", "trails"),
    ] {
        let (st, _, body) = send_req(
            &mut router,
            "GET",
            uri,
            Some(&adm_a_cookie),
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{uri}: {body}");
        let ids: HashSet<&str> = body[field]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|x| x["flightId"].as_str())
            .collect();
        assert!(
            !ids.contains(fid_taxi.as_str()),
            "{uri} must hide nearby public ground/taxi flight at 0 ft AGL under default AGL mode"
        );
        assert!(
            ids.contains(fid_air.as_str()),
            "{uri} must show airborne nearby public flight above default 300 ft AGL"
        );
    }
}
