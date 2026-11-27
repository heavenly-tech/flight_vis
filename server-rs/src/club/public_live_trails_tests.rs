//! Tests for anonymous `GET /api/public/live-trails` and bundled fixes:
//! - Anonymous access, cookie/bearer ignored (no `Set-Cookie`), `Cache-Control: public, max-age=5`, no CORS
//! - Visibility & lifecycle scoping (`public` only, operational clubs, no ended/stale/NULL `club_id`)
//! - PII rules (`pilotName` only when `show_pilot_name_public`, never pilot/account/device IDs)
//! - Radius defaults & clamping (`5..=500`, default `20`), exact haversine, antimeridian wrap
//! - Strict query validation (`lat`, `lon`, `radius_km`, `min_agl_ft`)
//! - Per-IP rate limiting (30 req/min -> `429 {"error":"rate_limited"}` + `Retry-After`), trusted-proxy IP extraction, injectable/resettable throttle
//! - Weak ETag / `If-None-Match` -> `304 Not Modified`
//! - K-1: `GET /api/me/live-trails` runs auth BEFORE query validation (`401` signed-out, `400` signed-in)
//! - L-77: SQL `LIMIT` enforced on both authenticated & anonymous public-layer queries (`PUBLIC_LIVE_TRAILS_SQL_CANDIDATE_LIMIT == 500`)
//! - L-76: Optional `min_agl_ft` AGL floor (`0..=20000`) on `/api/me/live-trails` (public layer only)
//! - M-27: Mandatory fail-closed `300.0` ft AGL default floor (`300..=20000`) on anonymous `/api/public/live-trails`
//! - L-79: Hard distinct-key cap on `LoginThrottle` (`public_live_trails_throttle`)

use crate::{
    club::{
        auth::new_id,
        db::now_ms,
        geo::haversine_km,
        map_filter::DEFAULT_PUBLIC_ALTITUDE_MIN_FT,
        pilot_live_trails::{
            LiveTrailsCaller, LiveTrailsQuery, PublicArea,
            PILOT_LIVE_TRAILS_DEFAULT_PUBLIC_RADIUS_KM, PILOT_LIVE_TRAILS_MAX_FLIGHTS,
            PILOT_LIVE_TRAILS_MAX_PUBLIC_RADIUS_KM, PILOT_LIVE_TRAILS_MIN_PUBLIC_RADIUS_KM,
            PUBLIC_LIVE_TRAILS_CACHE_CONTROL, PUBLIC_LIVE_TRAILS_MIN_AGL_FT_MAX,
            PUBLIC_LIVE_TRAILS_MIN_AGL_FT_MIN, PUBLIC_LIVE_TRAILS_SQL_CANDIDATE_LIMIT,
        },
        pilot_live_trails_tests::{
            create_live_club, http_stack, http_stack_with_throttle, make_fixes,
            make_fixes_with_alt, seed_live_flight, seed_member, send_req, FlightDataEnvGuard,
        },
        trail::{TrailMinutes, MAX_TRAIL_POINTS, MAX_TRAIL_WINDOW_MIN},
        visibility::FlightVisibility,
        ClubDb,
    },
    live::LiveFix,
    LoginThrottle, OverflowPolicy, PUBLIC_LIVE_TRAILS_MAX_REQUESTS,
};
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, HeaderMap, Request, StatusCode},
};
use serde_json::{json, Value};
use serial_test::serial;
use std::{collections::HashSet, net::SocketAddr};
use tower::Service;

async fn send_public_req(
    router: &mut axum::Router,
    uri: &str,
    peer: Option<SocketAddr>,
    xff: Option<&str>,
    if_none_match: Option<&str>,
    cookie: Option<&str>,
    bearer: Option<&str>,
) -> (StatusCode, HeaderMap, Value) {
    let mut b = Request::builder().method("GET").uri(uri);
    if let Some(ip) = xff {
        b = b.header("x-forwarded-for", ip);
    }
    if let Some(inm) = if_none_match {
        b = b.header(header::IF_NONE_MATCH, inm);
    }
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    if let Some(t) = bearer {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let mut req = b.body(Body::empty()).unwrap();
    if let Some(addr) = peer {
        req.extensions_mut().insert(ConnectInfo(addr));
    }
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

// ---------------------------------------------------------------------------
// Unit tests: PublicArea parser (/api/me/live-trails & /api/public/live-trails)
// ---------------------------------------------------------------------------

#[test]
fn public_area_parser_validates_location_radius_and_min_agl_ft() {
    let def = PublicArea::parse_query(Some("lat=-33.456&lon=-70.548")).unwrap();
    assert_eq!(
        def,
        PublicArea {
            lat: -33.456,
            lon: -70.548,
            radius_km: PILOT_LIVE_TRAILS_DEFAULT_PUBLIC_RADIUS_KM,
            min_agl_ft: None,
        }
    );

    let clamped_low =
        PublicArea::parse_query(Some("lat=-0.0&lon=-0.0&radius_km=1&min_agl_ft=-50")).unwrap();
    assert_eq!(
        clamped_low,
        PublicArea {
            lat: 0.0,
            lon: 0.0,
            radius_km: PILOT_LIVE_TRAILS_MIN_PUBLIC_RADIUS_KM,
            min_agl_ft: Some(PUBLIC_LIVE_TRAILS_MIN_AGL_FT_MIN),
        }
    );

    let clamped_high =
        PublicArea::parse_query(Some("lat=45&lon=10&radius_km=9999&min_agl_ft=99999")).unwrap();
    assert_eq!(
        clamped_high,
        PublicArea {
            lat: 45.0,
            lon: 10.0,
            radius_km: PILOT_LIVE_TRAILS_MAX_PUBLIC_RADIUS_KM,
            min_agl_ft: Some(PUBLIC_LIVE_TRAILS_MIN_AGL_FT_MAX),
        }
    );

    for bad_loc in [
        None,
        Some(""),
        Some("lat=-33.456"),
        Some("lon=-70.548"),
        Some("lat=91&lon=0"),
        Some("lat=-91&lon=0"),
        Some("lat=0&lon=181"),
        Some("lat=0&lon=-181"),
        Some("lat=NaN&lon=0"),
        Some("lat=0&lon=inf"),
        Some("lat=abc&lon=0"),
        Some("lat=0&lat=1&lon=0"),
    ] {
        assert_eq!(
            PublicArea::parse_query(bad_loc),
            Err("invalid location"),
            "query={bad_loc:?}"
        );
    }

    for bad_rad in [
        "lat=0&lon=0&radius_km=abc",
        "lat=0&lon=0&radius_km=NaN",
        "lat=0&lon=0&radius_km=inf",
        "lat=0&lon=0&radius_km=",
        "lat=0&lon=0&radius_km=10&radius_km=20",
    ] {
        assert_eq!(
            PublicArea::parse_query(Some(bad_rad)),
            Err("invalid radius_km"),
            "query={bad_rad}"
        );
    }

    for bad_agl in [
        "lat=0&lon=0&min_agl_ft=abc",
        "lat=0&lon=0&min_agl_ft=NaN",
        "lat=0&lon=0&min_agl_ft=inf",
        "lat=0&lon=0&min_agl_ft=-inf",
        "lat=0&lon=0&min_agl_ft=",
        "lat=0&lon=0&min_agl_ft=100&min_agl_ft=200",
    ] {
        assert_eq!(
            PublicArea::parse_query(Some(bad_agl)),
            Err("invalid min_agl_ft"),
            "query={bad_agl}"
        );
    }

    // Club-only /api/me/live-trails still ignores min_agl_ft
    assert!(LiveTrailsQuery::parse_query(Some("layers=club&min_agl_ft=abc")).is_ok());

    // Anonymous /api/public/live-trails parser: default 300 ft, clamp 300..=20000, reject negative
    assert_eq!(
        PublicArea::parse_anonymous_query(Some("lat=-33.456&lon=-70.548")).unwrap(),
        PublicArea {
            lat: -33.456,
            lon: -70.548,
            radius_km: PILOT_LIVE_TRAILS_DEFAULT_PUBLIC_RADIUS_KM,
            min_agl_ft: Some(DEFAULT_PUBLIC_ALTITUDE_MIN_FT),
        }
    );
    assert_eq!(
        PublicArea::parse_anonymous_query(Some("lat=0&lon=0&min_agl_ft=0"))
            .unwrap()
            .min_agl_ft,
        Some(DEFAULT_PUBLIC_ALTITUDE_MIN_FT)
    );
    assert_eq!(
        PublicArea::parse_anonymous_query(Some("lat=0&lon=0&min_agl_ft=150"))
            .unwrap()
            .min_agl_ft,
        Some(DEFAULT_PUBLIC_ALTITUDE_MIN_FT)
    );
    assert_eq!(
        PublicArea::parse_anonymous_query(Some("lat=0&lon=0&min_agl_ft=1000"))
            .unwrap()
            .min_agl_ft,
        Some(1000.0)
    );
    assert_eq!(
        PublicArea::parse_anonymous_query(Some("lat=0&lon=0&min_agl_ft=50000"))
            .unwrap()
            .min_agl_ft,
        Some(PUBLIC_LIVE_TRAILS_MIN_AGL_FT_MAX)
    );
    for bad_anon_agl in [
        "lat=0&lon=0&min_agl_ft=-1",
        "lat=0&lon=0&min_agl_ft=-50",
        "lat=0&lon=0&min_agl_ft=-0.01",
        "lat=0&lon=0&min_agl_ft=abc",
        "lat=0&lon=0&min_agl_ft=NaN",
        "lat=0&lon=0&min_agl_ft=inf",
        "lat=0&lon=0&min_agl_ft=-inf",
        "lat=0&lon=0&min_agl_ft=",
        "lat=0&lon=0&min_agl_ft=400&min_agl_ft=500",
    ] {
        assert_eq!(
            PublicArea::parse_anonymous_query(Some(bad_anon_agl)),
            Err("invalid min_agl_ft"),
            "anon query={bad_anon_agl}"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests 1, 2, 3: Anonymous access, visibility/suspension/ended/stale/null-club
// filtering, pilotName opt-in, cookie/bearer ignored, headers
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn public_live_trails_anonymous_scoping_visibility_pii_and_headers() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;
    // 600 m flat DEM so airborne make_fixes (~930-960 m MSL) sit > 300 ft AGL.
    let (_dem_root, _env_guard) = make_flat_dem(&[("N34W071", 600.0)]);

    let (club_a, _) = create_live_club(
        &db,
        "pub-lt-a",
        "own-puba@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    db.set_club_trail_minutes_default(&club_a, TrailMinutes::Min10)
        .await
        .unwrap();

    let (club_susp, _) = create_live_club(
        &db,
        "pub-lt-s",
        "own-pubs@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;

    // Pending / unverified club
    let now = now_ms();
    let club_pend = new_id();
    let pend_slug = format!("pub-lt-p-{}", &new_id()[..8]);
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at)
         VALUES ($1, 'Pending Club', $2, $3, 'pending', NULL)",
    )
    .bind(&club_pend)
    .bind(&pend_slug)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

    let (p_pub_named, pilot_cookie) =
        seed_member(&db, &club_a, "Visible Pilot", "pub-named@test", "pilot").await;
    let (p_pub_anon, _) = seed_member(&db, &club_a, "Hidden Pilot", "pub-anon@test", "pilot").await;
    let (p_pub_off, _) = seed_member(&db, &club_a, "Off Pilot", "pub-off@test", "pilot").await;
    let (p_club_only, _) = seed_member(&db, &club_a, "Club Pilot", "pub-club@test", "pilot").await;
    let (p_private, _) = seed_member(&db, &club_a, "Priv Pilot", "pub-priv@test", "pilot").await;
    let (p_ended, _) = seed_member(&db, &club_a, "Ended Pilot", "pub-end@test", "pilot").await;
    let (p_stale, _) = seed_member(&db, &club_a, "Stale Pilot", "pub-stale@test", "pilot").await;
    let (p_null_club, _) = seed_member(&db, &club_a, "Null Club", "pub-null@test", "pilot").await;
    let (p_susp, _) = seed_member(&db, &club_susp, "Susp Pilot", "pub-susp@test", "pilot").await;
    let (p_pend, _) = seed_member(&db, &club_pend, "Pend Pilot", "pub-pend@test", "pilot").await;

    // 1) In-radius public flight with show_pilot_name_public = true, inherits Club A's 10-min default
    let (fid_pub_named, pilot_bearer) = seed_live_flight(
        &db,
        &club_a,
        &p_pub_named,
        "CC-PUBN",
        FlightVisibility::Public,
        true,
        None,
        &make_fixes(now, -33.450, -70.548, &[20, 8, 3, 0]),
    )
    .await;

    // 2) In-radius public flight with show_pilot_name_public = false, flight override 2 min
    let (fid_pub_anon, _) = seed_live_flight(
        &db,
        &club_a,
        &p_pub_anon,
        "CC-PUBA",
        FlightVisibility::Public,
        false,
        Some(TrailMinutes::Min2),
        &make_fixes(now, -33.460, -70.548, &[10, 1, 0]),
    )
    .await;

    // 3) In-radius public flight with trailMinutes = Off (0) -> latest point only
    let (fid_pub_off, _) = seed_live_flight(
        &db,
        &club_a,
        &p_pub_off,
        "CC-PUBO",
        FlightVisibility::Public,
        false,
        Some(TrailMinutes::Off),
        &make_fixes(now, -33.455, -70.540, &[5, 2, 0]),
    )
    .await;

    // 4) Non-public flights (club, private) -> MUST NOT appear
    let (fid_club_only, _) = seed_live_flight(
        &db,
        &club_a,
        &p_club_only,
        "CC-CLB1",
        FlightVisibility::Club,
        true,
        None,
        &make_fixes(now, -33.455, -70.548, &[2, 0]),
    )
    .await;
    let (fid_private, _) = seed_live_flight(
        &db,
        &club_a,
        &p_private,
        "CC-PRV1",
        FlightVisibility::Private,
        true,
        None,
        &make_fixes(now, -33.455, -70.549, &[2, 0]),
    )
    .await;

    // 5) Ended public flight -> MUST NOT appear
    let (fid_ended, _) = seed_live_flight(
        &db,
        &club_a,
        &p_ended,
        "CC-END1",
        FlightVisibility::Public,
        true,
        None,
        &make_fixes(now, -33.455, -70.545, &[2, 0]),
    )
    .await;
    db.end_flight(&p_ended, &fid_ended, now).await.unwrap();

    // 6) Stale public flight (> 10 min old) -> MUST NOT appear
    let (fid_stale, _) = seed_live_flight(
        &db,
        &club_a,
        &p_stale,
        "CC-STL1",
        FlightVisibility::Public,
        true,
        None,
        &make_fixes(now - 15 * 60_000, -33.455, -70.546, &[2, 0]),
    )
    .await;

    // 7) NULL club_id public flight -> MUST NOT appear
    let (fid_null_club, _) = seed_live_flight(
        &db,
        &club_a,
        &p_null_club,
        "CC-NUL1",
        FlightVisibility::Public,
        true,
        None,
        &make_fixes(now, -33.455, -70.547, &[2, 0]),
    )
    .await;
    sqlx::query(
        "UPDATE flights SET club_id = NULL, aircraft_id = NULL, tail_text = 'CC-NUL1' WHERE id = $1",
    )
    .bind(&fid_null_club)
    .execute(&db.pool)
    .await
    .unwrap();

    // 8) Suspended-club public flight -> MUST NOT appear
    let (fid_susp, _) = seed_live_flight(
        &db,
        &club_susp,
        &p_susp,
        "CC-SSP1",
        FlightVisibility::Public,
        true,
        None,
        &make_fixes(now, -33.455, -70.544, &[2, 0]),
    )
    .await;
    db.set_club_suspended(&club_susp, now).await.unwrap();

    // 9) Unverified/pending-club public flight -> MUST NOT appear
    let (fid_pend, _) = seed_live_flight(
        &db,
        &club_pend,
        &p_pend,
        "CC-PND1",
        FlightVisibility::Public,
        true,
        None,
        &make_fixes(now, -33.455, -70.543, &[2, 0]),
    )
    .await;

    // Anonymous request (no Cookie, no Authorization) -> 200
    let (st, headers, body) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548",
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        headers
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some(PUBLIC_LIVE_TRAILS_CACHE_CONTROL)
    );
    assert!(
        headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none(),
        "must not add CORS headers"
    );
    assert!(headers.get(header::SET_COOKIE).is_none());
    assert!(headers
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|e| e.starts_with("W/\"")));

    assert_eq!(
        body["caps"],
        json!({
            "maxFlights": PILOT_LIVE_TRAILS_MAX_FLIGHTS,
            "maxPointsPerFlight": MAX_TRAIL_POINTS,
            "maxWindowMinutes": MAX_TRAIL_WINDOW_MIN,
        })
    );
    assert_eq!(
        body["publicArea"],
        json!({
            "lat": -33.456,
            "lon": -70.548,
            "radiusKm": 20.0,
            "minAglFt": DEFAULT_PUBLIC_ALTITUDE_MIN_FT,
        })
    );

    let flights = body["flights"].as_array().unwrap();
    let ids: HashSet<&str> = flights
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(ids.contains(fid_pub_named.as_str()));
    assert!(ids.contains(fid_pub_anon.as_str()));
    assert!(ids.contains(fid_pub_off.as_str()));
    assert!(!ids.contains(fid_club_only.as_str()));
    assert!(!ids.contains(fid_private.as_str()));
    assert!(!ids.contains(fid_ended.as_str()));
    assert!(!ids.contains(fid_stale.as_str()));
    assert!(!ids.contains(fid_null_club.as_str()));
    assert!(!ids.contains(fid_susp.as_str()));
    assert!(!ids.contains(fid_pend.as_str()));

    // Verify PII & field contract
    let f_named = flights
        .iter()
        .find(|f| f["flightId"] == fid_pub_named)
        .unwrap();
    assert_eq!(f_named["tail"], json!("CC-PUBN"));
    assert_eq!(f_named["layer"], json!("public"));
    assert_eq!(f_named["trailMinutes"], json!(10));
    assert_eq!(f_named["pilotName"], json!("Visible Pilot"));
    assert_eq!(f_named["points"].as_array().unwrap().len(), 3); // 8, 3, 0 min ago

    let f_anon = flights
        .iter()
        .find(|f| f["flightId"] == fid_pub_anon)
        .unwrap();
    assert_eq!(f_anon["tail"], json!("CC-PUBA"));
    assert_eq!(f_anon["layer"], json!("public"));
    assert_eq!(f_anon["trailMinutes"], json!(2));
    assert!(
        f_anon.get("pilotName").is_none(),
        "pilotName must be omitted when show_pilot_name_public=false"
    );
    assert_eq!(f_anon["points"].as_array().unwrap().len(), 2); // 1, 0 min ago

    let f_off = flights
        .iter()
        .find(|f| f["flightId"] == fid_pub_off)
        .unwrap();
    assert_eq!(f_off["trailMinutes"], json!(0));
    assert!(f_off.get("pilotName").is_none());
    let off_pts = f_off["points"].as_array().unwrap();
    assert_eq!(off_pts.len(), 1, "Off flight must return latest point only");
    assert_eq!(off_pts[0][0], json!(now));

    for f in flights {
        assert!(f.get("pilotId").is_none());
        assert!(f.get("accountId").is_none());
        assert!(f.get("deviceId").is_none());
        assert!(f.get("email").is_none());
        assert!(f.get("clubId").is_none());
        let first_pt = f["points"].as_array().unwrap()[0].as_array().unwrap();
        assert_eq!(first_pt.len(), 4);
    }

    // Cookies & Bearer (both valid AND invalid) must be completely ignored:
    // - Same response body (even includes p_pub_named's own flight!)
    // - Never 401/403 on bogus bearer
    // - Never emits Set-Cookie on public cached response
    for (cookie, bearer) in [
        (Some(pilot_cookie.as_str()), Some(pilot_bearer.as_str())),
        (Some("fv_club_session=bogus"), Some("invalid-token")),
    ] {
        let (st2, headers2, body2) = send_public_req(
            &mut router,
            "/api/public/live-trails?lat=-33.456&lon=-70.548",
            None,
            None,
            None,
            cookie,
            bearer,
        )
        .await;
        assert_eq!(st2, StatusCode::OK);
        assert!(
            headers2.get(header::SET_COOKIE).is_none(),
            "public endpoint must never emit Set-Cookie"
        );
        assert_eq!(body2, body, "cookies/bearer must not change behaviour");
    }

    // Unknown /api/public/* paths must return JSON 404 (never SPA/HTML fallback or html_csp)
    let (st_404, h_404, b_404) = send_public_req(
        &mut router,
        "/api/public/nonexistent",
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_404, StatusCode::NOT_FOUND);
    assert_eq!(b_404, json!({ "error": "not found" }));
    assert!(h_404.get(header::CONTENT_SECURITY_POLICY).is_none());
}

// ---------------------------------------------------------------------------
// Tests 4, 5, 7: Radius clamping & geo filtering, validation 400s, ETag / 304
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn public_live_trails_radius_clamping_geo_validation_and_etag_304() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;
    // Cover both SCTB (-33.x -> N34W071) and the ~500 km north points (-28.9x -> N29W071)
    // at 600 m terrain so 1200 m MSL test flights are ~1968 ft AGL (> 300 ft default floor).
    let (_dem_root, _env_guard) = make_flat_dem(&[("N34W071", 600.0), ("N29W071", 600.0)]);

    let center_lat = -33.456;
    let center_lon = -70.548;
    let (club_a, _) = create_live_club(
        &db,
        "pub-geo",
        "own-pubgeo@test",
        Some(("SCTB", center_lat, center_lon)),
    )
    .await;

    let now = now_ms();
    let fix_at = |lat: f64, lon: f64| LiveFix {
        t: now,
        lat,
        lon,
        alt: Some(1200.0),
        alt_ref: Some("msl".to_string()),
        speed: Some(30.0),
        course: Some(0.0),
        accuracy: Some(5.0),
    };

    let specs: [(&str, &str, f64, f64, f64, f64); 6] = [
        ("CC-P004", "pg4@test", -33.4200, center_lon, 3.9, 4.1),
        ("CC-P006", "pg6@test", -33.4020, center_lon, 5.9, 6.1),
        ("CC-P019", "pg19@test", -33.2850, center_lon, 18.9, 19.2),
        ("CC-P021", "pg21@test", -33.2670, center_lon, 20.9, 21.2),
        ("CC-P499", "pg499@test", -28.9685, center_lon, 498.5, 499.5),
        ("CC-P501", "pg501@test", -28.9505, center_lon, 500.5, 501.5),
    ];
    let mut fids = std::collections::HashMap::new();
    for (tail, email, lat, lon, min_d, max_d) in specs {
        let d = haversine_km(center_lat, center_lon, lat, lon);
        assert!((min_d..=max_d).contains(&d), "{tail} dist={d}");
        let (pid, _) = seed_member(&db, &club_a, tail, email, "pilot").await;
        let (fid, _) = seed_live_flight(
            &db,
            &club_a,
            &pid,
            tail,
            FlightVisibility::Public,
            false,
            None,
            &[fix_at(lat, lon)],
        )
        .await;
        fids.insert(tail, fid);
    }

    // Diagonal bbox corner (~22.6 km from center: inside 20 km lat/lon box, outside 20 km circle)
    let (p_corner, _) = seed_member(&db, &club_a, "Corner", "pg-corner@test", "pilot").await;
    let (fid_corner, _) = seed_live_flight(
        &db,
        &club_a,
        &p_corner,
        "CC-PCNR",
        FlightVisibility::Public,
        false,
        None,
        &[fix_at(-33.312, -70.375)],
    )
    .await;

    // Missing/invalid lat or lon -> 400 {"error":"invalid location"}
    for bad_loc in [
        "/api/public/live-trails",
        "/api/public/live-trails?lat=-33.456",
        "/api/public/live-trails?lon=-70.548",
        "/api/public/live-trails?lat=91&lon=-70.548",
        "/api/public/live-trails?lat=-33.456&lon=181",
        "/api/public/live-trails?lat=NaN&lon=-70.548",
        "/api/public/live-trails?lat=abc&lon=-70.548",
    ] {
        let (st, _, body) =
            send_public_req(&mut router, bad_loc, None, None, None, None, None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "uri={bad_loc} body={body}");
        assert_eq!(body, json!({ "error": "invalid location" }));
    }

    // Invalid radius_km -> 400 {"error":"invalid radius_km"}
    for bad_rad in [
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=abc",
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=NaN",
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=inf",
    ] {
        let (st, _, body) =
            send_public_req(&mut router, bad_rad, None, None, None, None, None).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "uri={bad_rad} body={body}");
        assert_eq!(body, json!({ "error": "invalid radius_km" }));
    }

    // Default radius = 20 km
    let (st, h_def, b_def) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548",
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(b_def["publicArea"]["radiusKm"], json!(20.0));
    assert_eq!(
        b_def["publicArea"]["minAglFt"],
        json!(DEFAULT_PUBLIC_ALTITUDE_MIN_FT)
    );
    let ids_def: HashSet<&str> = b_def["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(ids_def.contains(fids["CC-P004"].as_str()));
    assert!(ids_def.contains(fids["CC-P006"].as_str()));
    assert!(ids_def.contains(fids["CC-P019"].as_str()));
    assert!(!ids_def.contains(fids["CC-P021"].as_str()));
    assert!(!ids_def.contains(fid_corner.as_str()));

    // Clamping low: 1 -> 5 km (4 km in, 6 km out)
    let (st, h_r1, b_r1) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=1",
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(b_r1["publicArea"]["radiusKm"], json!(5.0));
    let ids_r1: HashSet<&str> = b_r1["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(ids_r1.contains(fids["CC-P004"].as_str()));
    assert!(!ids_r1.contains(fids["CC-P006"].as_str()));

    // Clamping high: 9999 -> 500 km (499 km in, 501 km out)
    let (st, _, b_r9999) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=9999",
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(b_r9999["publicArea"]["radiusKm"], json!(500.0));
    let ids_r9999: HashSet<&str> = b_r9999["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(ids_r9999.contains(fids["CC-P499"].as_str()));
    assert!(!ids_r9999.contains(fids["CC-P501"].as_str()));

    // ETag / If-None-Match -> 304 Not Modified (and clamped radius=1 matches radius=5)
    let etag_def = h_def
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let etag_r1 = h_r1
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(etag_def.starts_with("W/\""));

    let (st304, h304, b304) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=20",
        None,
        None,
        Some(&etag_def),
        None,
        None,
    )
    .await;
    assert_eq!(st304, StatusCode::NOT_MODIFIED);
    assert_eq!(b304, Value::Null);
    assert_eq!(h304.get(header::ETAG).unwrap().to_str().unwrap(), etag_def);
    assert_eq!(
        h304.get(header::CACHE_CONTROL).unwrap().to_str().unwrap(),
        PUBLIC_LIVE_TRAILS_CACHE_CONTROL
    );

    let (st304_r5, _, _) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=5",
        None,
        None,
        Some(&etag_r1),
        None,
        None,
    )
    .await;
    assert_eq!(st304_r5, StatusCode::NOT_MODIFIED);

    // Flipping show_pilot_name_public on an included flight invalidates the ETag
    sqlx::query("UPDATE flights SET show_pilot_name_public = TRUE WHERE id = $1")
        .bind(&fids["CC-P004"])
        .execute(&db.pool)
        .await
        .unwrap();
    let (st_after, h_after, _) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=5",
        None,
        None,
        Some(&etag_r1),
        None,
        None,
    )
    .await;
    assert_eq!(st_after, StatusCode::OK);
    assert_ne!(
        h_after.get(header::ETAG).unwrap().to_str().unwrap(),
        etag_r1
    );
}

// ---------------------------------------------------------------------------
// Test 6: Per-IP rate limit (30/min -> 429 + Retry-After, different IP 200,
// untrusted peer cannot spoof XFF, resettable throttle)
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn public_live_trails_rate_limit_per_ip_with_retry_after_and_reset() {
    let db = ClubDb::open_test().await.unwrap();
    let throttle = LoginThrottle::with_policy(OverflowPolicy::RejectNew);
    let (_tmp, mut router) = http_stack_with_throttle(db.clone(), throttle.clone()).await;

    let trusted_proxy = Some(SocketAddr::from(([127, 0, 0, 1], 0)));
    let uri = "/api/public/live-trails?lat=-33.456&lon=-70.548";

    for i in 0..PUBLIC_LIVE_TRAILS_MAX_REQUESTS {
        let (st, _, _) = send_public_req(
            &mut router,
            uri,
            trusted_proxy,
            Some("198.51.100.10"),
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "request #{i} should succeed");
    }

    // 31st request from same IP -> 429 {"error":"rate_limited"} with Retry-After: <seconds>
    let (st, headers, body) = send_public_req(
        &mut router,
        uri,
        trusted_proxy,
        Some("198.51.100.10"),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body, json!({ "error": "rate_limited" }));
    let retry_after: u64 = headers
        .get(header::RETRY_AFTER)
        .expect("Retry-After header required on 429")
        .to_str()
        .unwrap()
        .parse()
        .expect("Retry-After must be integer seconds");
    assert!(
        (1..=60).contains(&retry_after),
        "Retry-After should be within 1..=60, got {retry_after}"
    );

    // Different IP still succeeds (200)
    let (st_other, _, _) = send_public_req(
        &mut router,
        uri,
        trusted_proxy,
        Some("198.51.100.11"),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_other, StatusCode::OK);

    // Untrusted peer cannot bypass rate limit by rotating spoofed X-Forwarded-For
    let untrusted_peer = Some(SocketAddr::from(([203, 0, 113, 50], 0)));
    for i in 0..PUBLIC_LIVE_TRAILS_MAX_REQUESTS {
        let spoofed = format!("192.0.2.{}", (i % 200) + 1);
        let (st_u, _, _) = send_public_req(
            &mut router,
            uri,
            untrusted_peer,
            Some(&spoofed),
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st_u, StatusCode::OK);
    }
    let (st_u_over, _, body_u_over) = send_public_req(
        &mut router,
        uri,
        untrusted_peer,
        Some("192.0.2.250"),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_u_over, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body_u_over, json!({ "error": "rate_limited" }));

    // Resetting the injected throttle restores allowance immediately
    throttle.reset();
    let (st_after_reset, _, _) = send_public_req(
        &mut router,
        uri,
        trusted_proxy,
        Some("198.51.100.10"),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_after_reset, StatusCode::OK);

    // L-79: Hard distinct-key cap rejects unknown IPs with 429 + Retry-After without growing map,
    // while already-tracked IPs continue working normally.
    let capped_throttle = LoginThrottle::with_max_keys_and_policy(2, OverflowPolicy::RejectNew);
    let (_tmp2, mut capped_router) = http_stack_with_throttle(db, capped_throttle.clone()).await;
    for ip in ["198.51.100.1", "198.51.100.2"] {
        let (st_ok, _, _) = send_public_req(
            &mut capped_router,
            uri,
            trusted_proxy,
            Some(ip),
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st_ok, StatusCode::OK);
    }
    assert_eq!(capped_throttle.len(), 2);

    let (st_cap, h_cap, b_cap) = send_public_req(
        &mut capped_router,
        uri,
        trusted_proxy,
        Some("198.51.100.3"),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_cap, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(b_cap, json!({ "error": "rate_limited" }));
    let cap_retry: u64 = h_cap
        .get(header::RETRY_AFTER)
        .expect("Retry-After required when rejected at key cap")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&cap_retry));
    assert_eq!(
        capped_throttle.len(),
        2,
        "rejected unknown IP must not be inserted into throttle map"
    );

    let (st_existing, _, _) = send_public_req(
        &mut capped_router,
        uri,
        trusted_proxy,
        Some("198.51.100.1"),
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_existing, StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Test 8 (K-1): /api/me/live-trails auth runs BEFORE query validation
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn me_live_trails_runs_auth_before_query_validation_k1() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;

    let (club_a, _) = create_live_club(
        &db,
        "k1-club",
        "own-k1@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    let (_pilot_id, pilot_cookie) =
        seed_member(&db, &club_a, "K1 Pilot", "p-k1@test", "pilot").await;

    // Signed-out caller ALWAYS gets 401 even with missing/invalid coords or invalid layers/min_agl_ft
    for uri in [
        "/api/me/live-trails?layers=public",
        "/api/me/live-trails?layers=club,public",
        "/api/me/live-trails?layers=public&lat=999&lon=abc",
        "/api/me/live-trails?layers=public&lat=0&lon=0&radius_km=abc",
        "/api/me/live-trails?layers=public&lat=0&lon=0&min_agl_ft=abc",
        "/api/me/live-trails?layers=bogus",
    ] {
        let (st, _, body) =
            send_req(&mut router, "GET", uri, None, None, None, None, None, None).await;
        assert_eq!(
            st,
            StatusCode::UNAUTHORIZED,
            "signed-out {uri} must be 401, got {st} {body}"
        );
        assert_eq!(body, json!({ "error": "unauthorized" }));
    }

    // Signed-in caller still gets 400 for missing coords / invalid params
    let (st_loc, _, body_loc) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=public",
        Some(&pilot_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_loc, StatusCode::BAD_REQUEST);
    assert_eq!(body_loc, json!({ "error": "invalid location" }));

    let (st_layers, _, body_layers) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=bogus",
        Some(&pilot_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_layers, StatusCode::BAD_REQUEST);
    assert_eq!(body_layers, json!({ "error": "invalid layers" }));
}

// ---------------------------------------------------------------------------
// Test 9 (L-77): Hard SQL candidate LIMIT enforced on both public-layer queries
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn public_layer_sql_queries_enforce_hard_candidate_limit_l77() {
    assert_eq!(PUBLIC_LIVE_TRAILS_SQL_CANDIDATE_LIMIT, 500);

    let db = ClubDb::open_test().await.unwrap();
    // 600 m flat DEM so 1200 m MSL test flights (~1968 ft AGL) pass the anonymous 300 ft floor.
    let (_dem_root, _env_guard) = make_flat_dem(&[("N34W071", 600.0)]);
    let (club_a, _) = create_live_club(
        &db,
        "l77-club",
        "own-l77@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;

    let now = now_ms();
    let mut fids = Vec::new();
    for i in 0..3 {
        let tail = format!("CC-L77{i}");
        let (pid, _) = seed_member(
            &db,
            &club_a,
            &format!("L77 Pilot {i}"),
            &format!("l77-{i}@test"),
            "pilot",
        )
        .await;
        // Strictly increasing last_fix_at: index 2 is newest, index 0 is oldest
        let t = now - (2 - i) as i64 * 10_000;
        let (fid, _) = seed_live_flight(
            &db,
            &club_a,
            &pid,
            &tail,
            FlightVisibility::Public,
            false,
            None,
            &[LiveFix {
                t,
                lat: -33.456,
                lon: -70.548,
                alt: Some(1200.0),
                alt_ref: Some("msl".to_string()),
                speed: Some(30.0),
                course: Some(0.0),
                accuracy: Some(5.0),
            }],
        )
        .await;
        fids.push(fid);
    }

    let area = PublicArea {
        lat: -33.456,
        lon: -70.548,
        radius_km: 20.0,
        min_agl_ft: None,
    };
    let live_cutoff_ms = now.saturating_sub(10 * 60 * 1000);
    let empty_caller = LiveTrailsCaller {
        account_id: None,
        own_pilot_ids: Vec::new(),
        active_club_ids: Vec::new(),
    };

    // With injected SQL LIMIT 2, BOTH queries must return only the 2 most recent flights (fids[2], fids[1])
    let auth_limited = db
        .query_public_layer_live_flights_with_limit(&empty_caller, &[], live_cutoff_ms, area, 2)
        .await;
    assert_eq!(auth_limited.len(), 2);
    assert_eq!(auth_limited[0].flight_id, fids[2]);
    assert_eq!(auth_limited[1].flight_id, fids[1]);

    let anon_limited = db
        .query_anonymous_public_live_flights_with_limit(live_cutoff_ms, area, 2)
        .await;
    assert_eq!(anon_limited.len(), 2);
    assert_eq!(anon_limited[0].flight_id, fids[2]);
    assert_eq!(anon_limited[1].flight_id, fids[1]);
}

// ---------------------------------------------------------------------------
// Test 10 (L-76): Optional min_agl_ft altitude floor on both endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn live_trails_min_agl_ft_filters_public_layer_with_dem_fallback_and_etag_l76() {
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
        "l76-a",
        "own-l76a@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;
    let (club_b, _) = create_live_club(
        &db,
        "l76-b",
        "own-l76b@test",
        Some(("SCEL", -33.393, -70.786)),
    )
    .await;

    let (_viewer_a, viewer_cookie) =
        seed_member(&db, &club_a, "L76 Viewer", "l76-v@test", "pilot").await;
    let (p_club_low, _) = seed_member(&db, &club_a, "Club Low", "l76-cl@test", "pilot").await;
    let (p_pub_low, _) = seed_member(&db, &club_b, "Pub Low", "l76-pl@test", "pilot").await;
    let (p_pub_high, _) = seed_member(&db, &club_b, "Pub High", "l76-ph@test", "pilot").await;
    let (p_pub_noalt, _) = seed_member(&db, &club_b, "Pub NoAlt", "l76-pn@test", "pilot").await;

    let now = now_ms();
    // Own-club flight at 650 m MSL over 600 m DEM (~164 ft AGL): must stay visible on /api/me/live-trails club layer!
    let (fid_club_low, _) = seed_live_flight(
        &db,
        &club_a,
        &p_club_low,
        "CC-CLW1",
        FlightVisibility::Club,
        false,
        None,
        &make_fixes_with_alt(now, -33.455, -70.548, &[(0, Some(650.0), Some("msl"))]),
    )
    .await;

    // Public flight at 700 m MSL (~2297 ft MSL) over 600 m DEM = 100 m AGL (~328 ft < 500 ft AGL)
    let (fid_pub_low, _) = seed_live_flight(
        &db,
        &club_b,
        &p_pub_low,
        "CC-PLW1",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(now, -33.450, -70.548, &[(0, Some(700.0), Some("msl"))]),
    )
    .await;

    // Public flight at 900 m MSL over 600 m DEM = 300 m AGL (~984 ft >= 500 ft AGL)
    let (fid_pub_high, _) = seed_live_flight(
        &db,
        &club_b,
        &p_pub_high,
        "CC-PHI1",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(now, -33.445, -70.548, &[(0, Some(900.0), Some("msl"))]),
    )
    .await;

    // Public flight with missing altitude -> shown when min_agl_ft absent, hidden when min_agl_ft present
    let (fid_pub_noalt, _) = seed_live_flight(
        &db,
        &club_b,
        &p_pub_noalt,
        "CC-PNA1",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(now, -33.440, -70.548, &[(0, None, None)]),
    )
    .await;

    // 1) Invalid min_agl_ft -> 400 {"error":"invalid min_agl_ft"} on BOTH endpoints
    for bad_uri in [
        "/api/public/live-trails?lat=-33.456&lon=-70.548&min_agl_ft=abc",
        "/api/public/live-trails?lat=-33.456&lon=-70.548&min_agl_ft=NaN",
        "/api/public/live-trails?lat=-33.456&lon=-70.548&min_agl_ft=-1",
        "/api/public/live-trails?lat=-33.456&lon=-70.548&min_agl_ft=-50",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&min_agl_ft=abc",
        "/api/me/live-trails?layers=club,public&lat=-33.456&lon=-70.548&min_agl_ft=inf",
    ] {
        let cookie = if bad_uri.starts_with("/api/me/") {
            Some(viewer_cookie.as_str())
        } else {
            None
        };
        let (st, _, body) = send_req(
            &mut router,
            "GET",
            bad_uri,
            cookie,
            None,
            None,
            None,
            None,
            None,
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "uri={bad_uri} body={body}");
        assert_eq!(body, json!({ "error": "invalid min_agl_ft" }));
    }

    // 2a) Absent min_agl_ft on /api/public/live-trails -> default 300 ft AGL floor, fail-closed:
    //     fid_pub_low (~328 ft) and fid_pub_high (~984 ft) pass, fid_pub_noalt excluded, minAglFt = 300.0
    let (st_default_floor, h_default_floor, b_default_floor) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548",
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_default_floor, StatusCode::OK);
    assert_eq!(
        b_default_floor["publicArea"]["minAglFt"],
        json!(DEFAULT_PUBLIC_ALTITUDE_MIN_FT)
    );
    let ids_default_floor: HashSet<&str> = b_default_floor["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(ids_default_floor.contains(fid_pub_low.as_str()));
    assert!(ids_default_floor.contains(fid_pub_high.as_str()));
    assert!(
        !ids_default_floor.contains(fid_pub_noalt.as_str()),
        "anonymous endpoint must fail closed on alt NULL"
    );

    // 2b) Absent min_agl_ft on /api/me/live-trails -> unchanged (no floor, minAglFt omitted,
    //     all 3 public flights + club flight returned)
    let (st_me_no_floor, _, b_me_no_floor) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=club,public&lat=-33.456&lon=-70.548",
        Some(&viewer_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_me_no_floor, StatusCode::OK, "{b_me_no_floor}");
    assert!(b_me_no_floor["publicArea"].get("minAglFt").is_none());
    let ids_me_no_floor: HashSet<&str> = b_me_no_floor["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(ids_me_no_floor.contains(fid_club_low.as_str()));
    assert!(ids_me_no_floor.contains(fid_pub_low.as_str()));
    assert!(ids_me_no_floor.contains(fid_pub_high.as_str()));
    assert!(ids_me_no_floor.contains(fid_pub_noalt.as_str()));

    // 3) With DEM + min_agl_ft=500 on /api/public/live-trails -> only fid_pub_high shown, publicArea.minAglFt = 500.0
    let (st_500, h_500, b_500) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548&min_agl_ft=500",
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_500, StatusCode::OK);
    assert_eq!(b_500["publicArea"]["minAglFt"], json!(500.0));
    let ids_500: HashSet<&str> = b_500["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(ids_500.contains(fid_pub_high.as_str()));
    assert!(!ids_500.contains(fid_pub_low.as_str()));
    assert!(!ids_500.contains(fid_pub_noalt.as_str()));

    // ETag differs between default (300) and 500 (and between 500 and 600 even with same flights!)
    let etag_default_floor = h_default_floor.get(header::ETAG).unwrap().to_str().unwrap();
    let etag_500 = h_500.get(header::ETAG).unwrap().to_str().unwrap();
    assert_ne!(etag_default_floor, etag_500);
    let (st_600, h_600, _) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548&min_agl_ft=600",
        None,
        None,
        Some(etag_500),
        None,
        None,
    )
    .await;
    assert_eq!(st_600, StatusCode::OK);
    assert_ne!(h_600.get(header::ETAG).unwrap().to_str().unwrap(), etag_500);

    // 4) With DEM + min_agl_ft=500 on /api/me/live-trails?layers=club,public:
    //    - public layer drops fid_pub_low and fid_pub_noalt, keeps fid_pub_high
    //    - club layer is UNAFFECTED and still returns low-altitude fid_club_low!
    let (st_me, _, b_me) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=club,public&lat=-33.456&lon=-70.548&min_agl_ft=500",
        Some(&viewer_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_me, StatusCode::OK, "{b_me}");
    assert_eq!(b_me["publicArea"]["minAglFt"], json!(500.0));
    let ids_me: HashSet<&str> = b_me["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(
        ids_me.contains(fid_club_low.as_str()),
        "club layer must be exempt from min_agl_ft"
    );
    assert!(ids_me.contains(fid_pub_high.as_str()));
    assert!(!ids_me.contains(fid_pub_low.as_str()));
    assert!(!ids_me.contains(fid_pub_noalt.as_str()));

    // 5) Without DEM coverage:
    //    - /api/public/live-trails FAILS CLOSED (excludes all flights even at high MSL)
    //    - /api/me/live-trails keeps NoDemFallbackMsl (unchanged L-76 behavior)
    env_guard.update(empty_root.path());
    let (st_pub_nodem, _, b_pub_nodem) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548&min_agl_ft=500",
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_pub_nodem, StatusCode::OK);
    assert_eq!(b_pub_nodem["publicArea"]["minAglFt"], json!(500.0));
    assert!(
        b_pub_nodem["flights"].as_array().unwrap().is_empty(),
        "anonymous /api/public/live-trails must fail closed when DEM coverage is missing"
    );

    let (st_me_nodem, _, b_me_nodem) = send_req(
        &mut router,
        "GET",
        "/api/me/live-trails?layers=public&lat=-33.456&lon=-70.548&min_agl_ft=500",
        Some(&viewer_cookie),
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_me_nodem, StatusCode::OK, "{b_me_nodem}");
    let ids_me_nodem: HashSet<&str> = b_me_nodem["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(
        ids_me_nodem.contains(fid_pub_low.as_str()),
        "authenticated /api/me/live-trails keeps NoDemFallbackMsl"
    );
    assert!(ids_me_nodem.contains(fid_pub_high.as_str()));
    assert!(!ids_me_nodem.contains(fid_pub_noalt.as_str()));
}

// ---------------------------------------------------------------------------
// Test 11 (M-27): Anonymous /api/public/live-trails 300 ft AGL default floor & fail-closed
// ---------------------------------------------------------------------------

#[tokio::test]
#[serial]
async fn public_live_trails_enforces_300ft_default_agl_floor_fail_closed_m27() {
    let db = ClubDb::open_test().await.unwrap();
    let (_tmp, mut router) = http_stack(db.clone()).await;

    // 600 m flat DEM covering ONLY (-34..-33, -71..-70) -> N34W071.asc.
    // Positions south of -34.0 (tile N35W071) intentionally have NO DEM coverage.
    let (_dem_root, _env_guard) = make_flat_dem(&[("N34W071", 600.0)]);

    let (club_a, _) = create_live_club(
        &db,
        "m27-club",
        "own-m27@test",
        Some(("SCTB", -33.456, -70.548)),
    )
    .await;

    let (p_ground, _) = seed_member(&db, &club_a, "Ground Pilot", "m27-gnd@test", "pilot").await;
    let (p_150, _) = seed_member(&db, &club_a, "Low 150ft", "m27-150@test", "pilot").await;
    let (p_500, _) = seed_member(&db, &club_a, "Air 500ft", "m27-500@test", "pilot").await;
    let (p_1500, _) = seed_member(&db, &club_a, "Air 1500ft", "m27-1500@test", "pilot").await;
    let (p_21000, _) = seed_member(&db, &club_a, "High 21kft", "m27-21k@test", "pilot").await;
    let (p_null, _) = seed_member(&db, &club_a, "Null Alt", "m27-null@test", "pilot").await;
    let (p_nodem, _) = seed_member(&db, &club_a, "No DEM", "m27-nodem@test", "pilot").await;
    let (p_hae, _) = seed_member(&db, &club_a, "HAE Pass", "m27-hae@test", "pilot").await;
    let (p_msl_lo, _) = seed_member(&db, &club_a, "MSL Fail", "m27-msllo@test", "pilot").await;

    let now = now_ms();

    // 1) Ground flight at airfield: earlier fix airborne (900 m MSL), latest fix at terrain (605 m MSL ≈ 16 ft AGL) -> EXCLUDED
    let (fid_ground, _) = seed_live_flight(
        &db,
        &club_a,
        &p_ground,
        "CC-M27G",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(
            now,
            -33.456,
            -70.548,
            &[(3, Some(900.0), Some("msl")), (0, Some(605.0), Some("msl"))],
        ),
    )
    .await;

    // 2) Low flight at ~151 ft AGL (646 m MSL over 600 m terrain = 46 m ≈ 150.9 ft) -> EXCLUDED under default and min_agl_ft=0
    let (fid_150ft, _) = seed_live_flight(
        &db,
        &club_a,
        &p_150,
        "CC-M27L",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(now, -33.450, -70.548, &[(0, Some(646.0), Some("msl"))]),
    )
    .await;

    // 3) Airborne flight at 500 ft AGL (752.4 m MSL over 600 m terrain; earlier fix on ground) -> INCLUDED at 300, EXCLUDED at 1000
    let (fid_500ft, _) = seed_live_flight(
        &db,
        &club_a,
        &p_500,
        "CC-M27A",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(
            now,
            -33.445,
            -70.548,
            &[(3, Some(600.0), Some("msl")), (0, Some(752.4), Some("msl"))],
        ),
    )
    .await;

    // 4) Higher flight at ~1509 ft AGL (1060 m MSL over 600 m terrain) -> INCLUDED at 300 & 1000, EXCLUDED at 20000
    let (fid_1500ft, _) = seed_live_flight(
        &db,
        &club_a,
        &p_1500,
        "CC-M27B",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(now, -33.440, -70.548, &[(0, Some(1060.0), Some("msl"))]),
    )
    .await;

    // 5) Very high flight at ~21161 ft AGL (7050 m MSL over 600 m terrain) -> INCLUDED even when min_agl_ft=50000 (clamped to 20000)
    let (fid_21000ft, _) = seed_live_flight(
        &db,
        &club_a,
        &p_21000,
        "CC-M27H",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(now, -33.435, -70.548, &[(0, Some(7050.0), Some("msl"))]),
    )
    .await;

    // 6) Latest fix with alt NULL (even with high earlier fix) -> EXCLUDED (fail closed)
    let (fid_null_alt, _) = seed_live_flight(
        &db,
        &club_a,
        &p_null,
        "CC-M27N",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(
            now,
            -33.430,
            -70.548,
            &[(3, Some(1500.0), Some("msl")), (0, None, None)],
        ),
    )
    .await;

    // 7) Fix outside DEM coverage (-34.05, -70.548 is ~66 km south, inside radius_km=100, tile N35W071 missing)
    //    at high MSL (3000 m ≈ 9842 ft MSL) -> EXCLUDED (fail closed, no NoDemFallbackMsl)
    assert!(haversine_km(-33.456, -70.548, -34.05, -70.548) < 100.0);
    let (fid_no_dem, _) = seed_live_flight(
        &db,
        &club_a,
        &p_nodem,
        "CC-M27D",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(now, -34.05, -70.548, &[(0, Some(3000.0), Some("msl"))]),
    )
    .await;

    // 8) alt_ref semantics at 680 m over 600 m terrain (undulation ≈ -30.07 m at lat -33.45):
    //    - alt_ref="hae" -> ~710.07 m MSL -> ~361 ft AGL (>= 300 ft) -> INCLUDED
    //    - alt_ref="msl" -> 680.00 m MSL -> ~262 ft AGL (< 300 ft) -> EXCLUDED
    let (fid_hae_pass, _) = seed_live_flight(
        &db,
        &club_a,
        &p_hae,
        "CC-M27E",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(now, -33.452, -70.548, &[(0, Some(680.0), Some("hae"))]),
    )
    .await;
    let (fid_msl_fail, _) = seed_live_flight(
        &db,
        &club_a,
        &p_msl_lo,
        "CC-M27M",
        FlightVisibility::Public,
        false,
        None,
        &make_fixes_with_alt(now, -33.453, -70.548, &[(0, Some(680.0), Some("msl"))]),
    )
    .await;

    // Case A: Default (no min_agl_ft) -> effective floor 300 ft AGL, fail closed
    let (st_def, h_def, b_def) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=100",
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_def, StatusCode::OK, "{b_def}");
    assert_eq!(b_def["publicArea"]["minAglFt"], json!(300.0));
    let ids_def: HashSet<&str> = b_def["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(
        !ids_def.contains(fid_ground.as_str()),
        "ground flight must be excluded"
    );
    assert!(
        !ids_def.contains(fid_150ft.as_str()),
        "150 ft AGL flight must be excluded"
    );
    assert!(
        !ids_def.contains(fid_null_alt.as_str()),
        "alt NULL flight must be excluded"
    );
    assert!(
        !ids_def.contains(fid_no_dem.as_str()),
        "no-DEM flight must be excluded even at high MSL"
    );
    assert!(
        !ids_def.contains(fid_msl_fail.as_str()),
        "680 m MSL (~262 ft AGL) must be excluded"
    );
    assert!(
        ids_def.contains(fid_500ft.as_str()),
        "500 ft AGL flight must be included"
    );
    assert!(
        ids_def.contains(fid_1500ft.as_str()),
        "1500 ft AGL flight must be included"
    );
    assert!(
        ids_def.contains(fid_21000ft.as_str()),
        "21000 ft AGL flight must be included"
    );
    assert!(
        ids_def.contains(fid_hae_pass.as_str()),
        "680 m HAE (~361 ft AGL) must be included"
    );

    // Case B: min_agl_ft=0 -> clamped up to 300 (cannot lower floor); identical ETag -> 304
    let (st_zero, h_zero, b_zero) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=100&min_agl_ft=0",
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_zero, StatusCode::OK, "{b_zero}");
    assert_eq!(b_zero["publicArea"]["minAglFt"], json!(300.0));
    let ids_zero: HashSet<&str> = b_zero["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(
        !ids_zero.contains(fid_150ft.as_str()),
        "min_agl_ft=0 must still exclude 150 ft AGL"
    );
    assert!(!ids_zero.contains(fid_ground.as_str()));
    assert!(!ids_zero.contains(fid_null_alt.as_str()));
    assert!(!ids_zero.contains(fid_no_dem.as_str()));
    assert!(ids_zero.contains(fid_500ft.as_str()));
    assert_eq!(
        h_zero.get(header::ETAG).unwrap(),
        h_def.get(header::ETAG).unwrap(),
        "min_agl_ft=0 and omitted min_agl_ft share canonical floor 300 and identical ETag"
    );

    // Case C: min_agl_ft=1000 raises the floor -> 500 ft excluded, 1500 ft & 21000 ft included
    let (st_1000, _, b_1000) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=100&min_agl_ft=1000",
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_1000, StatusCode::OK, "{b_1000}");
    assert_eq!(b_1000["publicArea"]["minAglFt"], json!(1000.0));
    let ids_1000: HashSet<&str> = b_1000["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(
        !ids_1000.contains(fid_500ft.as_str()),
        "500 ft AGL must be excluded at min_agl_ft=1000"
    );
    assert!(!ids_1000.contains(fid_hae_pass.as_str()));
    assert!(ids_1000.contains(fid_1500ft.as_str()));
    assert!(ids_1000.contains(fid_21000ft.as_str()));

    // Case D: min_agl_ft=50000 -> clamped to 20000 (1500 ft excluded, 21000 ft included)
    let (st_50k, _, b_50k) = send_public_req(
        &mut router,
        "/api/public/live-trails?lat=-33.456&lon=-70.548&radius_km=100&min_agl_ft=50000",
        None,
        None,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(st_50k, StatusCode::OK, "{b_50k}");
    assert_eq!(b_50k["publicArea"]["minAglFt"], json!(20000.0));
    let ids_50k: HashSet<&str> = b_50k["flights"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["flightId"].as_str())
        .collect();
    assert!(!ids_50k.contains(fid_1500ft.as_str()));
    assert!(ids_50k.contains(fid_21000ft.as_str()));
}
