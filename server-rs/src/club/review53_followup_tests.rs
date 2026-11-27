//! Regression & security tests for Jordan's PR #53 review follow-up items (1..=9):
//! - Item 1: `DELETE /api/account` auth before CSRF and Content-Type
//! - Item 2 (M-32): omit-id ingest & `/api/osmand` drop previous-flight/pre-start leftovers
//! - Item 3 (M-34): shared `lock_account_for_membership_change` advisory lock prevents ghost memberships
//! - Item 4 (L-91): per-device `409 flight_mismatch` throttle (`429` + `Retry-After`)
//! - Item 5 (L-92): `nearby_public_positions_for_club` sets `nonClub` true only for pilot tails
//!   and `otherClub` true only for other-club aircraft (own-club rows set neither)
//! - Item 6 (L-95): `delete_account` active/pending gate + `DELETE /api/account/clubs/{slug}`
//! - Item 7 (L-96): `/api/club/map/public` SQL candidate cap `LIMIT 500`
//! - Item 8 (L-97): uniform `404 {"error":"not found"}` on flight write/share management paths
//! - Item 9 (L-87): `WindowThrottle` / `LoginThrottle` pinning under `EvictSoonestExpiring`

use crate::{
    auth::Auth,
    build_router,
    club::{
        auth::{
            hash_password, new_id, ACCOUNT_COOKIE as ACCOUNT_SESSION_COOKIE,
            COOKIE as SESSION_COOKIE,
        },
        db::{now_ms, ClubDb, WindowThrottle},
        db_account::{AccountJoinError, ACCOUNT_SESSION_AUTH_MAGIC, ACCOUNT_SESSION_AUTH_PASSWORD},
        map::{TAIL_SOURCE_CLUB, TAIL_SOURCE_PILOT},
        pilot_live_trails::PUBLIC_LIVE_TRAILS_SQL_CANDIDATE_LIMIT,
        visibility::FlightVisibility,
    },
    device_auth::DeviceAuth,
    live::{LiveEngine, LiveFix, MAX_INGEST_FLIGHT_MISMATCH_PER_MINUTE},
    store::Store,
    AppState, OverflowPolicy,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use serial_test::serial;
use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc,
};
use tower::Service;

struct Fixture {
    _dir: tempfile::TempDir,
    router: Router,
    db: ClubDb,
    live: Arc<LiveEngine>,
    clock_ms: Arc<AtomicI64>,
}

async fn setup_fx() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let dist = dir.path().join("dist");
    std::fs::create_dir_all(&dist).unwrap();
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
        auth: Auth::with_password("legacy-pw"),
        device_auth: DeviceAuth::empty(),
        live: live.clone(),
        store: Store::new(dir.path()),
        club: Some(db.clone()),
        video_queue: tokio::sync::mpsc::channel(1).0,
        login_throttle: crate::LoginThrottle::default(),
        device_register_throttle: crate::LoginThrottle::default(),
        share_lookup_throttle: crate::LoginThrottle::default(),
        account_password_throttle: crate::LoginThrottle::default(),
        account_password_account_throttle: crate::LoginThrottle::default(),
        public_live_trails_throttle: crate::LoginThrottle::with_policy(OverflowPolicy::RejectNew),
        mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
        webauthn: None,
        sse_limiter: Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails: Default::default(),
    };
    let router = build_router(state);
    Fixture {
        _dir: dir,
        router,
        db,
        live,
        clock_ms,
    }
}

/// Write a tiny deterministic flat-elevation DEM tile (SRTM-style 3x3 HGT) covering S34W071
/// so fail-closed 300 ft AGL evaluation passes deterministically for test coordinates near SCTB.
struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, val: &std::path::Path) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, val);
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        if let Some(ref p) = self.prev {
            std::env::set_var(self.key, p);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn write_flat_dem_tile(data_dir: &std::path::Path, tile_name: &str, elevation_m: i16) {
    let dem_dir = data_dir.join("dem");
    std::fs::create_dir_all(&dem_dir).unwrap();
    let e = elevation_m as f64;
    std::fs::write(
        dem_dir.join(format!("{tile_name}.asc")),
        format!("# {e} m flat terrain\n{e} {e} {e}\n{e} {e} {e}\n{e} {e} {e}\n"),
    )
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn send_raw(
    router: &mut Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    bearer: Option<&str>,
    extra_headers: &[(&str, &str)],
    content_type: Option<&str>,
    raw_body: Vec<u8>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut b = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, c);
    }
    if let Some(t) = bearer {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    if let Some(ct) = content_type {
        b = b.header(header::CONTENT_TYPE, ct);
    }
    for (k, v) in extra_headers {
        b = b.header(*k, *v);
    }
    let req = b.body(Body::from(raw_body)).unwrap();
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
    router: &mut Router,
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
    let parsed = serde_json::from_slice(&raw)
        .unwrap_or_else(|_| json!({ "_raw": String::from_utf8_lossy(&raw) }));
    (st, hdrs, parsed)
}

async fn create_live_club(db: &ClubDb, slug_prefix: &str, lat: f64, lon: f64) -> (String, String) {
    let club_id = new_id();
    let slug = format!("{slug_prefix}-{}", &new_id()[..6]);
    let now = now_ms();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, contact_email, email_verified_at,
                            aerodrome_icao, latitude, longitude, checkin_window_hours,
                            show_nearby_public, nearby_public_radius_km, show_public_tracks,
                            public_radius_km, public_flights_cap, public_altitude_mode, public_altitude_min_ft)
         VALUES ($1, $2, $3, $4, 'live', $5, $4, 'SCTB', $6, $7, 12, TRUE, 50, TRUE, 100.0, 50, 'agl', 300.0)",
    )
    .bind(&club_id)
    .bind(format!("Club {slug}"))
    .bind(&slug)
    .bind(now)
    .bind(format!("owner-{slug}@test.example"))
    .bind(lat)
    .bind(lon)
    .execute(&db.pool)
    .await
    .unwrap();
    (club_id, slug)
}

async fn create_account_session(
    db: &ClubDb,
    email: &str,
    verified: bool,
    password: Option<&str>,
    auth_method: &str,
    session_created_at: i64,
) -> (String, String, String) {
    let now = now_ms();
    let acct_id = db.ensure_account_for_email(email, None, now).await.unwrap();
    if verified {
        sqlx::query("UPDATE accounts SET email_verified_at = $1 WHERE id = $2")
            .bind(now)
            .bind(&acct_id)
            .execute(&db.pool)
            .await
            .unwrap();
    }
    if let Some(pw) = password {
        let hash = hash_password(pw).unwrap();
        sqlx::query("UPDATE accounts SET password_hash = $1 WHERE id = $2")
            .bind(&hash)
            .bind(&acct_id)
            .execute(&db.pool)
            .await
            .unwrap();
    }
    let raw_sess = db
        .create_account_session_auth(&acct_id, None, session_created_at, auth_method)
        .await
        .unwrap();
    let cookie = format!("{ACCOUNT_SESSION_COOKIE}={raw_sess}");
    (acct_id, raw_sess, cookie)
}

async fn add_club_member_with_status(
    db: &ClubDb,
    account_id: &str,
    email: &str,
    name: &str,
    club_id: &str,
    role: &str,
    membership_status: &str,
) -> (String, String, String) {
    let now = now_ms();
    let pid = new_id();
    let pilot_status = if membership_status == "pending" {
        "pending"
    } else if membership_status == "active" || membership_status == "approved" {
        "approved"
    } else {
        "suspended"
    };
    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, $3, $4, $5, '', $6, $7, '', $8, $8)",
    )
    .bind(&pid)
    .bind(club_id)
    .bind(account_id)
    .bind(name)
    .bind(email)
    .bind(pilot_status)
    .bind(role)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let mid = new_id();
    sqlx::query(
        "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&mid)
    .bind(&pid)
    .bind(club_id)
    .bind(role)
    .bind(membership_status)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let active_mem = if membership_status == "active" || membership_status == "approved" {
        Some(mid.as_str())
    } else {
        None
    };
    let raw_sess = db
        .create_account_session_auth(account_id, active_mem, now, ACCOUNT_SESSION_AUTH_PASSWORD)
        .await
        .unwrap();
    let cookie = format!("{ACCOUNT_SESSION_COOKIE}={raw_sess}");
    (pid, mid, cookie)
}

// ===========================================================================
// Item 1: DELETE /api/account auth before CSRF and Content-Type
// ===========================================================================

#[tokio::test]
#[serial]
async fn item1_delete_account_checks_auth_before_csrf_and_content_type() {
    let mut fx = setup_fx().await;
    let now = now_ms();
    let (acct_id, _, valid_cookie) = create_account_session(
        &fx.db,
        &format!("item1-{}@test.example", &new_id()[..6]),
        true,
        Some("StrongPass123!"),
        ACCOUNT_SESSION_AUTH_PASSWORD,
        now,
    )
    .await;

    // 1. No cookie + no body + no Content-Type -> 401 unauthorized (NOT 415!)
    let (st, _, raw) = send_raw(
        &mut fx.router,
        "DELETE",
        "/api/account",
        None,
        None,
        &[],
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(
        serde_json::from_slice::<Value>(&raw).unwrap(),
        json!({ "error": "unauthorized" })
    );

    // 2. No cookie + text/plain -> 401 unauthorized (NOT 415!)
    let (st, _, raw) = send_raw(
        &mut fx.router,
        "DELETE",
        "/api/account",
        None,
        None,
        &[],
        Some("text/plain"),
        b"hello".to_vec(),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(
        serde_json::from_slice::<Value>(&raw).unwrap(),
        json!({ "error": "unauthorized" })
    );

    // 3. No cookie + cross-site Origin + text/plain -> 401 unauthorized (NOT 403/415!)
    let (st, _, raw) = send_raw(
        &mut fx.router,
        "DELETE",
        "/api/account",
        None,
        None,
        &[
            ("host", "flight.example"),
            ("origin", "https://evil.example"),
        ],
        Some("text/plain"),
        Vec::new(),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(
        serde_json::from_slice::<Value>(&raw).unwrap(),
        json!({ "error": "unauthorized" })
    );

    // 4. Bogus cookie + no body -> 401 unauthorized
    let (st, _, raw) = send_raw(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&format!("{ACCOUNT_SESSION_COOKIE}=nonexistent-token")),
        None,
        &[],
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    assert_eq!(
        serde_json::from_slice::<Value>(&raw).unwrap(),
        json!({ "error": "unauthorized" })
    );

    // 5. Valid session + wrong Content-Type (text/plain) -> 415 unsupported media type
    let (st, _, raw) = send_raw(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&valid_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        Some("text/plain"),
        b"{}".to_vec(),
    )
    .await;
    assert_eq!(st, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(
        serde_json::from_slice::<Value>(&raw).unwrap(),
        json!({ "error": "unsupported media type" })
    );

    // 6. Valid session + cross-site Origin -> 403 csrf (even without JSON Content-Type)
    let (st, _, raw) = send_raw(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&valid_cookie),
        None,
        &[
            ("host", "flight.example"),
            ("origin", "https://evil.example"),
        ],
        Some("application/json"),
        b"{}".to_vec(),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(
        serde_json::from_slice::<Value>(&raw).unwrap(),
        json!({ "error": "csrf" })
    );

    // Account still intact after all rejected calls.
    assert!(fx.db.account_by_id(&acct_id).await.is_some());
}

// ===========================================================================
// Item 2 (M-32): omit-id drops leftovers from recently ended flight
// ===========================================================================

#[tokio::test]
#[serial]
async fn item2_m32_omit_id_drops_previous_flight_and_pre_start_leftovers_ingest_and_osmand() {
    let mut fx = setup_fx().await;
    let _dem_guard = EnvGuard::set("FLIGHT_DATA", fx._dir.path());
    write_flat_dem_tile(fx._dir.path(), "N34W071", 500);

    // Align T to whole seconds so /api/osmand second-precision timestamps are exact.
    let now_sec_ms = (now_ms() / 1000) * 1000;
    let t_end = now_sec_ms - 60_000;
    let t_b_start = t_end + 30_000; // B starts 30 s after A ends (well inside 120 s skew!)
    fx.clock_ms.store(now_sec_ms, Ordering::SeqCst);

    // --- Part A: same device, private flight A ends at T, public flight B starts at T+30s ---
    let email = format!("m32-same-{}@test.example", &new_id()[..6]);
    let (acct_id, _, _) = create_account_session(
        &fx.db,
        &email,
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        t_end - 300_000,
    )
    .await;
    let pid = fx
        .db
        .ensure_personal_pilot(&acct_id, t_end - 300_000)
        .await
        .unwrap();
    let (dev_id, dev_tok) = fx.db.create_device(&pid, "m32-phone").await.unwrap();

    let flight_a = fx
        .db
        .start_personal_flight_atomic(
            &pid,
            &acct_id,
            &dev_id,
            "CC-M32A",
            FlightVisibility::Private,
            false,
            None,
            t_end - 180_000,
        )
        .await
        .unwrap();
    fx.db.end_flight(&pid, &flight_a, t_end).await.unwrap();

    let flight_b = fx
        .db
        .start_personal_flight_atomic(
            &pid,
            &acct_id,
            &dev_id,
            "CC-M32B",
            FlightVisibility::Public,
            false,
            None,
            t_b_start,
        )
        .await
        .unwrap();

    // Omit-id batch with fixes at T-10s (during private A), T (end of A), T+5s (before B start, inside skew),
    // and T+40s (in-flight on B, high enough AGL for public trail).
    let (st, _, body) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&dev_tok),
        &[],
        Some(json!({
            "fixes": [
                { "t": t_end - 10_000, "lat": -33.41, "lon": -70.51, "alt": 1200.0 },
                { "t": t_end,          "lat": -33.42, "lon": -70.52, "alt": 1200.0 },
                { "t": t_end + 5_000,  "lat": -33.43, "lon": -70.53, "alt": 1200.0 },
                { "t": t_end + 40_000, "lat": -33.45, "lon": -70.55, "alt": 1200.0 }
            ]
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        body,
        json!({ "ok": true, "accepted": 1, "duplicates": 0, "droppedPreStart": 3 }),
        "T-10s, T, and T+5s must all be droppedPreStart on omit-id batch"
    );

    // Only T+40s appears on B's DB fixes and public live-trails!
    let fixes_b = fx.db.fixes_for_flight(&flight_b).await;
    assert_eq!(fixes_b.len(), 1);
    assert_eq!(fixes_b[0].t, t_end + 40_000);

    let (st_pub, _, pub_body) = send_json(
        &mut fx.router,
        "GET",
        "/api/public/live-trails?lat=-33.45&lon=-70.55&radius_km=50",
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st_pub, StatusCode::OK, "{pub_body}");
    let trail_b = pub_body["flights"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["flightId"] == flight_b)
        .expect("public flight B on public live-trails");
    let pts = trail_b["points"].as_array().unwrap();
    assert_eq!(
        pts.len(),
        1,
        "leftover fixes must never leak to public trail: {pts:?}"
    );
    assert_eq!(pts[0][0], json!(t_end + 40_000));

    // Same through /api/osmand without flight_id: T-10s and T+5s are dropped (200 OK, zero writes); T+45s attaches.
    for stale_t in [t_end - 10_000, t_end, t_end + 5_000] {
        let (st_os, _, _) = send_raw(
            &mut fx.router,
            "POST",
            &format!(
                "/api/osmand?id={dev_tok}&lat=-33.41&lon=-70.51&altitude=1200&timestamp={}",
                stale_t / 1000
            ),
            None,
            None,
            &[],
            None,
            Vec::new(),
        )
        .await;
        assert_eq!(st_os, StatusCode::OK);
    }
    assert_eq!(
        fx.db.fixes_for_flight(&flight_b).await.len(),
        1,
        "/api/osmand omit-id must drop T-10s, T, T+5s"
    );

    let (st_os_ok, _, _) = send_raw(
        &mut fx.router,
        "POST",
        &format!(
            "/api/osmand?id={dev_tok}&lat=-33.46&lon=-70.56&altitude=1200&timestamp={}",
            (t_end + 45_000) / 1000
        ),
        None,
        None,
        &[],
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(st_os_ok, StatusCode::OK);
    let ts_after_os: Vec<i64> = fx
        .db
        .fixes_for_flight(&flight_b)
        .await
        .into_iter()
        .map(|f| f.t)
        .collect();
    assert_eq!(ts_after_os, vec![t_end + 40_000, t_end + 45_000]);

    // Explicit flight_id path is UNCHANGED: T-10s and T+5s tagged with flight_b attach normally!
    let (st_exp, _, b_exp) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&dev_tok),
        &[],
        Some(json!({
            "flight_id": flight_b,
            "fixes": [
                { "t": t_end - 10_000, "lat": -33.41, "lon": -70.51, "alt": 1200.0 },
                { "t": t_end + 5_000,  "lat": -33.43, "lon": -70.53, "alt": 1200.0 }
            ]
        })),
    )
    .await;
    assert_eq!(st_exp, StatusCode::OK, "{b_exp}");
    assert_eq!(
        b_exp,
        json!({ "ok": true, "accepted": 2, "duplicates": 0, "droppedPreStart": 0 }),
        "explicit flight_id path must remain unchanged"
    );

    // --- Part B: cross-mode (club flight A ends at T, personal public flight B starts at T+30s on same account) ---
    let (club_id, _) = create_live_club(&fx.db, "m32-club", -33.456, -70.548).await;
    fx.db.upsert_type(&club_id, "C172").await.unwrap();
    fx.db
        .upsert_aircraft(&club_id, "CC-M32C", "C172", true)
        .await
        .unwrap();
    let xm_email = format!("m32-xmode-{}@test.example", &new_id()[..6]);
    let (xm_acct, _, _) = create_account_session(
        &fx.db,
        &xm_email,
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        t_end - 300_000,
    )
    .await;
    let (club_pid, _, _) = add_club_member_with_status(
        &fx.db,
        &xm_acct,
        &xm_email,
        "Cross Mode",
        &club_id,
        "pilot",
        "active",
    )
    .await;
    fx.db
        .add_checkout(&club_pid, "C172", None, None)
        .await
        .unwrap();
    let (club_dev, _) = fx.db.create_device(&club_pid, "club-dev").await.unwrap();
    fx.db
        .issue_open_checkin(&club_pid, &club_dev, &club_id, t_end - 200_000)
        .await
        .unwrap();
    let club_flight_a = fx
        .db
        .start_flight(
            &club_pid,
            &club_dev,
            "CC-M32C",
            FlightVisibility::Club,
            false,
            t_end - 180_000,
        )
        .await
        .unwrap();
    fx.db
        .end_flight(&club_pid, &club_flight_a, t_end)
        .await
        .unwrap();

    let pers_pid = fx
        .db
        .ensure_personal_pilot(&xm_acct, t_b_start)
        .await
        .unwrap();
    let (pers_dev, pers_tok) = fx.db.create_device(&pers_pid, "pers-dev").await.unwrap();
    let pers_flight_b = fx
        .db
        .start_personal_flight_atomic(
            &pers_pid,
            &xm_acct,
            &pers_dev,
            "CC-M32P",
            FlightVisibility::Public,
            false,
            None,
            t_b_start,
        )
        .await
        .unwrap();

    let (st_xm, _, b_xm) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&pers_tok),
        &[],
        Some(json!({
            "fixes": [
                { "t": t_end - 10_000, "lat": -33.41, "lon": -70.51, "alt": 1200.0 },
                { "t": t_end + 5_000,  "lat": -33.42, "lon": -70.52, "alt": 1200.0 },
                { "t": t_end + 40_000, "lat": -33.45, "lon": -70.55, "alt": 1200.0 }
            ]
        })),
    )
    .await;
    assert_eq!(st_xm, StatusCode::OK, "{b_xm}");
    assert_eq!(
        b_xm,
        json!({ "ok": true, "accepted": 1, "duplicates": 0, "droppedPreStart": 2 })
    );
    let xm_fixes = fx.db.fixes_for_flight(&pers_flight_b).await;
    assert_eq!(xm_fixes.len(), 1);
    assert_eq!(xm_fixes[0].t, t_end + 40_000);
}

// ===========================================================================
// Item 3 (M-34): ghost membership advisory lock interleaving + static helper check
// ===========================================================================

#[tokio::test]
#[serial]
async fn item3_m34_join_and_approve_block_on_delete_advisory_lock_with_no_ghosts() {
    let fx = setup_fx().await;
    let now = now_ms();
    let (club_id, _) = create_live_club(&fx.db, "m34-club", -33.456, -70.548).await;

    // --- Part A: join_club_with_account vs delete_account_in_tx deterministic interleaving ---
    let email_j = format!("m34-join-{}@test.example", &new_id()[..6]);
    let pw = "RaceTestPass99!";
    let (acct_j, sess_j, _) = create_account_session(
        &fx.db,
        &email_j,
        true,
        Some(pw),
        ACCOUNT_SESSION_AUTH_PASSWORD,
        now,
    )
    .await;

    let mut blocker_tx = fx.db.pool.begin().await.unwrap();
    let lock_key_j = format!("account-delete:{acct_j}");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(&lock_key_j)
        .execute(&mut *blocker_tx)
        .await
        .unwrap();

    let db_join = fx.db.clone();
    let acct_j_c = acct_j.clone();
    let club_j_c = club_id.clone();
    let mut join_handle = tokio::spawn(async move {
        db_join
            .join_club_with_account(&acct_j_c, &club_j_c, "Race Joiner", "", now)
            .await
    });

    // Must remain blocked while advisory lock is held!
    let timed = tokio::time::timeout(std::time::Duration::from_millis(150), &mut join_handle).await;
    assert!(
        timed.is_err(),
        "join_club_with_account MUST block until account-delete advisory lock is released"
    );

    // Commit account deletion inside the lock-holding transaction!
    ClubDb::delete_account_in_tx(&mut blocker_tx, &acct_j, &sess_j, Some(pw), now)
        .await
        .expect("delete_account_in_tx must succeed");
    blocker_tx.commit().await.unwrap();

    let join_res = join_handle.await.unwrap();
    assert_eq!(
        join_res,
        Err(AccountJoinError::NotFound),
        "join after account delete must fail with NotFound, got {join_res:?}"
    );
    let mem_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM club_memberships m
         JOIN pilots p ON p.id = m.pilot_id
         WHERE m.club_id = $1 AND lower(p.email) = lower($2)",
    )
    .bind(&club_id)
    .bind(&email_j)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    let pilot_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pilots WHERE club_id = $1 AND lower(email) = lower($2)",
    )
    .bind(&club_id)
    .bind(&email_j)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(mem_count, 0, "no ghost membership row allowed");
    assert_eq!(pilot_count, 0, "no orphan pilot row allowed");

    // --- Part B: approve_membership vs delete_account deterministic interleaving ---
    let email_a = format!("m34-appr-{}@test.example", &new_id()[..6]);
    let (acct_a, sess_a, _) = create_account_session(
        &fx.db,
        &email_a,
        true,
        Some(pw),
        ACCOUNT_SESSION_AUTH_PASSWORD,
        now,
    )
    .await;
    let (pid_a, mid_a, _) = add_club_member_with_status(
        &fx.db,
        &acct_a,
        &email_a,
        "Race Approve",
        &club_id,
        "pilot",
        "pending",
    )
    .await;

    let mut blocker_a = fx.db.pool.begin().await.unwrap();
    let lock_key_a = format!("account-delete:{acct_a}");
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(&lock_key_a)
        .execute(&mut *blocker_a)
        .await
        .unwrap();

    let db_appr = fx.db.clone();
    let mid_a_c = mid_a.clone();
    let mut appr_handle =
        tokio::spawn(async move { db_appr.approve_membership(&mid_a_c, now).await });

    let timed_a =
        tokio::time::timeout(std::time::Duration::from_millis(150), &mut appr_handle).await;
    assert!(
        timed_a.is_err(),
        "approve_membership MUST block until account-delete advisory lock is released"
    );

    // Simulate user leaving/removing membership then deleting account while lock is held.
    sqlx::query("DELETE FROM club_memberships WHERE id = $1")
        .bind(&mid_a)
        .execute(&mut *blocker_a)
        .await
        .unwrap();
    ClubDb::delete_account_in_tx(&mut blocker_a, &acct_a, &sess_a, Some(pw), now)
        .await
        .expect("delete_account_in_tx succeeds after membership removed");
    blocker_a.commit().await.unwrap();

    let appr_res = appr_handle.await.unwrap();
    assert!(
        appr_res.is_err(),
        "approve_membership after account delete must fail, got {appr_res:?}"
    );
    let active_mem_a: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM club_memberships WHERE pilot_id = $1")
            .bind(&pid_a)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(active_mem_a, 0, "no ghost membership after approve race");
    let pilot_status_a: String = sqlx::query_scalar("SELECT status FROM pilots WHERE id = $1")
        .bind(&pid_a)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    assert_ne!(
        pilot_status_a, "approved",
        "detached pilot must not be approved"
    );
}

#[test]
fn item3_m34_all_membership_mutation_paths_call_shared_lock_helper() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/club");
    let checks: &[(&str, &str)] = &[
        ("db_account.rs", "pub async fn join_club_with_account("),
        ("db_account.rs", "pub async fn register_into_account("),
        ("db_account.rs", "pub async fn approve_membership("),
        ("db_account.rs", "pub(crate) async fn delete_account_in_tx("),
        ("db_account.rs", "pub async fn leave_club_by_slug("),
        (
            "db_memberships_admin.rs",
            "pub async fn accept_membership_invite(",
        ),
        (
            "db_access_pg.rs",
            "pub async fn approve_pilot_with_checkout(",
        ),
        (
            "db_club_lifecycle.rs",
            "pub async fn try_consume_magic_club_verify(",
        ),
        ("db.rs", "pub async fn approve_pilot("),
    ];
    for (file, fn_sig) in checks {
        let src = std::fs::read_to_string(base.join(file)).unwrap();
        let start = src
            .find(fn_sig)
            .unwrap_or_else(|| panic!("{file} missing {fn_sig}"));
        let rest = &src[start + fn_sig.len()..];
        let next_fn = rest
            .find("\n    pub ")
            .or_else(|| rest.find("\n    async fn "))
            .unwrap_or(rest.len());
        let body = &rest[..next_fn];
        assert!(
            body.contains("lock_account_for_membership_change("),
            "{file}::{fn_sig} must call shared lock_account_for_membership_change"
        );
    }
}

// ===========================================================================
// Item 4 (L-91): per-device 409 flight_mismatch throttle
// ===========================================================================

#[tokio::test]
#[serial]
async fn item4_l91_flight_mismatch_throttled_per_device_and_other_device_unaffected() {
    let mut fx = setup_fx().await;
    let t0 = (now_ms() / 1000) * 1000;
    fx.clock_ms.store(t0, Ordering::SeqCst);
    assert_eq!(
        fx.live.mismatch_throttle_policy(),
        OverflowPolicy::RejectNew
    );

    // Device 1 (attacker / probing valid token)
    let (a1, _, _) = create_account_session(
        &fx.db,
        &format!("l91-d1-{}@test.example", &new_id()[..6]),
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        t0,
    )
    .await;
    let p1 = fx.db.ensure_personal_pilot(&a1, t0).await.unwrap();
    let (d1, tok1) = fx.db.create_device(&p1, "dev-1").await.unwrap();
    let closed_f1 = fx
        .db
        .start_personal_flight_atomic(
            &p1,
            &a1,
            &d1,
            "CC-L91A",
            FlightVisibility::Private,
            false,
            None,
            t0 - 300_000,
        )
        .await
        .unwrap();
    fx.db
        .end_flight(&p1, &closed_f1, t0 - 200_000)
        .await
        .unwrap();
    let _open_f1 = fx
        .db
        .start_personal_flight_atomic(
            &p1,
            &a1,
            &d1,
            "CC-L91B",
            FlightVisibility::Private,
            false,
            None,
            t0 - 60_000,
        )
        .await
        .unwrap();

    // Device 2 (legitimate concurrent device)
    let (a2, _, _) = create_account_session(
        &fx.db,
        &format!("l91-d2-{}@test.example", &new_id()[..6]),
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        t0,
    )
    .await;
    let p2 = fx.db.ensure_personal_pilot(&a2, t0).await.unwrap();
    let (d2, tok2) = fx.db.create_device(&p2, "dev-2").await.unwrap();
    let open_f2 = fx
        .db
        .start_personal_flight_atomic(
            &p2,
            &a2,
            &d2,
            "CC-L91C",
            FlightVisibility::Private,
            false,
            None,
            t0 - 60_000,
        )
        .await
        .unwrap();

    let expected_409 = json!({ "error": "flight_mismatch", "reason": "flight_mismatch" });
    let wrong_ids = [
        closed_f1.as_str(),
        open_f2.as_str(),
        "unknown-flight-id-xyz",
    ];

    // First N (= MAX_INGEST_FLIGHT_MISMATCH_PER_MINUTE = 10) mismatches return identical 409 body.
    for i in 0..MAX_INGEST_FLIGHT_MISMATCH_PER_MINUTE {
        let fid = wrong_ids[i % wrong_ids.len()];
        let (st, _, body) = send_json(
            &mut fx.router,
            "POST",
            "/api/ingest",
            None,
            Some(&tok1),
            &[],
            Some(json!({
                "flight_id": fid,
                "fixes": [{ "t": t0 - 1_000, "lat": -33.45, "lon": -70.55 }]
            })),
        )
        .await;
        assert_eq!(st, StatusCode::CONFLICT, "mismatch {i}: {body}");
        assert_eq!(body, expected_409);
    }

    // (N+1)th mismatch -> 429 Too Many Requests with Retry-After header!
    let (st_429, hdrs_429, body_429) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&tok1),
        &[],
        Some(json!({
            "flight_id": "probe-n-plus-1",
            "fixes": [{ "t": t0 - 1_000, "lat": -33.45, "lon": -70.55 }]
        })),
    )
    .await;
    assert_eq!(st_429, StatusCode::TOO_MANY_REQUESTS, "{body_429}");
    let retry_after: u64 = hdrs_429
        .get(header::RETRY_AFTER)
        .expect("429 mismatch must send Retry-After")
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=60).contains(&retry_after), "retry_after={retry_after}");

    // /api/osmand on the same device shares the mismatch throttle -> 429 + Retry-After.
    let (st_os_429, hdrs_os_429, _) = send_raw(
        &mut fx.router,
        "POST",
        &format!(
            "/api/osmand?id={tok1}&lat=-33.45&lon=-70.55&timestamp={}&flight_id=probe-osmand",
            (t0 - 1_000) / 1000
        ),
        None,
        None,
        &[],
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(st_os_429, StatusCode::TOO_MANY_REQUESTS);
    assert!(hdrs_os_429.get(header::RETRY_AFTER).is_some());

    // Legit ingest by Device 2 is completely unaffected!
    let (st_d2, _, b_d2) = send_json(
        &mut fx.router,
        "POST",
        "/api/ingest",
        None,
        Some(&tok2),
        &[],
        Some(json!({
            "flight_id": open_f2,
            "fixes": [{ "t": t0 - 1_000, "lat": -33.45, "lon": -70.55 }]
        })),
    )
    .await;
    assert_eq!(st_d2, StatusCode::OK, "{b_d2}");
    assert_eq!(b_d2["accepted"], json!(1));
}

// ===========================================================================
// Item 5 (L-92): nearby_public_positions_for_club non_club flag
// ===========================================================================

#[tokio::test]
#[serial]
async fn item5_l92_nearby_public_non_club_true_only_for_pilot_tails_in_same_response() {
    let mut fx = setup_fx().await;
    let _dem_guard = EnvGuard::set("FLIGHT_DATA", fx._dir.path());
    write_flat_dem_tile(fx._dir.path(), "N34W071", 500);
    let now = now_ms();

    let (club_a, _) = create_live_club(&fx.db, "l92-a", -33.456, -70.548).await;
    let (club_b, club_b_slug) = create_live_club(&fx.db, "l92-b", -33.460, -70.550).await;

    // Admin in viewing club A
    let adm_email = format!("l92-adm-{}@test.example", &new_id()[..6]);
    let (adm_acct, _, _) = create_account_session(
        &fx.db,
        &adm_email,
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (adm_pid, _, adm_cookie) = add_club_member_with_status(
        &fx.db, &adm_acct, &adm_email, "Admin A", &club_a, "admin", "active",
    )
    .await;

    // Flight 0: own-club (club A) public aircraft — must set neither nonClub nor otherClub
    fx.db.upsert_type(&club_a, "C172").await.unwrap();
    fx.db
        .upsert_aircraft(&club_a, "CC-OWNA", "C172", true)
        .await
        .unwrap();
    fx.db
        .add_checkout(&adm_pid, "C172", None, None)
        .await
        .unwrap();
    let (adm_dev, _) = fx.db.create_device(&adm_pid, "dev-a").await.unwrap();
    fx.db
        .issue_open_checkin(&adm_pid, &adm_dev, &club_a, now)
        .await
        .unwrap();
    let fid_own_club = fx
        .db
        .start_flight(
            &adm_pid,
            &adm_dev,
            "CC-OWNA",
            FlightVisibility::Public,
            false,
            now - 60_000,
        )
        .await
        .unwrap();
    fx.db
        .append_fix(
            &fid_own_club,
            &adm_dev,
            &LiveFix {
                t: now - 6_000,
                lat: -33.456,
                lon: -70.548,
                alt: Some(1200.0),
                alt_ref: Some("msl".to_string()),
                speed: Some(30.0),
                course: Some(45.0),
                accuracy: Some(5.0),
            },
        )
        .await
        .unwrap();

    // Flight 1: other-club (club B) public aircraft
    fx.db.upsert_type(&club_b, "C172").await.unwrap();
    fx.db
        .upsert_aircraft(&club_b, "CC-OTHC", "C172", true)
        .await
        .unwrap();
    let pb_email = format!("l92-pb-{}@test.example", &new_id()[..6]);
    let (pb_acct, _, _) = create_account_session(
        &fx.db,
        &pb_email,
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (pb_pid, _, _) = add_club_member_with_status(
        &fx.db, &pb_acct, &pb_email, "Pilot B", &club_b, "pilot", "active",
    )
    .await;
    fx.db
        .add_checkout(&pb_pid, "C172", None, None)
        .await
        .unwrap();
    let (pb_dev, _) = fx.db.create_device(&pb_pid, "dev-b").await.unwrap();
    fx.db
        .issue_open_checkin(&pb_pid, &pb_dev, &club_b, now)
        .await
        .unwrap();
    let fid_other_club = fx
        .db
        .start_flight(
            &pb_pid,
            &pb_dev,
            "CC-OTHC",
            FlightVisibility::Public,
            false,
            now - 60_000,
        )
        .await
        .unwrap();
    fx.db
        .append_fix(
            &fid_other_club,
            &pb_dev,
            &LiveFix {
                t: now - 5_000,
                lat: -33.46,
                lon: -70.55,
                alt: Some(1200.0),
                alt_ref: Some("msl".to_string()),
                speed: Some(30.0),
                course: Some(90.0),
                accuracy: Some(5.0),
            },
        )
        .await
        .unwrap();

    // Flight 2: club-less pilot-tail public flight
    let cl_email = format!("l92-cl-{}@test.example", &new_id()[..6]);
    let (cl_acct, _, _) = create_account_session(
        &fx.db,
        &cl_email,
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let cl_pid = fx.db.ensure_personal_pilot(&cl_acct, now).await.unwrap();
    let (cl_dev, _) = fx.db.create_device(&cl_pid, "dev-cl").await.unwrap();
    let fid_pilot_tail = fx
        .db
        .start_personal_flight_atomic(
            &cl_pid,
            &cl_acct,
            &cl_dev,
            "CC-FREE",
            FlightVisibility::Public,
            false,
            None,
            now - 60_000,
        )
        .await
        .unwrap();
    fx.db
        .append_fix(
            &fid_pilot_tail,
            &cl_dev,
            &LiveFix {
                t: now - 4_000,
                lat: -33.45,
                lon: -70.54,
                alt: Some(1200.0),
                alt_ref: Some("msl".to_string()),
                speed: Some(30.0),
                course: Some(180.0),
                accuracy: Some(5.0),
            },
        )
        .await
        .unwrap();

    // Direct DB check: own-club excluded from nearby; nearby rows carry exact non_club / other_club flags
    let nearby = fx.db.nearby_public_positions_for_club(&club_a, now).await;
    assert!(
        !nearby.iter().any(|p| p.flight_id == fid_own_club),
        "own-club flight must not appear in nearby_public_positions_for_club"
    );
    let pos_club = nearby
        .iter()
        .find(|p| p.flight_id == fid_other_club)
        .expect("other-club row");
    let pos_pilot = nearby
        .iter()
        .find(|p| p.flight_id == fid_pilot_tail)
        .expect("pilot-tail row");
    assert_eq!(pos_club.tail_source, TAIL_SOURCE_CLUB);
    assert!(
        !pos_club.non_club,
        "other-club aircraft must have non_club=false"
    );
    assert!(
        pos_club.other_club,
        "other-club aircraft must have other_club=true"
    );
    assert_eq!(pos_pilot.tail_source, TAIL_SOURCE_PILOT);
    assert!(
        pos_pilot.non_club,
        "club-less pilot tail must have non_club=true"
    );
    assert!(
        !pos_pilot.other_club,
        "club-less pilot tail must have other_club=false"
    );

    // HTTP check: own-club, other-club, and pilot-tail rows in one response on map & trails endpoints
    for (uri, key) in [
        ("/api/admin/map", "aircraft"),
        ("/api/display/map", "aircraft"),
        ("/api/admin/map/trails", "trails"),
        ("/api/display/map/trails", "trails"),
    ] {
        let (st, _, body) = send_json(
            &mut fx.router,
            "GET",
            uri,
            Some(&adm_cookie),
            None,
            &[],
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{uri}: {body}");
        let items = body[key].as_array().unwrap();
        let row_own = items
            .iter()
            .find(|x| x["flightId"] == fid_own_club)
            .expect("own-club row in HTTP");
        let row_club = items
            .iter()
            .find(|x| x["flightId"] == fid_other_club)
            .expect("other-club row in HTTP");
        let row_pilot = items
            .iter()
            .find(|x| x["flightId"] == fid_pilot_tail)
            .expect("pilot-tail row in HTTP");

        assert_eq!(row_own["tailSource"], json!(TAIL_SOURCE_CLUB), "{uri}");
        assert!(
            row_own.get("nonClub").is_none(),
            "{uri} own-club must omit nonClub: {row_own}"
        );
        assert!(
            row_own.get("otherClub").is_none(),
            "{uri} own-club must omit otherClub: {row_own}"
        );

        assert_eq!(row_club["tailSource"], json!(TAIL_SOURCE_CLUB), "{uri}");
        assert_eq!(
            row_club["otherClub"],
            json!(true),
            "{uri} other-club must set otherClub=true: {row_club}"
        );
        assert!(
            row_club.get("nonClub").is_none() || row_club["nonClub"] == json!(false),
            "{uri} other-club must not have nonClub=true: {row_club}"
        );
        if key == "aircraft" {
            assert_eq!(row_club["clubSlug"], json!(club_b_slug), "{uri}");
        }

        assert_eq!(row_pilot["tailSource"], json!(TAIL_SOURCE_PILOT), "{uri}");
        assert_eq!(row_pilot["nonClub"], json!(true), "{uri}: {row_pilot}");
        assert!(
            row_pilot.get("otherClub").is_none(),
            "{uri} pilot-tail must omit otherClub: {row_pilot}"
        );
        assert!(row_pilot.get("clubSlug").is_none(), "{uri}");
    }
}

// ===========================================================================
// Item 6 (L-95): delete_account active/pending gate + DELETE /api/account/clubs/{slug}
// ===========================================================================

#[tokio::test]
#[serial]
async fn item6_l95_delete_account_active_pending_gate_and_self_serve_leave_club() {
    let mut fx = setup_fx().await;
    let now = now_ms();
    let pw = "LeaveClubPass99!";
    let (club_id, club_slug) = create_live_club(&fx.db, "l95-club", -33.456, -70.548).await;
    let (club2_id, _club2_slug) = create_live_club(&fx.db, "l95-club2", -33.46, -70.55).await;

    // 1. Removed / suspended / rejected / left memberships do NOT block DELETE /api/account!
    for non_blocking_status in ["suspended", "removed", "rejected", "left"] {
        let email = format!("l95-{non_blocking_status}-{}@test.example", &new_id()[..6]);
        let (acct, _, cookie) = create_account_session(
            &fx.db,
            &email,
            true,
            Some(pw),
            ACCOUNT_SESSION_AUTH_PASSWORD,
            now,
        )
        .await;
        let (pid, mid, _) = add_club_member_with_status(
            &fx.db,
            &acct,
            &email,
            "Former Member",
            &club_id,
            "pilot",
            non_blocking_status,
        )
        .await;

        let (st, _, body) = send_json(
            &mut fx.router,
            "DELETE",
            "/api/account",
            Some(&cookie),
            None,
            &[("sec-fetch-site", "same-origin")],
            Some(json!({ "password": pw })),
        )
        .await;
        assert_eq!(
            st,
            StatusCode::OK,
            "membership status '{non_blocking_status}' must NOT block delete: {body}"
        );
        assert!(fx.db.account_by_id(&acct).await.is_none());
        // Non-blocking membership row cleaned up; historical club pilot detached (retained).
        let mem_left: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM club_memberships WHERE id = $1")
                .bind(&mid)
                .fetch_one(&fx.db.pool)
                .await
                .unwrap();
        assert_eq!(mem_left, 0);
        let detached: Option<String> =
            sqlx::query_scalar("SELECT account_id FROM pilots WHERE id = $1")
                .bind(&pid)
                .fetch_one(&fx.db.pool)
                .await
                .unwrap();
        assert!(detached.is_none(), "club pilot row retained and detached");
    }

    // 2. Last active admin cannot leave -> 409 {"error":"last_admin"}; active membership blocks delete.
    let adm1_email = format!("l95-adm1-{}@test.example", &new_id()[..6]);
    let (adm1_acct, _, adm1_cookie) = create_account_session(
        &fx.db,
        &adm1_email,
        true,
        Some(pw),
        ACCOUNT_SESSION_AUTH_PASSWORD,
        now,
    )
    .await;
    let (_adm1_pid, adm1_mid, _) = add_club_member_with_status(
        &fx.db,
        &adm1_acct,
        &adm1_email,
        "Solo Admin",
        &club_id,
        "admin",
        "active",
    )
    .await;

    // Unauthenticated leave -> 401
    let (st_unauth, _, b_unauth) = send_raw(
        &mut fx.router,
        "DELETE",
        &format!("/api/account/clubs/{club_slug}"),
        None,
        None,
        &[
            ("origin", "https://evil.example"),
            ("host", "flight.example"),
        ],
        Some("text/plain"),
        Vec::new(),
    )
    .await;
    assert_eq!(st_unauth, StatusCode::UNAUTHORIZED);
    assert_eq!(
        serde_json::from_slice::<Value>(&b_unauth).unwrap(),
        json!({ "error": "unauthorized" })
    );

    // Cross-site CSRF on leave -> 403
    let (st_csrf, _, b_csrf) = send_json(
        &mut fx.router,
        "DELETE",
        &format!("/api/account/clubs/{club_slug}"),
        Some(&adm1_cookie),
        None,
        &[("sec-fetch-site", "cross-site")],
        None,
    )
    .await;
    assert_eq!(st_csrf, StatusCode::FORBIDDEN, "{b_csrf}");
    assert_eq!(b_csrf["error"], json!("csrf"));

    // Last active admin leave -> 409 {"error":"last_admin"}
    let (st_last, _, b_last) = send_json(
        &mut fx.router,
        "DELETE",
        &format!("/api/account/clubs/{club_slug}"),
        Some(&adm1_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        None,
    )
    .await;
    assert_eq!(st_last, StatusCode::CONFLICT, "{b_last}");
    assert_eq!(b_last, json!({ "error": "last_admin" }));

    // Active membership still blocks DELETE /api/account with 409 leave_clubs_first
    let (st_blk, _, b_blk) = send_json(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&adm1_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        Some(json!({ "password": pw })),
    )
    .await;
    assert_eq!(st_blk, StatusCode::CONFLICT, "{b_blk}");
    assert_eq!(b_blk["error"], json!("leave_clubs_first"));

    // 3. Another account CANNOT remove adm1's membership; unknown slug & non-member club return identical 404.
    let other_email = format!("l95-other-{}@test.example", &new_id()[..6]);
    let (_other_acct, _, other_cookie) = create_account_session(
        &fx.db,
        &other_email,
        true,
        Some(pw),
        ACCOUNT_SESSION_AUTH_PASSWORD,
        now,
    )
    .await;
    let (st_not_mem, _, raw_not_mem) = send_raw(
        &mut fx.router,
        "DELETE",
        &format!("/api/account/clubs/{club_slug}"),
        Some(&other_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        None,
        Vec::new(),
    )
    .await;
    let (st_unknown, _, raw_unknown) = send_raw(
        &mut fx.router,
        "DELETE",
        "/api/account/clubs/definitely-no-such-club-slug",
        Some(&other_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(st_not_mem, StatusCode::NOT_FOUND);
    assert_eq!(st_unknown, StatusCode::NOT_FOUND);
    assert_eq!(
        raw_not_mem, raw_unknown,
        "non-member and unknown slug must be byte-identical"
    );
    // adm1's membership is untouched!
    let adm1_still: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM club_memberships WHERE id = $1")
        .bind(&adm1_mid)
        .fetch_one(&fx.db.pool)
        .await
        .unwrap();
    assert_eq!(adm1_still, 1);

    // 4. Regular active member (also with club session in club1 & club2):
    //    active blocks delete -> leave club1 succeeds, writes audit row, revokes club1 session only,
    //    preserves club2 session -> after leaving both, DELETE /api/account succeeds!
    let mem_email = format!("l95-mem-{}@test.example", &new_id()[..6]);
    let (mem_acct, _, mem_acct_cookie) = create_account_session(
        &fx.db,
        &mem_email,
        true,
        Some(pw),
        ACCOUNT_SESSION_AUTH_PASSWORD,
        now,
    )
    .await;
    let (mem_pid1, mem_mid1, _) = add_club_member_with_status(
        &fx.db,
        &mem_acct,
        &mem_email,
        "Pilot Leave",
        &club_id,
        "pilot",
        "active",
    )
    .await;
    let (mem_pid2, _mem_mid2, _) = add_club_member_with_status(
        &fx.db,
        &mem_acct,
        &mem_email,
        "Pilot Leave",
        &club2_id,
        "pilot",
        "pending",
    )
    .await;
    let club1_pilot_sess = fx.db.create_pilot_session(&mem_pid1, now).await.unwrap();
    let club2_pilot_sess = fx.db.create_pilot_session(&mem_pid2, now).await.unwrap();

    // Leave club1 (with club1 session cookie also attached -> cleared!)
    let combined_cookie = format!("{mem_acct_cookie}; {SESSION_COOKIE}={club1_pilot_sess}");
    let (st_lv1, hdrs_lv1, b_lv1) = send_json(
        &mut fx.router,
        "DELETE",
        &format!("/api/account/clubs/{club_slug}"),
        Some(&combined_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        None,
    )
    .await;
    assert_eq!(st_lv1, StatusCode::OK, "{b_lv1}");
    assert_eq!(b_lv1, json!({ "ok": true }));
    let set_cookies: Vec<&str> = hdrs_lv1
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect();
    assert!(
        set_cookies
            .iter()
            .any(|c| c.starts_with(&format!("{SESSION_COOKIE}=;"))),
        "leaving active club must clear matching club session cookie: {set_cookies:?}"
    );
    // Club1 pilot session revoked, club2 pilot session untouched!
    assert!(fx
        .db
        .pilot_from_session(&club1_pilot_sess, now)
        .await
        .is_none());
    assert!(fx
        .db
        .pilot_from_session(&club2_pilot_sess, now)
        .await
        .is_some());
    // Audit row recorded!
    let audit_cnt: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log
         WHERE club_id = $1 AND actor_pilot_id = $2 AND action = 'leave_club'
           AND entity_type = 'membership' AND entity_id = $3",
    )
    .bind(&club_id)
    .bind(&mem_pid1)
    .bind(&mem_mid1)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(audit_cnt, 1, "leave must write an audit_log entry");

    // Still has pending membership in club2 -> DELETE /api/account still returns 409 leave_clubs_first!
    let (st_pend_blk, _, b_pend_blk) = send_json(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&mem_acct_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        Some(json!({ "password": pw })),
    )
    .await;
    assert_eq!(st_pend_blk, StatusCode::CONFLICT, "{b_pend_blk}");
    assert_eq!(b_pend_blk["error"], json!("leave_clubs_first"));

    // Leave pending club2 -> now DELETE /api/account succeeds!
    let club2_slug = fx.db.club_slug(&club2_id).await.unwrap();
    let (st_lv2, _, b_lv2) = send_json(
        &mut fx.router,
        "DELETE",
        &format!("/api/account/clubs/{club2_slug}"),
        Some(&mem_acct_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        None,
    )
    .await;
    assert_eq!(st_lv2, StatusCode::OK, "{b_lv2}");

    let (st_del, _, b_del) = send_json(
        &mut fx.router,
        "DELETE",
        "/api/account",
        Some(&mem_acct_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        Some(json!({ "password": pw })),
    )
    .await;
    assert_eq!(st_del, StatusCode::OK, "{b_del}");
    assert!(fx.db.account_by_id(&mem_acct).await.is_none());
}

// ===========================================================================
// Item 7 (L-96): /api/club/map/public SQL LIMIT 500
// ===========================================================================

#[tokio::test]
#[serial]
async fn item7_l96_public_map_caps_sql_candidates_at_500() {
    let mut fx = setup_fx().await;
    let _dem_guard = EnvGuard::set("FLIGHT_DATA", fx._dir.path());
    write_flat_dem_tile(fx._dir.path(), "N34W071", 500);
    let now = now_ms();

    assert_eq!(PUBLIC_LIVE_TRAILS_SQL_CANDIDATE_LIMIT, 500);
    assert!(
        ClubDb::LATEST_DEVICE_FIXES_PUBLIC_MAP_SQL.contains("LIMIT"),
        "public map candidate SQL must contain LIMIT"
    );

    let (club_id, club_slug) = create_live_club(&fx.db, "l96-cap", -33.456, -70.548).await;
    // Raise Rust-side `public_flights_cap` above 520 so ONLY the SQL LIMIT 500 caps the result!
    sqlx::query("UPDATE clubs SET public_flights_cap = 600 WHERE id = $1")
        .bind(&club_id)
        .execute(&fx.db.pool)
        .await
        .unwrap();
    fx.db.upsert_type(&club_id, "C172").await.unwrap();
    let type_id: String =
        sqlx::query_scalar("SELECT id FROM aircraft_types WHERE club_id = $1 AND code = 'C172'")
            .bind(&club_id)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();

    let email = format!("l96-p-{}@test.example", &new_id()[..6]);
    let (acct_id, _, _) =
        create_account_session(&fx.db, &email, true, None, ACCOUNT_SESSION_AUTH_MAGIC, now).await;
    let (pilot_id, _, _) = add_club_member_with_status(
        &fx.db,
        &acct_id,
        &email,
        "Bulk Pilot",
        &club_id,
        "pilot",
        "active",
    )
    .await;
    let (dev_id, _) = fx.db.create_device(&pilot_id, "bulk-dev").await.unwrap();

    // Bulk-insert 520 distinct active public club flights with distinct tails and increasing timestamps.
    // Put the 20 oldest flights right on the club base (0 km) and the 500 newest 1 km away so that
    // WITHOUT SQL `LIMIT 500 ORDER BY newest DESC`, `apply_public_cap` (nearest-first) would return
    // the 20 oldest flights on `/api/club/map/public`; WITH SQL `LIMIT 500`, none of the 20 oldest appear.
    let total = 520usize;
    let mut ac_ids = Vec::with_capacity(total);
    let mut tails = Vec::with_capacity(total);
    let mut flight_ids = Vec::with_capacity(total);
    let mut t_ms_list = Vec::with_capacity(total);
    let mut lats = Vec::with_capacity(total);
    for i in 0..total {
        ac_ids.push(format!("l96-ac-{i:04}"));
        tails.push(format!("CC-{i:04}"));
        flight_ids.push(format!("l96-fl-{i:04}"));
        t_ms_list.push(now - ((total - i) as i64) * 100);
        lats.push(if i < 20 { -33.456 } else { -33.465 });
    }

    sqlx::query(
        "INSERT INTO aircraft (id, club_id, tail, type_id, active)
         SELECT aid, $1, tail, $2, TRUE
         FROM UNNEST($3::text[], $4::text[]) AS t(aid, tail)",
    )
    .bind(&club_id)
    .bind(&type_id)
    .bind(&ac_ids)
    .bind(&tails)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO flights (id, pilot_id, aircraft_id, club_id, status, started_at, last_fix_at, visibility, show_pilot_name_public)
         SELECT fid, $1, aid, $2, 'active', tms - 60000, tms, 'public', FALSE
         FROM UNNEST($3::text[], $4::text[], $5::bigint[]) AS t(fid, aid, tms)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(&flight_ids)
    .bind(&ac_ids)
    .bind(&t_ms_list)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO fixes (flight_id, device_id, t_ms, received_at, lat, lon, alt, alt_ref)
         SELECT fid, $1, tms, tms, lat, -70.548, 1200.0, 'msl'
         FROM UNNEST($2::text[], $3::bigint[], $4::double precision[]) AS t(fid, tms, lat)",
    )
    .bind(&dev_id)
    .bind(&flight_ids)
    .bind(&t_ms_list)
    .bind(&lats)
    .execute(&fx.db.pool)
    .await
    .unwrap();

    // Direct helper check: bounded at 500, newest first (excludes oldest 20 flights 0..20).
    let limited = fx
        .db
        .latest_device_fixes_for_public_map(&club_id, PUBLIC_LIVE_TRAILS_SQL_CANDIDATE_LIMIT)
        .await;
    assert_eq!(limited.len(), PUBLIC_LIVE_TRAILS_SQL_CANDIDATE_LIMIT);
    assert_eq!(limited[0].flight_id, format!("l96-fl-{:04}", total - 1));
    assert!(
        !limited.iter().any(|r| r.flight_id == "l96-fl-0000"),
        "oldest flight beyond LIMIT 500 must not be loaded"
    );

    // HTTP endpoint /api/club/map/public returns <= 500 AND excludes the 20 oldest flights
    // that were dropped by SQL `LIMIT 500` even though they are closer to the club base!
    let (st, _, body) = send_json(
        &mut fx.router,
        "GET",
        &format!("/api/club/map/public?club={club_slug}"),
        None,
        None,
        &[],
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let ac = body["aircraft"].as_array().unwrap();
    assert!(!ac.is_empty());
    assert!(ac.len() <= PUBLIC_LIVE_TRAILS_SQL_CANDIDATE_LIMIT);
    assert!(
        !ac.iter().any(|a| a["flightId"] == json!("l96-fl-0000")),
        "oldest flight beyond SQL LIMIT 500 must never appear in /api/club/map/public"
    );
}

// ===========================================================================
// Item 8 (L-97): write/share paths return byte-identical 404 for non-owner & missing
// ===========================================================================

#[tokio::test]
#[serial]
async fn item8_l97_write_and_share_paths_return_byte_identical_404_for_non_owner_and_missing() {
    let mut fx = setup_fx().await;
    let now = now_ms();
    let (club_id, _) = create_live_club(&fx.db, "l97-club", -33.456, -70.548).await;
    fx.db.upsert_type(&club_id, "C172").await.unwrap();
    fx.db
        .upsert_aircraft(&club_id, "CC-L97A", "C172", true)
        .await
        .unwrap();

    // Owner: club pilot with an active flight + a share on it
    let owner_email = format!("l97-own-{}@test.example", &new_id()[..6]);
    let (owner_acct, _, _) = create_account_session(
        &fx.db,
        &owner_email,
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (owner_pid, _, _) = add_club_member_with_status(
        &fx.db,
        &owner_acct,
        &owner_email,
        "Owner",
        &club_id,
        "pilot",
        "active",
    )
    .await;
    fx.db
        .add_checkout(&owner_pid, "C172", None, None)
        .await
        .unwrap();
    let (owner_dev, _) = fx.db.create_device(&owner_pid, "owner-dev").await.unwrap();
    fx.db
        .issue_open_checkin(&owner_pid, &owner_dev, &club_id, now)
        .await
        .unwrap();
    let existing_fid = fx
        .db
        .start_flight(
            &owner_pid,
            &owner_dev,
            "CC-L97A",
            FlightVisibility::Private,
            false,
            now - 30_000,
        )
        .await
        .unwrap();
    let (share_row, _) = fx
        .db
        .create_flight_share(&owner_pid, &existing_fid)
        .await
        .unwrap();
    let existing_sid = share_row.id;

    // Non-owner actors:
    // 1. Other club pilot in same club
    let p2_email = format!("l97-p2-{}@test.example", &new_id()[..6]);
    let (p2_acct, _, _) = create_account_session(
        &fx.db,
        &p2_email,
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_, _, p2_cookie) = add_club_member_with_status(
        &fx.db,
        &p2_acct,
        &p2_email,
        "Other Pilot",
        &club_id,
        "pilot",
        "active",
    )
    .await;
    // 2. Club admin in same club (NOT flight owner)
    let adm_email = format!("l97-adm-{}@test.example", &new_id()[..6]);
    let (adm_acct, _, _) = create_account_session(
        &fx.db,
        &adm_email,
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let (_, _, adm_cookie) = add_club_member_with_status(
        &fx.db,
        &adm_acct,
        &adm_email,
        "Club Admin",
        &club_id,
        "admin",
        "active",
    )
    .await;
    // 3. Club-less account WITH personal pilot
    let cl_email = format!("l97-cl-{}@test.example", &new_id()[..6]);
    let (cl_acct, _, cl_cookie) = create_account_session(
        &fx.db,
        &cl_email,
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;
    let _ = fx.db.ensure_personal_pilot(&cl_acct, now).await.unwrap();
    // 4. Club-less account WITHOUT personal pilot (exercises resolve_flight_owner_actor branch!)
    let np_email = format!("l97-np-{}@test.example", &new_id()[..6]);
    let (_, _, np_cookie) = create_account_session(
        &fx.db,
        &np_email,
        true,
        None,
        ACCOUNT_SESSION_AUTH_MAGIC,
        now,
    )
    .await;

    // Confirm exact read-path 404 body bytes for reference.
    let (ref_st, _, ref_bytes) = send_raw(
        &mut fx.router,
        "GET",
        "/api/flights/does-not-exist-ref/fixes",
        Some(&p2_cookie),
        None,
        &[],
        None,
        Vec::new(),
    )
    .await;
    assert_eq!(ref_st, StatusCode::NOT_FOUND);
    assert_eq!(ref_bytes, br#"{"error":"not found"}"#);

    let random_fid = format!("nonexistent-flight-{}", new_id());
    let random_sid = format!("nonexistent-share-{}", new_id());

    struct WritePathCase {
        label: &'static str,
        method: &'static str,
        existing_uri: String,
        random_uri: String,
        ct: Option<&'static str>,
        body: Vec<u8>,
    }

    for (actor_label, cookie) in [
        ("other_club_pilot", p2_cookie.as_str()),
        ("club_admin", adm_cookie.as_str()),
        ("clubless_with_pilot", cl_cookie.as_str()),
        ("clubless_no_pilot", np_cookie.as_str()),
    ] {
        let cases = [
            WritePathCase {
                label: "patch_flight",
                method: "PATCH",
                existing_uri: format!("/api/flights/{existing_fid}"),
                random_uri: format!("/api/flights/{random_fid}"),
                ct: Some("application/json"),
                body: br#"{"visibility":"public"}"#.to_vec(),
            },
            WritePathCase {
                label: "end_flight",
                method: "POST",
                existing_uri: format!("/api/flights/{existing_fid}/end"),
                random_uri: format!("/api/flights/{random_fid}/end"),
                ct: None,
                body: Vec::new(),
            },
            WritePathCase {
                label: "set_flight_trail_minutes",
                method: "PATCH",
                existing_uri: format!("/api/flights/{existing_fid}/trail"),
                random_uri: format!("/api/flights/{random_fid}/trail"),
                ct: Some("application/json"),
                body: br#"{"trailMinutes":10}"#.to_vec(),
            },
            WritePathCase {
                label: "delete_my_flight",
                method: "DELETE",
                existing_uri: format!("/api/me/flights/{existing_fid}"),
                random_uri: format!("/api/me/flights/{random_fid}"),
                ct: None,
                body: Vec::new(),
            },
            WritePathCase {
                label: "share_create",
                method: "POST",
                existing_uri: format!("/api/me/flights/{existing_fid}/shares"),
                random_uri: format!("/api/me/flights/{random_fid}/shares"),
                ct: None,
                body: Vec::new(),
            },
            WritePathCase {
                label: "share_list",
                method: "GET",
                existing_uri: format!("/api/me/flights/{existing_fid}/shares"),
                random_uri: format!("/api/me/flights/{random_fid}/shares"),
                ct: None,
                body: Vec::new(),
            },
            WritePathCase {
                label: "share_revoke",
                method: "DELETE",
                existing_uri: format!("/api/me/flights/{existing_fid}/shares/{existing_sid}"),
                random_uri: format!("/api/me/flights/{random_fid}/shares/{random_sid}"),
                ct: None,
                body: Vec::new(),
            },
        ];

        for WritePathCase {
            label,
            method,
            existing_uri,
            random_uri,
            ct,
            body,
        } in cases
        {
            let (st_ex, _, bytes_ex) = send_raw(
                &mut fx.router,
                method,
                &existing_uri,
                Some(cookie),
                None,
                &[("sec-fetch-site", "same-origin")],
                ct,
                body.clone(),
            )
            .await;
            let (st_rn, _, bytes_rn) = send_raw(
                &mut fx.router,
                method,
                &random_uri,
                Some(cookie),
                None,
                &[("sec-fetch-site", "same-origin")],
                ct,
                body,
            )
            .await;
            assert_eq!(
                st_ex,
                StatusCode::NOT_FOUND,
                "{actor_label} {label} existing must return 404, got {st_ex} ({})",
                String::from_utf8_lossy(&bytes_ex)
            );
            assert_eq!(
                st_rn,
                StatusCode::NOT_FOUND,
                "{actor_label} {label} random must return 404, got {st_rn} ({})",
                String::from_utf8_lossy(&bytes_rn)
            );
            assert_eq!(
                bytes_ex, ref_bytes,
                "{actor_label} {label} existing body must match read-path 404"
            );
            assert_eq!(
                bytes_ex, bytes_rn,
                "{actor_label} {label} existing vs random body must be byte-identical"
            );
        }
    }

    // Owner's flight and share remain active & untouched.
    let f_after = fx
        .db
        .get_flight(&existing_fid)
        .await
        .expect("flight still exists");
    assert_eq!(f_after.status, "active");
    assert_eq!(f_after.visibility, "private");
    let shares_after = fx
        .db
        .list_flight_shares(&owner_pid, &existing_fid)
        .await
        .unwrap();
    assert_eq!(shares_after.len(), 1);
    assert!(!shares_after[0].revoked);

    // Role-only 403 (does not depend on flight ID) is preserved: non-admin PATCH /api/admin/club/trail -> 403.
    let (st_role, _, _) = send_json(
        &mut fx.router,
        "PATCH",
        "/api/admin/club/trail",
        Some(&p2_cookie),
        None,
        &[("sec-fetch-site", "same-origin")],
        Some(json!({ "trailMinutes": 10 })),
    )
    .await;
    assert_eq!(st_role, StatusCode::FORBIDDEN);
}

// ===========================================================================
// Item 9 (L-87): WindowThrottle also inherits pinned EvictSoonestExpiring protection
// ===========================================================================

#[test]
fn item9_l87_window_throttle_pins_saturated_keys_and_rejects_when_all_pinned() {
    let cap = 3;
    let limit = 2;
    let window = 600;
    let wt = WindowThrottle::with_max_keys(cap);
    let t0 = 60_000_u64;

    // Saturate key-0 at earliest timestamp t0; fill rest with under-limit keys.
    assert!(wt.allow("ip-pinned", t0, limit, window));
    assert!(wt.allow("ip-pinned", t0, limit, window));
    assert!(!wt.allow("ip-pinned", t0, limit, window));
    assert!(wt.allow("ip-u1", t0 + 1, limit, window));
    assert!(wt.allow("ip-u2", t0 + 2, limit, window));
    assert_eq!(wt.len(), cap);

    // Flood 2*cap new keys -> pinned key MUST stay limited and map stays <= cap!
    for j in 0..(2 * cap) {
        assert!(wt.allow(&format!("ip-flood-{j}"), t0 + 10 + j as u64, limit, window));
        assert!(wt.len() <= cap);
    }
    assert!(
        !wt.allow("ip-pinned", t0 + 20, limit, window),
        "L-87: WindowThrottle must pin saturated key under 2x-cap flood"
    );

    // Saturate all slots -> new key MUST be rejected.
    let wt_all = WindowThrottle::with_max_keys(cap);
    for i in 0..cap {
        let k = format!("ip-all-{i}");
        assert!(wt_all.allow(&k, t0, limit, window));
        assert!(wt_all.allow(&k, t0, limit, window));
    }
    assert!(!wt_all.allow("ip-rejected", t0 + 5, limit, window));
    assert_eq!(wt_all.len(), cap);
}
