//! Integration tests for account-session kiosk auth, role/club scoping,
//! removed display-token endpoints (404), sliding session renewal, and migration 000028.

use crate::club::auth::{
    digest_token, hash_password, new_id, ACCOUNT_COOKIE, COOKIE, SESSION_RENEW_THROTTLE_SEC,
    SESSION_TTL_SEC,
};
use crate::club::{db::now_ms, ClubDb};
use crate::{
    auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
};
use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use serial_test::serial;
use std::collections::HashSet;
use std::sync::Arc;
use tempfile::TempDir;
use tower::Service;

struct KioskFx {
    _data_dir: TempDir,
    dist_dir: std::path::PathBuf,
    club: ClubDb,
    router: axum::Router,
    club_a_id: String,
    club_a_slug: String,
    club_b_slug: String,
    admin_a_account_cookie: String,
    admin_a_account_sess_raw: String,
    admin_a_club_cookie: String,
    admin_a_club_sess_raw: String,
    admin_b_account_cookie: String,
    pilot_a_account_cookie: String,
    staff_a_account_cookie: String,
    superadmin_a_account_cookie: String,
}

async fn seed_user_with_account_session(
    db: &ClubDb,
    club_id: &str,
    email: &str,
    role: &str,
    global_role: &str,
) -> (String, String, String) {
    let now = now_ms();
    let hash = hash_password("password12").unwrap();
    let account_id = new_id();
    let pilot_id = new_id();
    let mem_id = new_id();

    sqlx::query(
        "INSERT INTO accounts (id, email, password_hash, email_verified_at, global_role, created_at)
         VALUES ($1, $2, $3, $4, $5, $4)",
    )
    .bind(&account_id)
    .bind(email)
    .bind(&hash)
    .bind(now)
    .bind(global_role)
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO pilots (id, club_id, account_id, name, email, license_no, status, role, global_role, password_hash, email_verified_at, created_at)
         VALUES ($1, $2, $3, $4, $5, '', 'approved', $6, $7, $8, $9, $9)",
    )
    .bind(&pilot_id)
    .bind(club_id)
    .bind(&account_id)
    .bind(email)
    .bind(email)
    .bind(role)
    .bind(global_role)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

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

    let raw_sess = db
        .create_account_session(&account_id, Some(&mem_id), now)
        .await
        .unwrap();
    (
        pilot_id,
        raw_sess.clone(),
        format!("{ACCOUNT_COOKIE}={raw_sess}"),
    )
}

async fn setup_kiosk_fx() -> KioskFx {
    let data_dir = tempfile::tempdir().unwrap();
    let dist_dir = data_dir.path().join("dist");
    std::fs::create_dir_all(&dist_dir).unwrap();
    std::fs::write(
        dist_dir.join("index.html"),
        "<!doctype html><html><body>spa</body></html>",
    )
    .unwrap();
    std::fs::write(
        dist_dir.join("club-screen.html"),
        "<!doctype html><html lang=\"es-CL\"><body>kiosk-screen</body></html>",
    )
    .unwrap();

    let club = ClubDb::open_test().await.unwrap();
    let now = now_ms();
    let club_a_id = club.default_club_id().await.unwrap();
    let club_a_slug = format!("club-a-{}", &new_id()[..6]);
    sqlx::query(
        "UPDATE clubs SET name = 'Club Alfa', slug = $1, aerodrome_icao = 'SCAL', latitude = -33.45, longitude = -70.55 WHERE id = $2",
    )
    .bind(&club_a_slug)
    .bind(&club_a_id)
    .execute(&club.pool)
    .await
    .unwrap();

    let club_b_id = new_id();
    let club_b_slug = format!("club-b-{}", &new_id()[..6]);
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at, aerodrome_icao, latitude, longitude, show_public_tracks)
         VALUES ($1, 'Club Beta', $2, $3, 'live', $3, 'SCBE', -36.82, -73.05, TRUE)",
    )
    .bind(&club_b_id)
    .bind(&club_b_slug)
    .bind(now)
    .execute(&club.pool)
    .await
    .unwrap();

    let (admin_a_pilot_id, admin_a_account_sess_raw, admin_a_account_cookie) =
        seed_user_with_account_session(&club, &club_a_id, "admin-a@test", "admin", "").await;
    let admin_a_club_sess_raw = club
        .create_pilot_session(&admin_a_pilot_id, now)
        .await
        .unwrap();
    let admin_a_club_cookie = format!("{COOKIE}={admin_a_club_sess_raw}");

    let (_admin_b_pilot_id, _, admin_b_account_cookie) =
        seed_user_with_account_session(&club, &club_b_id, "admin-b@test", "admin", "").await;
    let (_pilot_a_id, _, pilot_a_account_cookie) =
        seed_user_with_account_session(&club, &club_a_id, "pilot-a@test", "pilot", "").await;
    let (_staff_a_id, _, staff_a_account_cookie) =
        seed_user_with_account_session(&club, &club_a_id, "staff-a@test", "staff", "").await;
    let (_sa_a_id, _, superadmin_a_account_cookie) =
        seed_user_with_account_session(&club, &club_a_id, "sa-a@test", "pilot", "superadmin").await;

    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let state = AppState {
        dist_dir: dist_dir.clone(),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live: Arc::new(LiveEngine::new(data_dir.path())),
        store: Store::new(data_dir.path()),
        club: Some(club.clone()),
        video_queue,
        login_throttle: Default::default(),
        device_register_throttle: Default::default(),
        share_lookup_throttle: Default::default(),
        account_password_throttle: Default::default(),
        account_password_account_throttle: Default::default(),
        public_live_trails_throttle: Default::default(),
        mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
        webauthn: None,
        sse_limiter: Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails: HashSet::new(),
    };
    let router = build_router(state);

    KioskFx {
        _data_dir: data_dir,
        dist_dir,
        club,
        router,
        club_a_id,
        club_a_slug,
        club_b_slug,
        admin_a_account_cookie,
        admin_a_account_sess_raw,
        admin_a_club_cookie,
        admin_a_club_sess_raw,
        admin_b_account_cookie,
        pilot_a_account_cookie,
        staff_a_account_cookie,
        superadmin_a_account_cookie,
    }
}

async fn send(
    router: &mut axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: &str,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    if !body.is_empty() {
        builder = builder
            .header(header::CONTENT_TYPE, "application/json")
            .header("sec-fetch-site", "same-origin");
    }
    let req = builder.body(Body::from(body.to_string())).unwrap();
    let res = Service::call(router, req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    (status, headers, text)
}

#[tokio::test]
#[serial]
async fn signed_in_admin_and_superadmin_can_access_kiosk_pages_and_apis() {
    let mut fx = setup_kiosk_fx().await;
    let slug_a = fx.club_a_slug.clone();
    let cookies = [
        fx.admin_a_account_cookie.clone(),
        fx.admin_a_club_cookie.clone(),
        fx.superadmin_a_account_cookie.clone(),
    ];

    for cookie in cookies {
        for page_uri in [
            "/club-screen.html".to_string(),
            "/club/screen".to_string(),
            format!("/c/{slug_a}/screen"),
        ] {
            let (st, hdrs, body) = send(&mut fx.router, "GET", &page_uri, Some(&cookie), "").await;
            assert_eq!(st, StatusCode::OK, "GET {page_uri}");
            assert!(body.contains("kiosk-screen"), "GET {page_uri} html body");
            assert_eq!(
                hdrs.get(header::CONTENT_SECURITY_POLICY)
                    .and_then(|v| v.to_str().ok()),
                Some(crate::html_csp::HTML_MAP_CSP),
                "GET {page_uri} must send HTML_MAP_CSP"
            );
            assert_eq!(
                hdrs.get(header::REFERRER_POLICY)
                    .and_then(|v| v.to_str().ok()),
                Some(crate::html_csp::SENSITIVE_REFERRER_POLICY),
                "GET {page_uri} must send no-referrer"
            );
        }

        let (st, _, map_body) =
            send(&mut fx.router, "GET", "/api/display/map", Some(&cookie), "").await;
        assert_eq!(st, StatusCode::OK);
        let map_json: serde_json::Value = serde_json::from_str(&map_body).unwrap();
        assert!(map_json["aircraft"].is_array());

        let (st, _, base_body) = send(
            &mut fx.router,
            "GET",
            "/api/display/base",
            Some(&cookie),
            "",
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let base_json: serde_json::Value = serde_json::from_str(&base_body).unwrap();
        assert_eq!(base_json["icao"], "SCAL");
        assert_eq!(base_json["clubName"], "Club Alfa");

        let (st, _, layout_body) = send(
            &mut fx.router,
            "GET",
            "/api/display/kiosk-layout",
            Some(&cookie),
            "",
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        let layout_json: serde_json::Value = serde_json::from_str(&layout_body).unwrap();
        assert!(layout_json["size"].is_number());

        let (st, _, qr_meta) = send(
            &mut fx.router,
            "GET",
            "/api/display/checkin-qr",
            Some(&cookie),
            "",
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert!(qr_meta.contains("stepSeconds"));

        let (st, hdrs, svg) = send(
            &mut fx.router,
            "GET",
            "/api/club/screen/qr.svg",
            Some(&cookie),
            "",
        )
        .await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(
            hdrs.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()),
            Some("image/svg+xml; charset=utf-8")
        );
        assert!(svg.contains("<svg"));
    }
}

#[tokio::test]
#[serial]
async fn signed_out_kiosk_pages_redirect_safely_and_apis_return_401() {
    let mut fx = setup_kiosk_fx().await;
    let slug_a = fx.club_a_slug.clone();

    let (st, hdrs, _) = send(&mut fx.router, "GET", "/club-screen.html", None, "").await;
    assert_eq!(st, StatusCode::FOUND);
    assert_eq!(
        hdrs.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/login?next=%2Fclub-screen.html")
    );

    let (st, hdrs, _) = send(&mut fx.router, "GET", "/club/screen", None, "").await;
    assert_eq!(st, StatusCode::FOUND);
    assert_eq!(
        hdrs.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some("/login?next=%2Fclub-screen.html")
    );

    let (st, hdrs, _) = send(
        &mut fx.router,
        "GET",
        &format!("/c/{slug_a}/screen"),
        None,
        "",
    )
    .await;
    assert_eq!(st, StatusCode::FOUND);
    assert_eq!(
        hdrs.get(header::LOCATION).and_then(|v| v.to_str().ok()),
        Some(format!("/login?next=%2Fc%2F{slug_a}%2Fscreen").as_str())
    );

    // Open redirect / CRLF / backslash in query must be rejected by safe_next.
    for bad_uri in [
        "/club-screen.html?x=%0d%0aSet-Cookie:evil=1",
        "/club-screen.html?x=\\evil.com",
        "/club/screen?x=https://evil.example",
        "//club-screen.html",
    ] {
        let (st, hdrs, _) = send(&mut fx.router, "GET", bad_uri, None, "").await;
        assert_eq!(st, StatusCode::FOUND, "GET {bad_uri}");
        let loc = hdrs
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(
            loc, "/login?next=%2Fclub-screen.html",
            "unsafe redirect not sanitized for {bad_uri}: {loc}"
        );
    }

    for api_uri in [
        "/api/display/map",
        "/api/display/base",
        "/api/display/kiosk-layout",
        "/api/display/kiosk-layout/stream",
        "/api/display/checkin-qr",
        "/api/club/screen/qr.svg",
    ] {
        let (st, _, _) = send(&mut fx.router, "GET", api_uri, None, "").await;
        assert_eq!(st, StatusCode::UNAUTHORIZED, "unauth GET {api_uri}");
    }
}

#[tokio::test]
#[serial]
async fn non_admin_pilot_and_staff_receive_403_and_no_kiosk_data() {
    let mut fx = setup_kiosk_fx().await;
    let slug_a = fx.club_a_slug.clone();

    for (label, cookie) in [
        ("pilot", fx.pilot_a_account_cookie.clone()),
        ("staff", fx.staff_a_account_cookie.clone()),
    ] {
        for page_uri in [
            "/club-screen.html".to_string(),
            "/club/screen".to_string(),
            format!("/c/{slug_a}/screen"),
        ] {
            let (st, _, body) = send(&mut fx.router, "GET", &page_uri, Some(&cookie), "").await;
            assert_eq!(st, StatusCode::FORBIDDEN, "{label} GET {page_uri}");
            assert!(!body.contains("kiosk-screen"));
        }

        for api_uri in [
            "/api/display/map",
            "/api/display/base",
            "/api/display/kiosk-layout",
            "/api/display/kiosk-layout/stream",
            "/api/display/checkin-qr",
            "/api/club/screen/qr.svg",
        ] {
            let (st, _, body) = send(&mut fx.router, "GET", api_uri, Some(&cookie), "").await;
            assert_eq!(st, StatusCode::FORBIDDEN, "{label} GET {api_uri}");
            assert!(!body.contains("<svg"), "{label} leaked svg on {api_uri}");
            assert!(
                !body.contains("Club Alfa") && !body.contains("SCAL"),
                "{label} leaked club data on {api_uri}: {body}"
            );
        }
    }
}

#[tokio::test]
#[serial]
async fn cross_club_admin_cannot_view_other_club_screen_or_data() {
    let mut fx = setup_kiosk_fx().await;
    let slug_a = fx.club_a_slug.clone();
    let slug_b = fx.club_b_slug.clone();
    let admin_b = fx.admin_b_account_cookie.clone();
    let admin_a = fx.admin_a_account_cookie.clone();

    // Seed an active club-visibility flight in Club A to verify Club B never sees Club A's aircraft.
    let now = now_ms();
    let type_id = fx.club.upsert_type(&fx.club_a_id, "C172").await.unwrap();
    let _ = type_id;
    let aircraft_id = fx
        .club
        .upsert_aircraft(&fx.club_a_id, "CC-ALFA", "C172", true)
        .await
        .unwrap();
    let pilot_a_id: String =
        sqlx::query_scalar("SELECT id FROM pilots WHERE email = 'pilot-a@test'")
            .fetch_one(&fx.club.pool)
            .await
            .unwrap();
    let flight_id = new_id();
    sqlx::query(
        "INSERT INTO flights (id, club_id, aircraft_id, pilot_id, started_at, last_fix_at, status, visibility, show_pilot_name_public)
         VALUES ($1, $2, $3, $4, $5, $5, 'active', 'club', FALSE)",
    )
    .bind(&flight_id)
    .bind(&fx.club_a_id)
    .bind(&aircraft_id)
    .bind(&pilot_a_id)
    .bind(now)
    .execute(&fx.club.pool)
    .await
    .unwrap();
    let device_id = new_id();
    sqlx::query(
        "INSERT INTO devices (id, pilot_id, token_hash, label, created_at)
         VALUES ($1, $2, $3, 'test-dev', $4)",
    )
    .bind(&device_id)
    .bind(&pilot_a_id)
    .bind(format!("tok-hash-{device_id}"))
    .bind(now)
    .execute(&fx.club.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO fixes (flight_id, device_id, t_ms, received_at, lat, lon, alt, speed, course, accuracy)
         VALUES ($1, $2, $3, $3, -33.45, -70.55, 1000.0, 40.0, 90.0, 5.0)",
    )
    .bind(&flight_id)
    .bind(&device_id)
    .bind(now)
    .execute(&fx.club.pool)
    .await
    .unwrap();

    // Admin B visiting /c/{club_a}/screen -> 403 Forbidden, no screen HTML.
    let (st, _, body) = send(
        &mut fx.router,
        "GET",
        &format!("/c/{slug_a}/screen"),
        Some(&admin_b),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert!(!body.contains("kiosk-screen"));
    assert!(!body.contains("Club Alfa"));

    // Admin B visiting /c/missing-club-xyz/screen -> 404 Not Found.
    let (st, _, _) = send(
        &mut fx.router,
        "GET",
        "/c/missing-club-xyz/screen",
        Some(&admin_b),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Admin B visiting own /c/{club_b}/screen -> 200 OK.
    let (st, _, _) = send(
        &mut fx.router,
        "GET",
        &format!("/c/{slug_b}/screen"),
        Some(&admin_b),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    // Admin B APIs return ONLY Club B data, never Club A's name/ICAO/aircraft.
    let (st, _, base_body) = send(
        &mut fx.router,
        "GET",
        "/api/display/base",
        Some(&admin_b),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(base_body.contains("Club Beta") && base_body.contains("SCBE"));
    assert!(!base_body.contains("Club Alfa") && !base_body.contains("SCAL"));

    let (st, _, map_body) = send(
        &mut fx.router,
        "GET",
        "/api/display/map",
        Some(&admin_b),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        !map_body.contains("CC-ALFA"),
        "Club B admin saw Club A aircraft: {map_body}"
    );

    // Sanity check: Admin A DOES see CC-ALFA.
    let (st, _, map_a) = send(
        &mut fx.router,
        "GET",
        "/api/display/map",
        Some(&admin_a),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(map_a.contains("CC-ALFA"), "Club A admin should see CC-ALFA");
}

#[tokio::test]
#[serial]
async fn old_display_token_endpoints_return_404() {
    let mut fx = setup_kiosk_fx().await;
    let admin_cookie = fx.admin_a_account_cookie.clone();
    // Ensure dist/index.html exists (SPA fallback must NOT swallow removed /api/* endpoints!).
    assert!(fx.dist_dir.join("index.html").is_file());

    let cases = [
        ("POST", "/api/display/activate", r#"{"token":"deadbeef"}"#),
        ("GET", "/api/admin/display-sessions", ""),
        (
            "POST",
            "/api/admin/display-sessions",
            r#"{"label":"Kiosk"}"#,
        ),
        ("POST", "/api/admin/display-sessions/abc123/revoke", ""),
    ];
    for (method, uri, body) in cases {
        let (st_anon, _, _) = send(&mut fx.router, method, uri, None, body).await;
        assert_eq!(st_anon, StatusCode::NOT_FOUND, "anon {method} {uri}");

        let (st_adm, _, _) = send(&mut fx.router, method, uri, Some(&admin_cookie), body).await;
        assert_eq!(st_adm, StatusCode::NOT_FOUND, "admin {method} {uri}");
    }
}

#[tokio::test]
#[serial]
async fn sliding_session_renewal_extends_expires_at_without_rotating_session_id_and_throttles() {
    let mut fx = setup_kiosk_fx().await;
    let acct_raw = fx.admin_a_account_sess_raw.clone();
    let acct_cookie = fx.admin_a_account_cookie.clone();
    let acct_db_id = digest_token(&acct_raw);

    // 1. Fresh session (< throttle window): no renewal write and no Set-Cookie.
    let (exp_initial, created_initial): (i64, i64) =
        sqlx::query_as("SELECT expires_at, created_at FROM account_sessions WHERE id = $1")
            .bind(&acct_db_id)
            .fetch_one(&fx.club.pool)
            .await
            .unwrap();
    let (st, hdrs, _) = send(
        &mut fx.router,
        "GET",
        "/api/display/map",
        Some(&acct_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        hdrs.get(header::SET_COOKIE).is_none(),
        "fresh session must not emit Set-Cookie within throttle window"
    );
    let exp_after_fresh: i64 =
        sqlx::query_scalar("SELECT expires_at FROM account_sessions WHERE id = $1")
            .bind(&acct_db_id)
            .fetch_one(&fx.club.pool)
            .await
            .unwrap();
    assert_eq!(exp_after_fresh, exp_initial);

    // 2. Backdate expires_at past the 5-minute throttle threshold (by 10 minutes) while still valid.
    let backdated_exp = now_ms() + (SESSION_TTL_SEC - SESSION_RENEW_THROTTLE_SEC - 300) * 1000;
    sqlx::query("UPDATE account_sessions SET expires_at = $1 WHERE id = $2")
        .bind(backdated_exp)
        .bind(&acct_db_id)
        .execute(&fx.club.pool)
        .await
        .unwrap();

    let (st, hdrs, _) = send(
        &mut fx.router,
        "GET",
        "/api/display/map",
        Some(&acct_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let set_cookie = hdrs
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("renewed session must re-issue Set-Cookie");
    assert!(
        set_cookie.starts_with(&format!("{ACCOUNT_COOKIE}={acct_raw};")),
        "sliding renewal must keep exact same session id: {set_cookie}"
    );
    assert!(
        set_cookie.contains(&format!("Max-Age={SESSION_TTL_SEC}")),
        "renewed Set-Cookie must refresh Max-Age: {set_cookie}"
    );
    assert!(set_cookie.contains("HttpOnly") && set_cookie.contains("SameSite=Lax"));

    let (exp_renewed, created_after): (i64, i64) =
        sqlx::query_as("SELECT expires_at, created_at FROM account_sessions WHERE id = $1")
            .bind(&acct_db_id)
            .fetch_one(&fx.club.pool)
            .await
            .unwrap();
    assert!(
        exp_renewed > backdated_exp + 200_000,
        "expires_at should be extended toward now + 12h"
    );
    assert_eq!(
        created_after, created_initial,
        "sliding renewal must never modify created_at"
    );

    // 3. Immediate follow-up request with the SAME cookie succeeds and is throttled (no Set-Cookie).
    let (st2, hdrs2, _) = send(
        &mut fx.router,
        "GET",
        "/api/display/base",
        Some(&acct_cookie),
        "",
    )
    .await;
    assert_eq!(st2, StatusCode::OK);
    assert!(
        hdrs2.get(header::SET_COOKIE).is_none(),
        "second immediate request must be throttled"
    );

    // 4. Same sliding renewal works for fv_club_session too, without rotating session id.
    let club_raw = fx.admin_a_club_sess_raw.clone();
    let club_cookie = fx.admin_a_club_cookie.clone();
    let club_db_id = digest_token(&club_raw);
    sqlx::query("UPDATE pilot_sessions SET expires_at = $1 WHERE id = $2")
        .bind(backdated_exp)
        .bind(&club_db_id)
        .execute(&fx.club.pool)
        .await
        .unwrap();

    let (st3, hdrs3, _) = send(
        &mut fx.router,
        "GET",
        "/api/display/map",
        Some(&club_cookie),
        "",
    )
    .await;
    assert_eq!(st3, StatusCode::OK);
    let club_set_cookie = hdrs3
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("renewed club session must emit Set-Cookie");
    assert!(
        club_set_cookie.starts_with(&format!("{COOKIE}={club_raw};")),
        "club session renewal must keep same session id: {club_set_cookie}"
    );
    let club_exp_renewed: i64 =
        sqlx::query_scalar("SELECT expires_at FROM pilot_sessions WHERE id = $1")
            .bind(&club_db_id)
            .fetch_one(&fx.club.pool)
            .await
            .unwrap();
    assert!(club_exp_renewed > backdated_exp + 200_000);
}

#[tokio::test]
#[serial]
async fn migration_000028_drops_display_sessions_on_fresh_and_populated_db() {
    let db = ClubDb::open_test().await.unwrap();

    // 1. Fresh DB after sqlx migrations must NOT have display_sessions table.
    let exists_after_fresh: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM information_schema.tables
            WHERE table_schema = 'public' AND table_name = 'display_sessions'
         )",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(
        !exists_after_fresh,
        "display_sessions must be dropped by migration 000028"
    );

    // 2. Simulate an existing DB that still had display_sessions with rows, then run 000028 SQL.
    let club_id = db.default_club_id().await.unwrap();
    let pilot_id = new_id();
    let now = now_ms();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Mig Admin', 'mig-admin@test', '', 'approved', 'admin', '', $3)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query(
        "CREATE TABLE display_sessions (
            id TEXT PRIMARY KEY NOT NULL,
            club_id TEXT NOT NULL REFERENCES clubs(id) ON DELETE CASCADE,
            created_by_pilot_id TEXT NOT NULL REFERENCES pilots(id) ON DELETE CASCADE,
            token_hash TEXT NOT NULL UNIQUE,
            label TEXT NOT NULL DEFAULT '',
            created_at BIGINT NOT NULL,
            revoked_at BIGINT,
            expires_at BIGINT
        )",
    )
    .execute(&db.pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO display_sessions (id, club_id, created_by_pilot_id, token_hash, label, created_at, expires_at)
         VALUES ('legacy-1', $1, $2, 'deadbeefhash', 'Old Kiosk', $3, $4)",
    )
    .bind(&club_id)
    .bind(&pilot_id)
    .bind(now)
    .bind(now + 1000)
    .execute(&db.pool)
    .await
    .unwrap();

    let migration_sql = include_str!("../../migrations/000028_drop_display_sessions.sql");
    sqlx::query(migration_sql).execute(&db.pool).await.unwrap();
    // Idempotent if re-executed.
    sqlx::query(migration_sql).execute(&db.pool).await.unwrap();

    let exists_after_drop: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM information_schema.tables
            WHERE table_schema = 'public' AND table_name = 'display_sessions'
         )",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert!(!exists_after_drop);
}
