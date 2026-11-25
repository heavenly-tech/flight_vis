//! HTTP-level visibility tests.
use crate::{
    auth::Auth,
    build_router,
    club::{db::now_ms, visibility::FlightVisibility, ClubDb},
    device_auth::DeviceAuth,
    live::LiveEngine,
    store::Store,
    AppState,
};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use serde::Deserialize;
use serial_test::serial;
use std::sync::Arc;
use tower::Service;

pub(crate) struct Fixture {
    _dir: tempfile::TempDir,
    dist_dir: std::path::PathBuf,
    pub(crate) router: axum::Router,
    pub(crate) club: ClubDb,
    mailer: crate::club::mail::Mailer,
    pilot_a: String,
    _pilot_b: String,
    device_token: String,
}

pub(crate) async fn setup() -> Fixture {
    setup_with_superadmins(&[]).await
}

pub(crate) async fn setup_with_superadmins(emails: &[&str]) -> Fixture {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let club = ClubDb::open_test().await.expect("club db");
    let club_id = club.default_club_id().await.unwrap();
    let now = now_ms();
    let _ = sqlx::query(
        "UPDATE clubs SET email_verified_at = COALESCE(email_verified_at, $1), status = 'live' WHERE id = $2",
    )
    .bind(now)
    .bind(&club_id)
    .execute(&club.pool)
    .await;
    club.upsert_type(&club_id, "C152").await.unwrap();
    club.upsert_aircraft(&club_id, "N1", "C152", true)
        .await
        .unwrap();
    let pilot_a = club
        .register(&club_id, "A", "a@test", "", "password12")
        .await
        .unwrap();
    let pilot_b = club
        .register(&club_id, "B", "b@test", "", "password12")
        .await
        .unwrap();
    let now = now_ms();
    club.mark_email_verified(&pilot_a, now).await.unwrap();
    club.mark_email_verified(&pilot_b, now).await.unwrap();
    club.approve_pilot(&pilot_a).await.unwrap();
    club.approve_pilot(&pilot_b).await.unwrap();
    club.add_checkout(&pilot_a, "C152", None, None)
        .await
        .unwrap();
    club.add_checkout(&pilot_b, "C152", None, None)
        .await
        .unwrap();
    let (_dev, device_token) = club.create_device(&pilot_a, "phone").await.unwrap();
    let live = Arc::new(LiveEngine::new(data_dir.path()));
    let (video_queue, _) = tokio::sync::mpsc::channel(4);
    let dist_dir = data_dir.path().join("dist");
    let mailer = crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default());
    let state = AppState {
        dist_dir: dist_dir.clone(),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live,
        store: Store::new(data_dir.path()),
        club: Some(club.clone()),
        video_queue,
        login_throttle: crate::LoginThrottle::default(),
        device_register_throttle: crate::LoginThrottle::default(),
        share_lookup_throttle: crate::LoginThrottle::default(),
        account_password_throttle: crate::LoginThrottle::default(),
        account_password_account_throttle: crate::LoginThrottle::default(),
        mailer: mailer.clone(),
        webauthn: None,
        sse_limiter: std::sync::Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails: emails.iter().map(|e| e.to_string()).collect(),
    };
    Fixture {
        _dir: data_dir,
        dist_dir,
        router: build_router(state),
        club,
        mailer,
        pilot_a,
        _pilot_b: pilot_b,
        device_token,
    }
}

async fn login_cookie(router: &mut axum::Router, email: &str) -> String {
    let res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/club/login")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(format!(
                    r#"{{"email":"{email}","password":"password12"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    res.headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .expect("set-cookie")
}

async fn start_flight(
    router: &mut axum::Router,
    club: &ClubDb,
    cookie: &str,
    token: &str,
    visibility: &str,
) -> String {
    let club_id = club.default_club_id().await.expect("club");
    let qr = club
        .mint_checkin_qr_token(&club_id, now_ms())
        .await
        .expect("qr");
    let checkin = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/checkin")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::COOKIE, cookie)
                .body(Body::from(format!(r#"{{"token":"{qr}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(checkin.status(), StatusCode::OK);
    let res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/flights")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::COOKIE, cookie)
                .body(Body::from(format!(
                    r#"{{"tail":"N1","visibility":"{visibility}"}}"#
                )))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice::<serde_json::Value>(&body)
        .unwrap()
        .get("id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

#[tokio::test]
#[serial]
async fn fixes_denied_after_mid_flight_private() {
    let mut fx = setup().await;
    let cookie_a = login_cookie(&mut fx.router, "a@test").await;
    let cookie_b = login_cookie(&mut fx.router, "b@test").await;
    let flight_id = start_flight(
        &mut fx.router,
        &fx.club,
        &cookie_a,
        &fx.device_token,
        "club",
    )
    .await;

    let visible = fx
        .router
        .call(
            Request::builder()
                .uri(format!("/api/flights/{flight_id}/fixes"))
                .header(header::COOKIE, cookie_b.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(visible.status(), StatusCode::OK);

    fx.club
        .patch_flight(
            &fx.pilot_a,
            &flight_id,
            Some(FlightVisibility::Private),
            None,
        )
        .await
        .unwrap();

    let hidden = fx
        .router
        .call(
            Request::builder()
                .uri(format!("/api/flights/{flight_id}/fixes"))
                .header(header::COOKIE, cookie_b)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[serial]
async fn anonymous_public_map_and_name_flag() {
    let mut fx = setup().await;
    let cookie_a = login_cookie(&mut fx.router, "a@test").await;
    let flight_id = start_flight(
        &mut fx.router,
        &fx.club,
        &cookie_a,
        &fx.device_token,
        "club",
    )
    .await;
    let _ = flight_id;
    let q = format!(
        "id={}&lat=-33.4&lon=-70.6&timestamp={}",
        fx.device_token,
        now_ms() / 1000
    );
    fx.router
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/api/osmand?{q}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let empty = fx
        .router
        .call(
            Request::builder()
                .uri("/api/club/map/public")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = to_bytes(empty.into_body(), usize::MAX).await.unwrap();
    let n = serde_json::from_slice::<serde_json::Value>(&bytes)
        .unwrap()
        .get("aircraft")
        .unwrap()
        .as_array()
        .unwrap()
        .len();
    assert_eq!(n, 0);

    fx.club
        .patch_flight(
            &fx.pilot_a,
            &flight_id,
            Some(FlightVisibility::Public),
            Some(true),
        )
        .await
        .unwrap();

    let res = fx
        .router
        .call(
            Request::builder()
                .uri("/api/club/map/public")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let row = serde_json::from_slice::<serde_json::Value>(&body)
        .unwrap()
        .get("aircraft")
        .unwrap()
        .as_array()
        .unwrap()
        .first()
        .cloned()
        .expect("row");
    assert!(row.get("pilotName").and_then(|v| v.as_str()).is_some());
}

#[tokio::test]
#[serial]
async fn server_qr_svg_endpoints() {
    let mut fx = setup().await;
    let cookie_a = login_cookie(&mut fx.router, "a@test").await;
    let (device_id, _) = fx
        .club
        .create_device(&fx.pilot_a, "qr-phone")
        .await
        .unwrap();

    let unauth = fx
        .router
        .call(
            Request::builder()
                .uri(format!("/api/devices/pair/{device_id}/qr.svg"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    let pair = fx
        .router
        .call(
            Request::builder()
                .uri(format!("/api/devices/pair/{device_id}/qr.svg"))
                .header(header::COOKIE, cookie_a.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pair.status(), StatusCode::OK);
    assert_eq!(
        pair.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("image/svg+xml; charset=utf-8")
    );
    assert_eq!(
        pair.headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let svg = String::from_utf8(
        to_bytes(pair.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(svg.contains("<svg"));
    assert!(!svg.contains("qrserver"));

    let (_sess, display_token) = fx
        .club
        .create_display_session(&fx.pilot_a, "kiosk")
        .await
        .unwrap();
    let display_cookie = format!("{}={}", crate::club::auth::DISPLAY_COOKIE, display_token);

    let screen_qr = fx
        .router
        .call(
            Request::builder()
                .uri("/api/club/screen/qr.svg")
                .header(header::COOKIE, display_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(screen_qr.status(), StatusCode::OK);
    let screen_svg = String::from_utf8(
        to_bytes(screen_qr.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(screen_svg.contains("<svg"));
    assert!(!screen_svg.contains("qrserver"));
    assert!(!screen_svg.to_lowercase().contains("intent:"));
}

#[tokio::test]
#[serial]
#[allow(clippy::await_holding_lock)]
async fn club_screen_qr_encodes_https_app_link_not_intent() {
    let _guard = crate::club::android_assetlinks::android_test_env_lock();
    std::env::set_var("FLIGHT_ANDROID_APP_PACKAGE", "cl.heavenly.flightvis");
    std::env::set_var("FLIGHT_ANDROID_CERT_SHA256", TEST_ANDROID_FP);
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    sqlx::query("UPDATE clubs SET slug = 'demo' WHERE id = $1")
        .bind(&club_id)
        .execute(&fx.club.pool)
        .await
        .unwrap();
    let (_sess, display_token) = fx
        .club
        .create_display_session(&fx.pilot_a, "kiosk")
        .await
        .unwrap();
    let display_cookie = format!("{}={}", crate::club::auth::DISPLAY_COOKIE, display_token);
    let res = fx
        .router
        .call(
            Request::builder()
                .uri("/api/club/screen/qr.svg")
                .header(header::HOST, "flight.heavenly.cl")
                .header(header::COOKIE, display_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let svg = String::from_utf8(
        to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!svg.to_lowercase().contains("intent:"));
    std::env::remove_var("FLIGHT_ANDROID_APP_PACKAGE");
    std::env::remove_var("FLIGHT_ANDROID_CERT_SHA256");
}

#[tokio::test]
#[serial]
async fn api_me_devices_auto_register_idempotent_and_revoke_on_logout() {
    let mut fx = setup().await;
    let cookie = login_cookie(&mut fx.router, "a@test").await;
    let reg = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/me/devices")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie.clone())
                .body(Body::from(
                    r#"{"label":"Pixel","model":"Google Pixel","installId":"install-api-test-01"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reg.status(), StatusCode::OK);
    let body = to_bytes(reg.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.get("created").and_then(|v| v.as_bool()), Some(true));
    let device_id = json.get("id").and_then(|v| v.as_str()).expect("id");
    let token = json.get("token").and_then(|v| v.as_str()).expect("token");

    let flight_id = start_flight(
        &mut fx.router,
        &fx.club,
        &cookie,
        token,
        FlightVisibility::Club.as_str(),
    )
    .await;
    assert!(!flight_id.is_empty());

    let reg2 = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/me/devices")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie.clone())
                .body(Body::from(
                    r#"{"label":"Pixel","installId":"install-api-test-01"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reg2.status(), StatusCode::OK);
    let body2 = to_bytes(reg2.into_body(), usize::MAX).await.unwrap();
    let json2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
    assert_eq!(json2.get("created").and_then(|v| v.as_bool()), Some(false));
    assert_eq!(json2.get("id").and_then(|v| v.as_str()), Some(device_id));
    let token2 = json2.get("token").and_then(|v| v.as_str()).expect("token2");
    assert_ne!(token, token2);

    let del = fx
        .router
        .call(
            Request::builder()
                .method("DELETE")
                .uri("/api/me/devices/install-api-test-01")
                .header(header::COOKIE, cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::OK);
    assert!(fx.club.resolve_device_token(token2).await.is_none());

    let reg3 = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/me/devices")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie.clone())
                .body(Body::from(
                    r#"{"label":"Pixel","installId":"install-api-test-01"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reg3.status(), StatusCode::OK);
    let body3 = to_bytes(reg3.into_body(), usize::MAX).await.unwrap();
    let json3: serde_json::Value = serde_json::from_slice(&body3).unwrap();
    let token3 = json3.get("token").and_then(|v| v.as_str()).expect("token3");

    let logout = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/club/logout")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie)
                .body(Body::from(r#"{"installId":"install-api-test-01"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::OK);
    assert!(fx.club.resolve_device_token(token3).await.is_none());
}

#[tokio::test]
#[serial]
async fn pending_pilot_cannot_load_club_map() {
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    let pending = fx
        .club
        .register(&club_id, "P", "p@test", "", "password12")
        .await
        .unwrap();
    let now = now_ms();
    let session = fx.club.create_pilot_session(&pending, now).await.unwrap();
    let cookie = format!("{}={session}", crate::club::auth::COOKIE);
    let res = fx
        .router
        .call(
            Request::builder()
                .uri("/api/club/map")
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[serial]
async fn kiosk_layout_admin_clamps_and_display_sse_rev() {
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    fx.club.ensure_kiosk_qr_defaults(&club_id).await.unwrap();
    sqlx::query("UPDATE pilots SET role = 'admin' WHERE id = $1")
        .bind(&fx.pilot_a)
        .execute(&fx.club.pool)
        .await
        .unwrap();
    fx.club
        .ensure_membership_row(&fx.pilot_a, &club_id, "admin", "approved")
        .await
        .unwrap();
    let cookie = login_cookie(&mut fx.router, "a@test").await;
    let patch = fx
        .router
        .call(
            Request::builder()
                .method("PATCH")
                .uri("/api/admin/kiosk-layout")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie.clone())
                .body(Body::from(
                    r#"{"x":0.99,"y":0.99,"size":0.9,"aspect":1.7777777778}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(patch.status(), StatusCode::OK);
    let body = to_bytes(patch.into_body(), usize::MAX).await.unwrap();
    let layout: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let size = layout.get("size").and_then(|v| v.as_f64()).unwrap();
    assert!(size <= 0.5 + 1e-9);
    let rev1 = layout.get("rev").and_then(|v| v.as_i64()).unwrap();

    let (session_id, display_token) = fx
        .club
        .create_display_session(&fx.pilot_a, "k")
        .await
        .unwrap();
    let _ = session_id;
    let display_cookie = format!(
        "{}={display_token}; Path=/",
        crate::club::auth::DISPLAY_COOKIE
    );
    let disp = fx
        .router
        .call(
            Request::builder()
                .uri("/api/display/kiosk-layout")
                .header(header::COOKIE, display_cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disp.status(), StatusCode::OK);
    let disp_body = to_bytes(disp.into_body(), usize::MAX).await.unwrap();
    let disp_layout: serde_json::Value = serde_json::from_slice(&disp_body).unwrap();
    assert_eq!(disp_layout.get("rev").and_then(|v| v.as_i64()), Some(rev1));
}

async fn seed_second_club(db: &ClubDb, slug: &str) -> String {
    let id = crate::club::auth::new_id();
    let now = now_ms();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at, latitude, longitude, show_public_tracks)
         VALUES ($1, $2, $3, $4, 'live', $4, -33.0, -70.0, TRUE)",
    )
    .bind(&id)
    .bind("Club B")
    .bind(slug)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    id
}

#[tokio::test]
#[serial]
async fn cross_club_admin_cannot_approve_other_club_pilot() {
    let mut fx = setup().await;
    let club_a = fx.club.default_club_id().await.unwrap();
    let slug = format!("club-b-{}", &crate::club::auth::new_id()[..8]);
    let club_b = seed_second_club(&fx.club, &slug).await;
    let pending_b = fx
        .club
        .register(&club_b, "Pending B", "pending-b@test", "", "password12")
        .await
        .unwrap();
    fx.club
        .mark_email_verified(&pending_b, now_ms())
        .await
        .unwrap();
    sqlx::query("UPDATE pilots SET role = 'admin' WHERE id = $1")
        .bind(&fx.pilot_a)
        .execute(&fx.club.pool)
        .await
        .unwrap();
    fx.club
        .ensure_membership_row(&fx.pilot_a, &club_a, "admin", "active")
        .await
        .unwrap();
    let cookie_a = login_cookie(&mut fx.router, "a@test").await;
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/api/admin/pilots/{pending_b}/approve"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, cookie_a)
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial]
async fn cross_club_admin_pending_list_is_scoped() {
    let mut fx = setup().await;
    let club_a = fx.club.default_club_id().await.unwrap();
    let slug = format!("club-b-{}", &crate::club::auth::new_id()[..8]);
    let club_b = seed_second_club(&fx.club, &slug).await;
    let pending_b = fx
        .club
        .register(&club_b, "Pending B", "scoped-b@test", "", "password12")
        .await
        .unwrap();
    sqlx::query("UPDATE pilots SET role = 'admin' WHERE id = $1")
        .bind(&fx.pilot_a)
        .execute(&fx.club.pool)
        .await
        .unwrap();
    fx.club
        .ensure_membership_row(&fx.pilot_a, &club_a, "admin", "approved")
        .await
        .unwrap();
    let cookie_a = login_cookie(&mut fx.router, "a@test").await;
    let res = fx
        .router
        .call(
            Request::builder()
                .uri("/api/admin/pilots/pending")
                .header(header::COOKIE, cookie_a)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let ids: Vec<&str> = json["pilots"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p.get("id").and_then(|v| v.as_str()))
        .collect();
    assert!(!ids.contains(&pending_b.as_str()));
}

#[tokio::test]
#[serial]
async fn cross_club_public_map_isolated_by_slug_query() {
    let mut fx = setup().await;
    let slug_a: String =
        sqlx::query_scalar("SELECT slug FROM clubs ORDER BY created_at ASC LIMIT 1")
            .fetch_one(&fx.club.pool)
            .await
            .unwrap();
    let slug_b = format!("club-b-{}", &crate::club::auth::new_id()[..8]);
    let club_b = seed_second_club(&fx.club, &slug_b).await;
    fx.club.upsert_type(&club_b, "C152").await.unwrap();
    fx.club
        .upsert_aircraft(&club_b, "NX-B", "C152", true)
        .await
        .unwrap();
    let pilot_b = fx
        .club
        .register(&club_b, "B2", "b2-map@test", "", "password12")
        .await
        .unwrap();
    fx.club
        .mark_email_verified(&pilot_b, now_ms())
        .await
        .unwrap();
    fx.club.approve_pilot(&pilot_b).await.unwrap();
    fx.club
        .add_checkout(&pilot_b, "C152", None, None)
        .await
        .unwrap();
    let now = now_ms();
    let dev_b = fx.club.create_device(&pilot_b, "b2").await.unwrap().0;
    fx.club
        .issue_open_checkin(&pilot_b, &dev_b, &club_b, now)
        .await
        .unwrap();
    let flight_b = fx
        .club
        .start_flight(
            &pilot_b,
            &dev_b,
            "NX-B",
            FlightVisibility::Public,
            false,
            now,
        )
        .await
        .unwrap();
    use crate::live::LiveFix;
    fx.club
        .append_fix(
            &flight_b,
            &dev_b,
            &LiveFix {
                t: now,
                lat: -33.5,
                lon: -70.7,
                alt: Some(1000.0),
                alt_ref: None,
                speed: None,
                course: None,
                accuracy: Some(5.0),
            },
        )
        .await
        .unwrap();

    let map_a = fx
        .router
        .call(
            Request::builder()
                .uri(format!("/api/club/map/public?club={slug_a}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body_a = to_bytes(map_a.into_body(), usize::MAX).await.unwrap();
    let json_a: serde_json::Value = serde_json::from_slice(&body_a).unwrap();
    let tails_a: Vec<String> = json_a["aircraft"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.get("tail").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    assert!(!tails_a.iter().any(|t| t == "NX-B"));

    let map_b = fx
        .router
        .call(
            Request::builder()
                .uri(format!("/api/club/map/public?club={slug_b}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body_b = to_bytes(map_b.into_body(), usize::MAX).await.unwrap();
    let json_b: serde_json::Value = serde_json::from_slice(&body_b).unwrap();
    let tails_b: Vec<String> = json_b["aircraft"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r.get("tail").and_then(|v| v.as_str()).map(str::to_string))
        .collect();
    assert!(tails_b.iter().any(|t| t == "NX-B"));
}

#[tokio::test]
#[serial]
async fn cross_club_me_tails_only_home_club_aircraft() {
    let fx = setup().await;
    let club_a = fx.club.default_club_id().await.unwrap();
    let slug_b = format!("tails-b-{}", &crate::club::auth::new_id()[..8]);
    let club_b = seed_second_club(&fx.club, &slug_b).await;
    fx.club.upsert_type(&club_a, "C152").await.unwrap();
    fx.club.upsert_type(&club_b, "C152").await.unwrap();
    fx.club
        .upsert_aircraft(&club_a, "N-A", "C152", true)
        .await
        .unwrap();
    fx.club
        .upsert_aircraft(&club_b, "N-B", "C152", true)
        .await
        .unwrap();
    fx.club
        .add_checkout(&fx.pilot_a, "C152", None, None)
        .await
        .unwrap();
    let tails = fx.club.me_tails(&fx.pilot_a, now_ms()).await.unwrap();
    let t: Vec<&str> = tails.iter().map(|x| x.tail.as_str()).collect();
    assert!(t.contains(&"N-A"));
    assert!(!t.contains(&"N-B"));
}

#[derive(Deserialize)]
struct ContractCase {
    method: String,
    #[serde(default)]
    path: String,
    #[serde(rename = "pathTemplate", default)]
    path_template: String,
    #[serde(default)]
    body: Option<serde_json::Value>,
    #[serde(rename = "expectStatus")]
    expect_status: u16,
    #[serde(rename = "expectBodyKeys", default)]
    expect_body_keys: Vec<String>,
}

#[tokio::test]
#[serial]
async fn contract_client_api_aliases() {
    let mut fx = setup().await;
    let slug: String = sqlx::query_scalar("SELECT slug FROM clubs ORDER BY created_at ASC LIMIT 1")
        .fetch_one(&fx.club.pool)
        .await
        .unwrap();
    let magic_case = include_str!("../../tests/contract/magic_send.json");
    let club_case = include_str!("../../tests/contract/club_by_slug.json");
    let anon_case = include_str!("../../tests/contract/devices_anonymous.json");
    for raw in [magic_case, club_case, anon_case] {
        let case: ContractCase = serde_json::from_str(raw).expect("contract json");
        let path = if case.path_template.is_empty() {
            case.path
        } else {
            case.path_template.replace("{slug}", &slug)
        };
        let mut req = Request::builder().method(case.method.as_str()).uri(path);
        let res = if let Some(body) = case.body {
            req = req.header(header::CONTENT_TYPE, "application/json");
            fx.router
                .call(req.body(Body::from(body.to_string())).unwrap())
                .await
                .unwrap()
        } else {
            fx.router
                .call(req.body(Body::empty()).unwrap())
                .await
                .unwrap()
        };
        assert_eq!(
            res.status(),
            StatusCode::from_u16(case.expect_status).unwrap()
        );
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for key in case.expect_body_keys {
            assert!(json.get(&key).is_some(), "missing key {key}");
        }
    }
}

#[tokio::test]
#[serial]
async fn postal_bounce_webhook_requires_and_verifies_rsa256() {
    use base64::Engine;
    use rsa::pkcs1v15::SigningKey;
    use rsa::pkcs8::EncodePublicKey;
    use rsa::signature::{SignatureEncoding, Signer};
    use rsa::RsaPrivateKey;
    use sha2::Sha256;
    let mut rng = rand::thread_rng();
    let private = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    let pem = private
        .to_public_key()
        .to_public_key_pem(rsa::pkcs8::LineEnding::LF)
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let key_path = dir.path().join("postal-pub.pem");
    std::fs::write(&key_path, &pem).unwrap();
    std::env::set_var("POSTAL_WEBHOOK_PUBLIC_KEY_FILE", key_path);
    let mut fx = setup().await;
    let body = br#"{"to":"bounced@test"}"#;
    let signing = SigningKey::<Sha256>::new(private);
    let sig = base64::engine::general_purpose::STANDARD.encode(signing.sign(body).to_bytes());
    let missing = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/webhooks/postal/bounce")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
    let bad = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/webhooks/postal/bounce")
                .header("X-Postal-Signature-256", "not-valid")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
    let ok = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/webhooks/postal/bounce")
                .header("X-Postal-Signature-256", &sig)
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_vec()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    std::env::remove_var("POSTAL_WEBHOOK_PUBLIC_KEY_FILE");
}

#[tokio::test]
#[serial]
async fn account_signup_same_response_for_duplicate_email() {
    let mut fx = setup().await;
    let body = r#"{"email":"dup-signup@test","password":"password12"}"#;
    for _ in 0..2 {
        let res = fx
            .router
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/api/account/signup")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::CREATED);
        let parsed: serde_json::Value =
            serde_json::from_slice(&to_bytes(res.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(parsed.get("ok"), Some(&serde_json::json!(true)));
        assert!(parsed.get("accountId").is_none());
    }
}

#[tokio::test]
#[serial]
async fn account_approve_membership_cross_club_forbidden() {
    let mut fx = setup().await;
    let club_a = fx.club.default_club_id().await.unwrap();
    let slug = format!("club-b-{}", &crate::club::auth::new_id()[..8]);
    let club_b = seed_second_club(&fx.club, &slug).await;
    let pending_b = fx
        .club
        .register(&club_b, "Joiner", "joiner-b@test", "", "password12")
        .await
        .unwrap();
    let membership_id: String =
        sqlx::query_scalar("SELECT id FROM club_memberships WHERE pilot_id = $1 AND club_id = $2")
            .bind(&pending_b)
            .bind(&club_b)
            .fetch_one(&fx.club.pool)
            .await
            .unwrap();
    sqlx::query("UPDATE pilots SET role = 'admin' WHERE id = $1")
        .bind(&fx.pilot_a)
        .execute(&fx.club.pool)
        .await
        .unwrap();
    fx.club
        .ensure_membership_row(&fx.pilot_a, &club_a, "admin", "active")
        .await
        .unwrap();
    let cookie_a = login_cookie(&mut fx.router, "a@test").await;
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/api/account/memberships/{membership_id}/approve"))
                .header(header::COOKIE, cookie_a)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial]
async fn flight_sse_closes_after_membership_suspended() {
    use http_body_util::BodyExt;
    use std::time::Duration;

    let mut fx = setup().await;
    let cookie_a = login_cookie(&mut fx.router, "a@test").await;
    let flight_id = start_flight(
        &mut fx.router,
        &fx.club,
        &cookie_a,
        &fx.device_token,
        "club",
    )
    .await;
    let club_id = fx.club.default_club_id().await.unwrap();
    let mid: String =
        sqlx::query_scalar("SELECT id FROM club_memberships WHERE pilot_id = $1 AND club_id = $2")
            .bind(&fx.pilot_a)
            .bind(&club_id)
            .fetch_one(&fx.club.pool)
            .await
            .unwrap();
    let club_db = fx.club.clone();
    let mid_spawn = mid.clone();
    let suspend = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        let _ = club_db.suspend_membership(&mid_spawn).await;
    });
    let stream_resp = fx
        .router
        .call(
            Request::builder()
                .method("GET")
                .uri(format!("/api/flights/{flight_id}/stream"))
                .header(header::COOKIE, &cookie_a)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stream_resp.status(), StatusCode::OK);
    let collected = tokio::time::timeout(Duration::from_secs(25), async {
        stream_resp.into_body().collect().await
    })
    .await
    .expect("sse should end within 25s after suspend")
    .expect("body collect")
    .to_bytes();
    let _ = suspend.await;
    assert!(!collected.is_empty());
}

#[tokio::test]
#[serial]
async fn flight_sse_closes_after_membership_removed() {
    use http_body_util::BodyExt;
    use std::time::Duration;

    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    let admin_b = fx
        .club
        .register(&club_id, "Admin2", "admin2@test", "", "password12")
        .await
        .unwrap();
    let now = now_ms();
    fx.club.mark_email_verified(&admin_b, now).await.unwrap();
    fx.club.approve_pilot(&admin_b).await.unwrap();
    fx.club
        .ensure_membership_row(&admin_b, &club_id, "admin", "active")
        .await
        .unwrap();
    let cookie_a = login_cookie(&mut fx.router, "a@test").await;
    let flight_id = start_flight(
        &mut fx.router,
        &fx.club,
        &cookie_a,
        &fx.device_token,
        "club",
    )
    .await;
    let mid_a: String =
        sqlx::query_scalar("SELECT id FROM club_memberships WHERE pilot_id = $1 AND club_id = $2")
            .bind(&fx.pilot_a)
            .bind(&club_id)
            .fetch_one(&fx.club.pool)
            .await
            .unwrap();
    let club_db = fx.club.clone();
    let mid_spawn = mid_a.clone();
    let remove = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        let _ = club_db.remove_admin_membership(&club_id, &mid_spawn).await;
    });
    let stream_resp = fx
        .router
        .call(
            Request::builder()
                .method("GET")
                .uri(format!("/api/flights/{flight_id}/stream"))
                .header(header::COOKIE, &cookie_a)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stream_resp.status(), StatusCode::OK);
    let collected = tokio::time::timeout(Duration::from_secs(25), async {
        stream_resp.into_body().collect().await
    })
    .await
    .expect("sse should end within 25s after removal")
    .expect("body collect")
    .to_bytes();
    let _ = remove.await;
    assert!(!collected.is_empty());
}

#[tokio::test]
#[serial]
async fn club_login_html_redirects_to_unified_login() {
    let mut fx = setup().await;
    let res = fx
        .router
        .call(
            Request::builder()
                .uri("/club-login.html?next=%2Fclub-pilot.html&e=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FOUND);
    let location = res
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(location.starts_with("/login?"));
    assert!(location.contains("next=%2Fclub-pilot.html"));
    assert!(location.contains("e=1"));
    assert!(!location.to_lowercase().contains("password"));
}

fn ensure_test_i18n() {
    let locales = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../locales");
    let _ = crate::i18n::init_global(&locales);
}

#[tokio::test]
#[serial]
async fn patch_me_locale_requires_session() {
    ensure_test_i18n();
    let mut fx = setup().await;
    let res = fx
        .router
        .call(
            Request::builder()
                .method("PATCH")
                .uri("/api/club/me/locale")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"locale":"en"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[serial]
async fn patch_me_locale_rejects_invalid() {
    ensure_test_i18n();
    let mut fx = setup().await;
    let cookie = login_cookie(&mut fx.router, "a@test").await;
    let res = fx
        .router
        .call(
            Request::builder()
                .method("PATCH")
                .uri("/api/club/me/locale")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"locale":"fr"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
#[serial]
async fn patch_me_locale_persists_for_pilot() {
    ensure_test_i18n();
    let mut fx = setup().await;
    let cookie = login_cookie(&mut fx.router, "a@test").await;
    let ok = fx
        .router
        .call(
            Request::builder()
                .method("PATCH")
                .uri("/api/club/me/locale")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(r#"{"locale":"en"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let ctx = fx
        .router
        .call(
            Request::builder()
                .uri("/api/club/i18n/context")
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ctx.status(), StatusCode::OK);
    let body = to_bytes(ctx.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.get("userLocale").and_then(|v| v.as_str()), Some("en"));
}

#[tokio::test]
#[serial]
async fn club_login_form_post_redirects_without_password_in_location() {
    let mut fx = setup().await;
    let body = "email=a%40test&password=password12&next=%2Fclub-pilot.html";
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/club/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let location = res
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(location, "/club-pilot.html");
    assert!(!location.to_lowercase().contains("password"));
    assert!(res.headers().get(header::SET_COOKIE).is_some());
}

#[tokio::test]
#[serial]
async fn club_login_form_post_rejects_open_redirect_next() {
    let mut fx = setup().await;
    let body = "email=a%40test&password=password12&next=%2F%5Cevil.com";
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/club/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let location = res
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(location, "/");
}

#[tokio::test]
#[serial]
async fn club_login_form_post_cross_site_is_forbidden() {
    let mut fx = setup().await;
    let body = "email=a%40test&password=password12&next=%2Fclub-pilot.html";
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/club/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "cross-site")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial]
async fn club_login_form_post_bad_password_preserves_safe_next() {
    let mut fx = setup().await;
    let body = "email=a%40test&password=wrong&next=%2Fclub-admin-types.html";
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/club/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let location = res
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(location.starts_with("/login?"));
    assert!(location.contains("e=1"));
    assert!(location.contains("mode=password"));
    assert!(location.contains("next=%2Fclub-admin-types.html"));
    assert!(location.contains("mode=password"));
    assert!(!location.to_lowercase().contains("password="));
}

#[tokio::test]
#[serial]
async fn patch_me_locale_cross_site_is_forbidden() {
    let mut fx = setup().await;
    let cookie = login_cookie(&mut fx.router, "a@test").await;
    let res = fx
        .router
        .call(
            Request::builder()
                .method("PATCH")
                .uri("/api/club/me/locale")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .header("sec-fetch-site", "cross-site")
                .body(Body::from(r#"{"locale":"en"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json.get("error").and_then(|v| v.as_str()), Some("csrf"));
}

async fn make_admin(fx: &mut Fixture, pilot_id: &str, club_id: &str, cookie_email: &str) -> String {
    sqlx::query("UPDATE pilots SET role = 'admin' WHERE id = $1")
        .bind(pilot_id)
        .execute(&fx.club.pool)
        .await
        .unwrap();
    fx.club
        .ensure_membership_row(pilot_id, club_id, "admin", "approved")
        .await
        .unwrap();
    login_cookie(&mut fx.router, cookie_email).await
}

#[tokio::test]
#[serial]
async fn kiosk_checkin_qr_visible_admin_can_set_and_display_reads() {
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    let pilot_a = fx.pilot_a.clone();
    let admin_cookie = make_admin(&mut fx, &pilot_a, &club_id, "a@test").await;
    let post = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/club/kiosk-checkin-qr-visible")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, admin_cookie.clone())
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(r#"{"visible":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(post.status(), StatusCode::OK);
    let body = to_bytes(post.into_body(), usize::MAX).await.unwrap();
    let layout: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        layout.get("checkinQrVisible").and_then(|v| v.as_bool()),
        Some(false)
    );

    let (_sid, display_token) = fx
        .club
        .create_display_session(&fx.pilot_a, "kiosk")
        .await
        .unwrap();
    let display_cookie = format!(
        "{}={display_token}; Path=/",
        crate::club::auth::DISPLAY_COOKIE
    );
    let disp = fx
        .router
        .call(
            Request::builder()
                .uri("/api/display/kiosk-layout")
                .header(header::COOKIE, display_cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disp.status(), StatusCode::OK);
    let disp_body = to_bytes(disp.into_body(), usize::MAX).await.unwrap();
    let disp_layout: serde_json::Value = serde_json::from_slice(&disp_body).unwrap();
    assert_eq!(
        disp_layout
            .get("checkinQrVisible")
            .and_then(|v| v.as_bool()),
        Some(false)
    );

    let screen_qr = fx
        .router
        .call(
            Request::builder()
                .uri("/api/club/screen/qr.svg")
                .header(header::COOKIE, display_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(screen_qr.status(), StatusCode::NOT_FOUND);

    let admin_qr = fx
        .router
        .call(
            Request::builder()
                .uri("/api/admin/checkin-qr.svg")
                .header(header::COOKIE, admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin_qr.status(), StatusCode::OK);
}

#[tokio::test]
#[serial]
async fn kiosk_checkin_qr_visible_staff_and_pilot_forbidden() {
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    fx.club
        .ensure_membership_row(&fx._pilot_b, &club_id, "staff", "approved")
        .await
        .unwrap();
    let staff_cookie = login_cookie(&mut fx.router, "b@test").await;
    let pilot_cookie = login_cookie(&mut fx.router, "a@test").await;
    for cookie in [staff_cookie, pilot_cookie] {
        let res = fx
            .router
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/api/admin/club/kiosk-checkin-qr-visible")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::COOKIE, cookie)
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from(r#"{"visible":false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
#[serial]
async fn kiosk_checkin_qr_visible_anonymous_unauthorized() {
    let mut fx = setup().await;
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/club/kiosk-checkin-qr-visible")
                .header(header::CONTENT_TYPE, "application/json")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(r#"{"visible":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[serial]
async fn kiosk_checkin_qr_visible_cross_site_post_csrf() {
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    let pilot_a = fx.pilot_a.clone();
    let admin_cookie = make_admin(&mut fx, &pilot_a, &club_id, "a@test").await;
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/club/kiosk-checkin-qr-visible")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, admin_cookie)
                .header("sec-fetch-site", "cross-site")
                .body(Body::from(r#"{"visible":false}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

const TEST_ANDROID_FP: &str =
    "00:01:02:03:04:05:06:07:08:09:0A:0B:0C:0D:0E:0F:10:11:12:13:14:15:16:17:18:19:1A:1B:1C:1D:1E:1F";

#[tokio::test]
#[serial]
async fn admin_checkin_qr_routes_auth_matrix() {
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    let pilot_cookie = login_cookie(&mut fx.router, "b@test").await;
    fx.club
        .ensure_membership_row(&fx._pilot_b, &club_id, "staff", "approved")
        .await
        .unwrap();
    let staff_cookie = login_cookie(&mut fx.router, "b@test").await;
    let pilot_a = fx.pilot_a.clone();
    let admin_cookie = make_admin(&mut fx, &pilot_a, &club_id, "a@test").await;

    let anon_svg = fx
        .router
        .call(
            Request::builder()
                .uri("/api/admin/checkin-qr.svg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anon_svg.status(), StatusCode::UNAUTHORIZED);

    for cookie in [staff_cookie.clone(), pilot_cookie] {
        let res = fx
            .router
            .call(
                Request::builder()
                    .uri("/api/admin/checkin-qr.svg")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    let admin_svg = fx
        .router
        .call(
            Request::builder()
                .uri("/api/admin/checkin-qr.svg")
                .header(header::COOKIE, admin_cookie.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(admin_svg.status(), StatusCode::OK);

    let anon_consumed = fx
        .router
        .call(
            Request::builder()
                .uri("/api/admin/checkin-qr/consumed?sinceMs=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(anon_consumed.status(), StatusCode::UNAUTHORIZED);

    let staff_consumed = fx
        .router
        .call(
            Request::builder()
                .uri("/api/admin/checkin-qr/consumed?sinceMs=1")
                .header(header::COOKIE, staff_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(staff_consumed.status(), StatusCode::FORBIDDEN);
}

#[test]
#[serial]
#[allow(clippy::await_holding_lock)]
fn android_fingerprint_env_validation() {
    let _guard = crate::club::android_assetlinks::android_test_env_lock();
    std::env::remove_var("FLIGHT_ANDROID_CERT_SHA256");
    std::env::set_var("FLIGHT_ANDROID_CERT_SHA256", "AA:BB:CC, not-valid");
    let list = crate::club::android_assetlinks::android_cert_sha256_list();
    assert!(list.is_empty());
    std::env::set_var("FLIGHT_ANDROID_CERT_SHA256", TEST_ANDROID_FP);
    let list = crate::club::android_assetlinks::android_cert_sha256_list();
    assert_eq!(list.len(), 1);
    std::env::remove_var("FLIGHT_ANDROID_CERT_SHA256");
}

// ---- Disk-library session policy -------------------------------------------

const DISK_TRACK: &str = "<?xml version=\"1.0\"?>\n<gpx version=\"1.1\"><trk><name>Disk</name><trkseg>\n<trkpt lat=\"-33.4\" lon=\"-70.6\"><ele>800</ele><time>2024-01-01T12:00:00Z</time></trkpt>\n<trkpt lat=\"-33.41\" lon=\"-70.61\"><ele>820</ele><time>2024-01-01T12:01:00Z</time></trkpt>\n</trkseg></trk></gpx>";

async fn call_json(
    router: &mut axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    if !body.is_empty() {
        request = request.header(header::CONTENT_TYPE, "application/json");
    }
    let payload = if body.is_empty() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    let res = router.call(request.body(payload).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

async fn put_disk_flight(
    router: &mut axum::Router,
    cookie: Option<&str>,
    slug: &str,
) -> (StatusCode, serde_json::Value) {
    let body = serde_json::json!({
        "name": slug,
        "fileName": format!("{slug}.gpx"),
        "track": DISK_TRACK,
        "tags": [],
    })
    .to_string();
    call_json(
        router,
        "PUT",
        &format!("/api/flights/{slug}"),
        cookie,
        &body,
    )
    .await
}

async fn login_legacy(router: &mut axum::Router) -> String {
    let res = router
        .call(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json")
                .body(Body::from(r#"{"password":"x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    res.headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .expect("set-cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string()
}

async fn account_cookie(club: &ClubDb, email: &str) -> String {
    let now = now_ms();
    let account_id = club
        .ensure_account_for_email(email, None, now)
        .await
        .expect("account");
    let session = club
        .create_account_session(&account_id, None, now)
        .await
        .expect("account session");
    crate::club::auth::set_account_session_cookie(&session, false)
}

#[tokio::test]
#[serial]
async fn club_session_is_signed_in_but_cannot_manage_disk_library() {
    let mut fx = setup().await;
    let cookie = login_cookie(&mut fx.router, "a@test").await;
    let cookie = cookie.as_str();

    let (status, body) = call_json(&mut fx.router, "GET", "/api/auth/me", Some(cookie), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["authenticated"], true);
    assert_eq!(body["canManageLibrary"], false);
    assert_eq!(body["configured"], true);

    let (status, body) = call_json(&mut fx.router, "GET", "/api/flights", Some(cookie), "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_array(), "expected a JSON array, got {body}");
    assert!(body.as_array().unwrap().is_empty());

    let (status, body) = put_disk_flight(&mut fx.router, Some(cookie), "club-denied").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");
}

#[tokio::test]
#[serial]
async fn account_session_is_signed_in_but_cannot_manage_disk_library() {
    let mut fx = setup().await;
    let cookie = account_cookie(&fx.club, "acct@test").await;
    let cookie = cookie.as_str();

    let (status, body) = call_json(&mut fx.router, "GET", "/api/auth/me", Some(cookie), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["authenticated"], true);
    assert_eq!(body["canManageLibrary"], false);
    assert_eq!(body["configured"], true);

    let (status, body) = call_json(&mut fx.router, "GET", "/api/flights", Some(cookie), "").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_array(), "expected a JSON array, got {body}");
    assert!(body.as_array().unwrap().is_empty());

    let (status, body) = put_disk_flight(&mut fx.router, Some(cookie), "acct-denied").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");
}

#[tokio::test]
#[serial]
async fn superadmin_sessions_can_manage_disk_library() {
    let mut fx = setup_with_superadmins(&["a@test"]).await;

    // Seed one disk track through a legacy fv_session.
    let legacy = login_legacy(&mut fx.router).await;
    assert!(legacy.starts_with("fv_session="));
    let legacy = legacy.as_str();
    let (status, _) = put_disk_flight(&mut fx.router, Some(legacy), "seeded").await;
    assert_eq!(status, StatusCode::OK);

    // Club pilot whose email is in the superadmin allowlist.
    let club_cookie = login_cookie(&mut fx.router, "a@test").await;
    let club_cookie = club_cookie.as_str();
    let (status, body) =
        call_json(&mut fx.router, "GET", "/api/auth/me", Some(club_cookie), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["authenticated"], true);
    assert_eq!(body["canManageLibrary"], true);

    let (status, body) =
        call_json(&mut fx.router, "GET", "/api/flights", Some(club_cookie), "").await;
    assert_eq!(status, StatusCode::OK);
    let slugs: Vec<&str> = body
        .as_array()
        .expect("array")
        .iter()
        .filter_map(|item| item["slug"].as_str())
        .collect();
    assert!(slugs.contains(&"seeded"), "missing seeded track: {body}");

    let (status, _) = put_disk_flight(&mut fx.router, Some(club_cookie), "from-superadmin").await;
    assert_eq!(status, StatusCode::OK);

    // Account session promoted through accounts.global_role.
    let now = now_ms();
    let account_id = fx
        .club
        .ensure_account_for_email("root@test", None, now)
        .await
        .expect("account");
    sqlx::query("UPDATE accounts SET global_role = 'superadmin' WHERE id = $1")
        .bind(&account_id)
        .execute(&fx.club.pool)
        .await
        .expect("promote");
    let session = fx
        .club
        .create_account_session(&account_id, None, now)
        .await
        .expect("account session");
    let root_cookie = crate::club::auth::set_account_session_cookie(&session, false);
    let root_cookie = root_cookie.as_str();
    let (status, body) =
        call_json(&mut fx.router, "GET", "/api/auth/me", Some(root_cookie), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["authenticated"], true);
    assert_eq!(body["canManageLibrary"], true);
    let (status, _) = put_disk_flight(&mut fx.router, Some(root_cookie), "root-write").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
#[serial]
async fn anonymous_invalid_and_device_credentials_are_not_signed_in() {
    let mut fx = setup().await;

    let (status, body) = call_json(&mut fx.router, "GET", "/api/auth/me", None, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["authenticated"], false);
    assert_eq!(body["canManageLibrary"], false);
    assert_eq!(body["configured"], true);

    let (status, body) = call_json(&mut fx.router, "GET", "/api/flights", None, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "unauthorized");

    // Invalid account session id.
    let bogus = "fv_account_session=not-a-real-session";
    let (status, body) = call_json(&mut fx.router, "GET", "/api/auth/me", Some(bogus), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["authenticated"], false);
    assert_eq!(body["canManageLibrary"], false);

    // Expired account session.
    let now = now_ms();
    let account_id = fx
        .club
        .ensure_account_for_email("expired@test", None, now)
        .await
        .expect("account");
    let session = fx
        .club
        .create_account_session(&account_id, None, now)
        .await
        .expect("account session");
    sqlx::query("UPDATE account_sessions SET expires_at = 0 WHERE account_id = $1")
        .bind(&account_id)
        .execute(&fx.club.pool)
        .await
        .expect("expire");
    let expired = crate::club::auth::set_account_session_cookie(&session, false);
    let expired = expired.as_str();
    let (status, body) = call_json(&mut fx.router, "GET", "/api/auth/me", Some(expired), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["authenticated"], false);
    assert_eq!(body["canManageLibrary"], false);

    // Device token / Authorization: Bearer alone never authenticates.
    let res = fx
        .router
        .call(
            Request::builder()
                .uri("/api/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {}", fx.device_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["authenticated"], false);
    assert_eq!(body["canManageLibrary"], false);

    let res = fx
        .router
        .call(
            Request::builder()
                .uri("/api/flights")
                .header(header::AUTHORIZATION, format!("Bearer {}", fx.device_token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[serial]
async fn admin_delete_checkout_enforces_scope_csrf_and_audit() {
    let mut fx = setup().await;
    let club_a = fx.club.default_club_id().await.expect("club_a");
    let now = now_ms();

    // Promote pilot_a to active admin of club_a.
    sqlx::query("UPDATE pilots SET role = 'admin' WHERE id = $1")
        .bind(&fx.pilot_a)
        .execute(&fx.club.pool)
        .await
        .expect("role admin");
    fx.club
        .ensure_membership_row(&fx.pilot_a, &club_a, "admin", "active")
        .await
        .expect("membership admin");

    let admin_a_cookie = login_cookie(&mut fx.router, "a@test").await;
    let pilot_b_cookie = login_cookie(&mut fx.router, "b@test").await;

    // Create club_b and an active admin in club_b to test cross-club isolation.
    let club_b = crate::club::auth::new_id();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at, checkin_window_hours, latitude, longitude)
         VALUES ($1, 'Club B', 'club-b', $2, 'live', $2, 12, -33.4489, -70.6693)",
    )
    .bind(&club_b)
    .bind(now)
    .execute(&fx.club.pool)
    .await
    .expect("club_b");
    let admin_b = fx
        .club
        .register(&club_b, "Admin B", "adminb@test", "", "password12")
        .await
        .expect("register admin_b");
    fx.club
        .mark_email_verified(&admin_b, now)
        .await
        .expect("verify admin_b");
    fx.club
        .approve_pilot(&admin_b)
        .await
        .expect("approve admin_b");
    sqlx::query("UPDATE pilots SET role = 'admin' WHERE id = $1")
        .bind(&admin_b)
        .execute(&fx.club.pool)
        .await
        .expect("admin_b role");
    fx.club
        .ensure_membership_row(&admin_b, &club_b, "admin", "active")
        .await
        .expect("admin_b membership");
    let admin_b_sess = fx
        .club
        .create_pilot_session(&admin_b, now)
        .await
        .expect("admin_b session");
    let admin_b_cookie = crate::club::auth::set_session_cookie(&admin_b_sess, false);

    // Fetch club_a checkouts and pick pilot_b's checkout.
    let (status, list) = call_json(
        &mut fx.router,
        "GET",
        "/api/admin/checkouts",
        Some(&admin_a_cookie),
        "",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let target_id = list["checkouts"]
        .as_array()
        .expect("checkouts array")
        .iter()
        .find(|c| c["pilot_email"] == "b@test")
        .and_then(|c| c["id"].as_str())
        .expect("pilot_b checkout id")
        .to_string();
    let uri = format!("/api/admin/checkouts/{target_id}");

    // Anonymous -> 401.
    let (status, _) = call_json(&mut fx.router, "DELETE", &uri, None, "").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Non-admin pilot -> 403.
    let (status, _) = call_json(&mut fx.router, "DELETE", &uri, Some(&pilot_b_cookie), "").await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Cross-site CSRF -> 403.
    let csrf_res = fx
        .router
        .call(
            Request::builder()
                .method("DELETE")
                .uri(&uri)
                .header(header::COOKIE, &admin_a_cookie)
                .header("sec-fetch-site", "cross-site")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(csrf_res.status(), StatusCode::FORBIDDEN);

    // Cross-club admin -> 404 and checkout remains in club_a.
    let (status, _) = call_json(&mut fx.router, "DELETE", &uri, Some(&admin_b_cookie), "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(fx
        .club
        .list_checkouts(&club_a)
        .await
        .iter()
        .any(|c| c.id == target_id));

    // Same-club admin -> 200 and checkout removed + audited.
    let (status, body) = call_json(&mut fx.router, "DELETE", &uri, Some(&admin_a_cookie), "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);
    assert!(!fx
        .club
        .list_checkouts(&club_a)
        .await
        .iter()
        .any(|c| c.id == target_id));

    let audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log
         WHERE club_id = $1 AND actor_pilot_id = $2
           AND action = 'revoke' AND entity_type = 'checkout' AND entity_id = $3",
    )
    .bind(&club_a)
    .bind(&fx.pilot_a)
    .bind(&target_id)
    .fetch_one(&fx.club.pool)
    .await
    .expect("audit query");
    assert_eq!(audit_count, 1);

    // Deleting again -> 404.
    let (status, _) = call_json(&mut fx.router, "DELETE", &uri, Some(&admin_a_cookie), "").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

fn extract_magic_token_from_mail(mailer: &crate::club::mail::Mailer, to_email: &str) -> String {
    let mail = mailer
        .captured()
        .into_iter()
        .rev()
        .find(|m| m.to.eq_ignore_ascii_case(to_email))
        .expect("captured magic link email");
    let marker = "token=";
    let start = mail.body.find(marker).expect("token= in magic link") + marker.len();
    mail.body[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '"' && *c != '<' && *c != '&')
        .collect()
}

fn extract_cookie_pair(headers: &axum::http::HeaderMap, cookie_name: &str) -> Option<String> {
    let prefix = format!("{cookie_name}=");
    headers
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with(&prefix) && !c.contains("Max-Age=0"))
        .and_then(|c| c.split(';').next())
        .map(String::from)
}

#[tokio::test]
#[serial]
async fn signup_routes_serve_signup_html_with_strict_csp_no_redirect() {
    let mut fx = setup().await;
    std::fs::create_dir_all(&fx.dist_dir).expect("dist dir");
    std::fs::write(
        fx.dist_dir.join("signup.html"),
        "<!doctype html><html lang=\"es-CL\"><body><h1>Crea tu cuenta</h1></body></html>",
    )
    .expect("write signup.html");

    for uri in ["/signup", "/signup.html"] {
        let res = fx
            .router
            .call(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "uri {uri} must not redirect");
        assert!(
            res.headers().get(header::LOCATION).is_none(),
            "uri {uri} must not set Location"
        );
        assert_eq!(
            res.headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .and_then(|v| v.to_str().ok()),
            Some(crate::html_csp::HTML_STRICT_CSP),
            "uri {uri} must use strict CSP"
        );
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("Crea tu cuenta"), "uri {uri} body mismatch");
    }
}

#[tokio::test]
#[serial]
async fn public_clubs_endpoint_lists_only_active_non_suspended_with_name_and_slug_only() {
    let mut fx = setup().await;
    let now = now_ms();

    let active_slug = format!("act-{}", &crate::club::auth::new_id()[..8]);
    let (active_id, active_tok) = fx
        .club
        .signup_club("Club Activo", &active_slug, "act@test", Some("SCEL"), now)
        .await
        .unwrap();
    fx.club
        .try_consume_magic_club_verify(&active_tok, now)
        .await
        .unwrap();
    let _ = active_id;

    let pending_slug = format!("pend-{}", &crate::club::auth::new_id()[..8]);
    let _ = fx
        .club
        .signup_club("Club Pendiente", &pending_slug, "pend@test", None, now)
        .await
        .unwrap();

    let susp_slug = format!("susp-{}", &crate::club::auth::new_id()[..8]);
    let (susp_id, susp_tok) = fx
        .club
        .signup_club("Club Suspendido", &susp_slug, "susp@test", None, now)
        .await
        .unwrap();
    fx.club
        .try_consume_magic_club_verify(&susp_tok, now)
        .await
        .unwrap();
    fx.club.set_club_suspended(&susp_id, now).await.unwrap();

    let (status, body) = call_json(&mut fx.router, "GET", "/api/clubs/public", None, "").await;
    assert_eq!(status, StatusCode::OK);
    let clubs = body["clubs"].as_array().expect("clubs array");
    let slugs: Vec<&str> = clubs.iter().filter_map(|c| c["slug"].as_str()).collect();
    assert!(slugs.contains(&active_slug.as_str()));
    assert!(!slugs.contains(&pending_slug.as_str()));
    assert!(!slugs.contains(&susp_slug.as_str()));

    for item in clubs {
        let obj = item.as_object().expect("club object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["name", "slug"],
            "public club entry must expose only name and slug, got {item}"
        );
    }
}

#[tokio::test]
#[serial]
async fn signup_with_club_magic_link_creates_pending_request_enforces_access_and_is_idempotent() {
    ensure_test_i18n();
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    let club_slug: String = sqlx::query_scalar("SELECT slug FROM clubs WHERE id = $1")
        .bind(&club_id)
        .fetch_one(&fx.club.pool)
        .await
        .unwrap();
    let pilot_a = fx.pilot_a.clone();
    let admin_cookie = make_admin(&mut fx, &pilot_a, &club_id, "a@test").await;
    let club_flight_id = start_flight(
        &mut fx.router,
        &fx.club,
        &admin_cookie,
        &fx.device_token,
        "club",
    )
    .await;

    let (st, counts_before) = call_json(
        &mut fx.router,
        "GET",
        "/api/admin/pending-counts",
        Some(&admin_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(counts_before["pilots"], 0);

    // 1. Magic-link signup with optional clubSlug (no password).
    let email = "magic-club-join@test";
    let signup_body = serde_json::json!({ "email": email, "clubSlug": club_slug }).to_string();
    let signup_res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/signup")
                .header(header::CONTENT_TYPE, "application/json")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(signup_body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(signup_res.status(), StatusCode::CREATED);

    // Pending request must NOT exist before email verification.
    let (_, counts_unverified) = call_json(
        &mut fx.router,
        "GET",
        "/api/admin/pending-counts",
        Some(&admin_cookie),
        "",
    )
    .await;
    assert_eq!(counts_unverified["pilots"], 0);

    // 2. Consume magic link (without nonce cookie -> works cross-device too).
    let token = extract_magic_token_from_mail(&fx.mailer, email);
    let verify_res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/auth/magic")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(format!("token={token}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(verify_res.status(), StatusCode::FOUND);
    let acct_cookie = extract_cookie_pair(verify_res.headers(), crate::club::auth::ACCOUNT_COOKIE)
        .expect("account session cookie");

    // 3. Visible in admin pending pilots queue, admin memberships, and pending counts.
    let (st, counts_after) = call_json(
        &mut fx.router,
        "GET",
        "/api/admin/pending-counts",
        Some(&admin_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(counts_after["pilots"], 1);

    let (st, pend_pilots) = call_json(
        &mut fx.router,
        "GET",
        "/api/admin/pilots/pending",
        Some(&admin_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let pending_pilot_id = pend_pilots["pilots"]
        .as_array()
        .expect("pilots array")
        .iter()
        .find(|p| p["email"] == email)
        .and_then(|p| p["id"].as_str())
        .expect("pending pilot visible in admin pending queue")
        .to_string();

    let (st, memberships_json) = call_json(
        &mut fx.router,
        "GET",
        "/api/admin/memberships",
        Some(&admin_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let membership_id = memberships_json["memberships"]
        .as_array()
        .expect("memberships array")
        .iter()
        .find(|m| m["email"] == email && m["status"] == "pending")
        .and_then(|m| m["id"].as_str())
        .expect("pending membership visible in admin memberships")
        .to_string();

    // 4. Idempotent: re-submitting signup and consuming a second magic link creates no duplicates.
    let signup_res2 = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/signup")
                .header(header::CONTENT_TYPE, "application/json")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(signup_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(signup_res2.status(), StatusCode::CREATED);
    let nonce2 = extract_cookie_pair(
        signup_res2.headers(),
        crate::club::auth::SIGNUP_NONCE_COOKIE,
    )
    .expect("nonce cookie");
    let token2 = extract_magic_token_from_mail(&fx.mailer, email);
    let verify_res2 = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/auth/magic")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, &nonce2)
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(format!("token={token2}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(verify_res2.status(), StatusCode::FOUND);

    let mem_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*)::bigint FROM club_memberships m
         JOIN pilots p ON p.id = m.pilot_id
         WHERE lower(p.email) = lower($1) AND m.club_id = $2",
    )
    .bind(email)
    .bind(&club_id)
    .fetch_one(&fx.club.pool)
    .await
    .unwrap();
    assert_eq!(
        mem_count, 1,
        "double signup must not duplicate pending request"
    );

    // 5. NO club access before admin approves.
    let (st, _) = call_json(
        &mut fx.router,
        "POST",
        "/api/account/switch-club",
        Some(&acct_cookie),
        &serde_json::json!({ "membershipId": membership_id }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    for denied_uri in [
        "/api/club/map",
        "/api/me/tails",
        "/api/admin/pilots",
        "/api/admin/memberships",
        "/api/admin/display-sessions",
    ] {
        let (st, _) = call_json(&mut fx.router, "GET", denied_uri, Some(&acct_cookie), "").await;
        assert!(
            st == StatusCode::UNAUTHORIZED || st == StatusCode::FORBIDDEN,
            "pending member must not access {denied_uri}, got {st}"
        );
    }

    let (st, _) = call_json(
        &mut fx.router,
        "GET",
        &format!("/api/flights/{club_flight_id}/fixes"),
        Some(&acct_cookie),
        "",
    )
    .await;
    assert_ne!(
        st,
        StatusCode::OK,
        "pending member must not access club-visibility flight"
    );

    let (st, lib) = call_json(
        &mut fx.router,
        "GET",
        "/api/flights",
        Some(&acct_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(lib.as_array().map(Vec::len), Some(0));
    let (st, _) = put_disk_flight(&mut fx.router, Some(&acct_cookie), "pending-denied").await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);

    // Legacy club magic login also rejects pending membership.
    let legacy_tok = fx
        .club
        .issue_magic_login(&club_id, email, now_ms())
        .await
        .unwrap()
        .expect("token");
    assert!(
        fx.club
            .consume_magic_login(&legacy_tok, &club_id, now_ms())
            .await
            .is_err(),
        "pending member must not mint legacy club session via magic login"
    );

    // 6. Admin approves the pending pilot -> count drops and pilot gains club access.
    let (st, _) = call_json(
        &mut fx.router,
        "POST",
        &format!("/api/admin/pilots/{pending_pilot_id}/approve"),
        Some(&admin_cookie),
        "{}",
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (_, counts_approved) = call_json(
        &mut fx.router,
        "GET",
        "/api/admin/pending-counts",
        Some(&admin_cookie),
        "",
    )
    .await;
    assert_eq!(counts_approved["pilots"], 0);

    let (st, _) = call_json(
        &mut fx.router,
        "POST",
        "/api/account/switch-club",
        Some(&acct_cookie),
        &serde_json::json!({ "membershipId": membership_id }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _) = call_json(
        &mut fx.router,
        "GET",
        "/api/club/map",
        Some(&acct_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call_json(
        &mut fx.router,
        "GET",
        "/api/me/tails",
        Some(&acct_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = call_json(
        &mut fx.router,
        "GET",
        &format!("/api/flights/{club_flight_id}/fixes"),
        Some(&acct_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
#[serial]
async fn signup_with_club_password_flow_creates_pending_request_and_enables_password_login() {
    ensure_test_i18n();
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    let club_slug: String = sqlx::query_scalar("SELECT slug FROM clubs WHERE id = $1")
        .bind(&club_id)
        .fetch_one(&fx.club.pool)
        .await
        .unwrap();
    let pilot_a = fx.pilot_a.clone();
    let admin_cookie = make_admin(&mut fx, &pilot_a, &club_id, "a@test").await;

    let email = "pw-club-join@test";
    let pw = "password1234";
    let signup_res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/signup")
                .header(header::CONTENT_TYPE, "application/json")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(
                    serde_json::json!({
                        "email": email,
                        "password": pw,
                        "clubSlug": club_slug,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(signup_res.status(), StatusCode::CREATED);
    let nonce_cookie =
        extract_cookie_pair(signup_res.headers(), crate::club::auth::SIGNUP_NONCE_COOKIE)
            .expect("signup nonce cookie");

    let token = extract_magic_token_from_mail(&fx.mailer, email);
    let verify_res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/auth/magic")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, &nonce_cookie)
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(format!("token={token}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(verify_res.status(), StatusCode::FOUND);

    // Password login succeeds after verification.
    let login_res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/login")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(
                    serde_json::json!({ "email": email, "password": pw }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_res.status(), StatusCode::OK);
    let acct_cookie = extract_cookie_pair(login_res.headers(), crate::club::auth::ACCOUNT_COOKIE)
        .expect("account session cookie from password login");

    // Pending membership appears in admin pending queue and counts.
    let (st, counts) = call_json(
        &mut fx.router,
        "GET",
        "/api/admin/pending-counts",
        Some(&admin_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(counts["pilots"], 1);

    let (st, mems) = call_json(
        &mut fx.router,
        "GET",
        "/api/admin/memberships",
        Some(&admin_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let membership_id = mems["memberships"]
        .as_array()
        .expect("memberships")
        .iter()
        .find(|m| m["email"] == email && m["status"] == "pending")
        .and_then(|m| m["id"].as_str())
        .expect("pending membership")
        .to_string();

    // Blocked before approval; granted after approval via /api/account/memberships/{id}/approve.
    let (st, _) = call_json(
        &mut fx.router,
        "POST",
        "/api/account/switch-club",
        Some(&acct_cookie),
        &serde_json::json!({ "membershipId": membership_id }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, _) = call_json(
        &mut fx.router,
        "POST",
        &format!("/api/account/memberships/{membership_id}/approve"),
        Some(&admin_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _) = call_json(
        &mut fx.router,
        "POST",
        "/api/account/switch-club",
        Some(&acct_cookie),
        &serde_json::json!({ "membershipId": membership_id }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let (st, _) = call_json(
        &mut fx.router,
        "GET",
        "/api/club/map",
        Some(&acct_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
#[serial]
async fn signup_rejects_invalid_nonexistent_unverified_and_suspended_clubs() {
    ensure_test_i18n();
    let mut fx = setup().await;
    let now = now_ms();

    let pending_slug = format!("pend-rej-{}", &crate::club::auth::new_id()[..8]);
    let _ = fx
        .club
        .signup_club("Unverified", &pending_slug, "unv@test", None, now)
        .await
        .unwrap();

    let susp_slug = format!("susp-rej-{}", &crate::club::auth::new_id()[..8]);
    let (susp_id, susp_tok) = fx
        .club
        .signup_club("Suspended", &susp_slug, "susp-rej@test", None, now)
        .await
        .unwrap();
    fx.club
        .try_consume_magic_club_verify(&susp_tok, now)
        .await
        .unwrap();
    fx.club.set_club_suspended(&susp_id, now).await.unwrap();

    for bad_slug in [
        "",
        "nonexistent-club",
        "../etc",
        "bad--slug",
        pending_slug.as_str(),
    ] {
        let email = format!("bad-{bad_slug}@test").replace('/', "_");
        let (st, body) = call_json(
            &mut fx.router,
            "POST",
            "/api/account/signup",
            None,
            &serde_json::json!({ "email": email, "clubSlug": bad_slug }).to_string(),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "slug={bad_slug}");
        assert_eq!(body["error"], "club not found", "slug={bad_slug}");
        assert!(
            fx.club.account_by_email(&email).await.is_none(),
            "rejected signup must not create account for slug={bad_slug}"
        );
    }

    let (st, body) = call_json(
        &mut fx.router,
        "POST",
        "/api/account/signup",
        None,
        &serde_json::json!({ "email": "susp-join@test", "clubSlug": susp_slug }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "club suspended");
    assert!(fx.club.account_by_email("susp-join@test").await.is_none());

    // Club suspended between signup and magic-link consume creates no membership.
    let race_slug = format!("race-susp-{}", &crate::club::auth::new_id()[..8]);
    let (race_club_id, race_tok) = fx
        .club
        .signup_club("Race Club", &race_slug, "race-owner@test", None, now)
        .await
        .unwrap();
    fx.club
        .try_consume_magic_club_verify(&race_tok, now)
        .await
        .unwrap();

    let race_email = "race-pilot@test";
    let (st, _) = call_json(
        &mut fx.router,
        "POST",
        "/api/account/signup",
        None,
        &serde_json::json!({ "email": race_email, "clubSlug": race_slug }).to_string(),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let tok = extract_magic_token_from_mail(&fx.mailer, race_email);
    fx.club
        .set_club_suspended(&race_club_id, now)
        .await
        .unwrap();

    let verify_res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/auth/magic")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(format!("token={tok}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(verify_res.status(), StatusCode::FOUND);
    let acct = fx
        .club
        .account_by_email(race_email)
        .await
        .expect("verified account");
    assert!(fx.club.list_account_memberships(&acct.id).await.is_empty());
}

#[tokio::test]
#[serial]
async fn signup_without_club_works_for_both_magic_and_password() {
    ensure_test_i18n();
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    let pilot_a = fx.pilot_a.clone();
    let admin_cookie = make_admin(&mut fx, &pilot_a, &club_id, "a@test").await;

    // 1. Magic-link signup without club.
    let email_magic = "noclub-magic@test";
    let res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/signup")
                .header(header::CONTENT_TYPE, "application/json")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(
                    serde_json::json!({ "email": email_magic }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let tok_magic = extract_magic_token_from_mail(&fx.mailer, email_magic);
    let verify_magic = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/auth/magic")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(format!("token={tok_magic}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(verify_magic.status(), StatusCode::FOUND);
    let cookie_magic =
        extract_cookie_pair(verify_magic.headers(), crate::club::auth::ACCOUNT_COOKIE)
            .expect("magic account session");
    let (st, me_magic) = call_json(
        &mut fx.router,
        "GET",
        "/api/account/me",
        Some(&cookie_magic),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me_magic["memberships"].as_array().map(Vec::len), Some(0));

    // 2. Password signup without club.
    let email_pw = "noclub-pw@test";
    let pw = "password1234";
    let res_pw = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/signup")
                .header(header::CONTENT_TYPE, "application/json")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(
                    serde_json::json!({ "email": email_pw, "password": pw }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res_pw.status(), StatusCode::CREATED);
    let nonce_pw = extract_cookie_pair(res_pw.headers(), crate::club::auth::SIGNUP_NONCE_COOKIE)
        .expect("nonce cookie");
    let tok_pw = extract_magic_token_from_mail(&fx.mailer, email_pw);
    let verify_pw = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/auth/magic")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, &nonce_pw)
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(format!("token={tok_pw}")))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(verify_pw.status(), StatusCode::FOUND);

    let login_pw = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/login")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json")
                .header("sec-fetch-site", "same-origin")
                .body(Body::from(
                    serde_json::json!({ "email": email_pw, "password": pw }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_pw.status(), StatusCode::OK);
    let cookie_pw = extract_cookie_pair(login_pw.headers(), crate::club::auth::ACCOUNT_COOKIE)
        .expect("password account session");
    let (st, me_pw) = call_json(
        &mut fx.router,
        "GET",
        "/api/account/me",
        Some(&cookie_pw),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me_pw["memberships"].as_array().map(Vec::len), Some(0));

    let (_, counts) = call_json(
        &mut fx.router,
        "GET",
        "/api/admin/pending-counts",
        Some(&admin_cookie),
        "",
    )
    .await;
    assert_eq!(counts["pilots"], 0);
}

#[tokio::test]
#[serial]
async fn club_me_includes_club_name_for_own_club() {
    let mut fx = setup().await;
    let club_a_id = fx.club.default_club_id().await.unwrap();
    sqlx::query("UPDATE clubs SET name = 'Club Aéreo Alfa' WHERE id = $1")
        .bind(&club_a_id)
        .execute(&fx.club.pool)
        .await
        .unwrap();

    // Anonymous returns pilot: null with no clubName.
    let (st, anon) = call_json(&mut fx.router, "GET", "/api/club/me", None, "").await;
    assert_eq!(st, StatusCode::OK);
    assert!(anon["pilot"].is_null());
    assert!(anon.get("clubName").is_none());

    // Signed-in member of club A gets Club Aéreo Alfa.
    let cookie_a = login_cookie(&mut fx.router, "a@test").await;
    let (st, me_a) = call_json(&mut fx.router, "GET", "/api/club/me", Some(&cookie_a), "").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me_a["pilot"]["email"], "a@test");
    assert_eq!(me_a["clubName"], "Club Aéreo Alfa");
    assert!(me_a["clubSlug"].as_str().is_some_and(|s| !s.is_empty()));

    // Create a second club and member; verify /api/club/me returns that member's own club name.
    let now = now_ms();
    let slug_b = format!("club-b-{}", &crate::club::auth::new_id()[..8]);
    let (club_b_id, tok_b) = fx
        .club
        .signup_club("Club Aéreo Sur", &slug_b, "owner-sur@test", None, now)
        .await
        .unwrap();
    fx.club
        .try_consume_magic_club_verify(&tok_b, now)
        .await
        .unwrap();
    let pilot_b2 = fx
        .club
        .register(&club_b_id, "Sur Pilot", "sur@test", "", "password12")
        .await
        .unwrap();
    fx.club.mark_email_verified(&pilot_b2, now).await.unwrap();
    fx.club.approve_pilot(&pilot_b2).await.unwrap();
    let sess_b = fx.club.create_pilot_session(&pilot_b2, now).await.unwrap();
    let cookie_b = format!("{}={sess_b}", crate::club::auth::COOKIE);
    let (st, me_b) = call_json(&mut fx.router, "GET", "/api/club/me", Some(&cookie_b), "").await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me_b["pilot"]["email"], "sur@test");
    assert_eq!(me_b["clubName"], "Club Aéreo Sur");
    assert_eq!(me_b["clubSlug"], slug_b);

    // Suspended club revokes existing sessions and returns 403 for any session on that club.
    fx.club.set_club_suspended(&club_b_id, now).await.unwrap();
    let suspended_sess = fx.club.create_pilot_session(&pilot_b2, now).await.unwrap();
    let suspended_cookie = format!("{}={suspended_sess}", crate::club::auth::COOKIE);
    let (st, _) = call_json(
        &mut fx.router,
        "GET",
        "/api/club/me",
        Some(&suspended_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
}

#[tokio::test]
#[serial]
async fn account_logout_clears_both_cookies_revokes_only_current_session_and_enforces_csrf() {
    let mut fx = setup().await;
    let now = now_ms();
    fx.club.backfill_accounts_from_pilots().await.unwrap();

    // Ensure a@test and b@test have verified accounts with password12.
    let acct_a = fx
        .club
        .account_id_for_email("a@test")
        .await
        .expect("acct_a");
    let acct_b = fx
        .club
        .account_id_for_email("b@test")
        .await
        .expect("acct_b");
    let hash = crate::club::auth::hash_password("password12").unwrap();
    for aid in [&acct_a, &acct_b] {
        sqlx::query(
            "UPDATE accounts SET password_hash = $1, email_verified_at = COALESCE(email_verified_at, $2) WHERE id = $3",
        )
        .bind(&hash)
        .bind(now)
        .bind(aid)
        .execute(&fx.club.pool)
        .await
        .unwrap();
    }

    async fn login_account(router: &mut axum::Router, email: &str) -> String {
        let res = router
            .call(
                Request::builder()
                    .method("POST")
                    .uri("/api/account/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(header::ACCEPT, "application/json")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::from(
                        serde_json::json!({ "email": email, "password": "password12" }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        extract_cookie_pair(res.headers(), crate::club::auth::ACCOUNT_COOKIE)
            .expect("account session cookie")
    }

    // 1. Idempotent with no cookies -> 200 {"ok":true} and clears both cookies.
    let empty_logout = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/logout")
                .header("sec-fetch-site", "same-origin")
                .header("x-forwarded-proto", "https")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(empty_logout.status(), StatusCode::OK);
    let set_cookies: Vec<String> = empty_logout
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(String::from))
        .collect();
    assert!(
        set_cookies
            .iter()
            .any(|c| c.starts_with("fv_account_session=;")
                && c.contains("Max-Age=0")
                && c.contains("Secure")),
        "missing account clear cookie: {set_cookies:?}"
    );
    assert!(
        set_cookies
            .iter()
            .any(|c| c.starts_with("fv_club_session=;")
                && c.contains("Max-Age=0")
                && c.contains("Secure")),
        "missing club clear cookie: {set_cookies:?}"
    );
    assert!(
        !set_cookies.iter().any(|c| c.starts_with("fv_session=")),
        "must not clear legacy fv_session cookie: {set_cookies:?}"
    );
    let empty_body: serde_json::Value = serde_json::from_slice(
        &to_bytes(empty_logout.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(empty_body, serde_json::json!({ "ok": true }));

    // 2. Create two sessions for account A (sess1 + sess2), switch sess1 into club, plus session for account B and a legacy fv_session.
    let acct_a_sess1 = login_account(&mut fx.router, "a@test").await;
    let acct_a_sess2 = login_account(&mut fx.router, "a@test").await;
    let acct_b_sess = login_account(&mut fx.router, "b@test").await;

    let (st, me_a1) = call_json(
        &mut fx.router,
        "GET",
        "/api/account/me",
        Some(&acct_a_sess1),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let mem_a_id = me_a1["memberships"][0]["id"]
        .as_str()
        .expect("membership id")
        .to_string();

    let sw = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/switch-club")
                .header(header::CONTENT_TYPE, "application/json")
                .header("sec-fetch-site", "same-origin")
                .header(header::COOKIE, &acct_a_sess1)
                .body(Body::from(
                    serde_json::json!({ "membershipId": mem_a_id }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(sw.status(), StatusCode::OK);
    let club_a_sess1 =
        extract_cookie_pair(sw.headers(), crate::club::auth::COOKIE).expect("club cookie");

    // Legacy fv_session cookie (password is "x" in fixture).
    let legacy_res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ACCEPT, "application/json")
                .body(Body::from(r#"{"password":"x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(legacy_res.status(), StatusCode::OK);
    let legacy_cookie =
        extract_cookie_pair(legacy_res.headers(), "fv_session").expect("legacy fv_session");

    let combined_a1 = format!("{acct_a_sess1}; {club_a_sess1}; {legacy_cookie}");

    // 3. CSRF rejection with a foreign Origin must return 403 and keep sessions alive.
    let csrf_res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/logout")
                .header(header::HOST, "app.example")
                .header(header::ORIGIN, "https://evil.example")
                .header(header::COOKIE, &combined_a1)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(csrf_res.status(), StatusCode::FORBIDDEN);
    let (st, _) = call_json(
        &mut fx.router,
        "GET",
        "/api/account/me",
        Some(&acct_a_sess1),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, club_me_before) = call_json(
        &mut fx.router,
        "GET",
        "/api/club/me",
        Some(&club_a_sess1),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!club_me_before["pilot"].is_null());

    // 4. Legitimate logout revokes sess1 (account + club) and clears both cookies, leaving sess2, account B, and legacy fv_session untouched.
    let logout_res = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/account/logout")
                .header("sec-fetch-site", "same-origin")
                .header(header::COOKIE, &combined_a1)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout_res.status(), StatusCode::OK);
    let logout_cookies: Vec<String> = logout_res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok().map(String::from))
        .collect();
    assert!(logout_cookies
        .iter()
        .any(|c| c.starts_with("fv_account_session=;") && c.contains("Max-Age=0")));
    assert!(logout_cookies
        .iter()
        .any(|c| c.starts_with("fv_club_session=;") && c.contains("Max-Age=0")));
    assert!(!logout_cookies.iter().any(|c| c.starts_with("fv_session=")));
    let logout_body: serde_json::Value =
        serde_json::from_slice(&to_bytes(logout_res.into_body(), usize::MAX).await.unwrap())
            .unwrap();
    assert_eq!(logout_body, serde_json::json!({ "ok": true }));

    // sess1 account session is now 401 and club session is anonymous.
    let (st, _) = call_json(
        &mut fx.router,
        "GET",
        "/api/account/me",
        Some(&acct_a_sess1),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let (st, club_me_after) = call_json(
        &mut fx.router,
        "GET",
        "/api/club/me",
        Some(&club_a_sess1),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(club_me_after["pilot"].is_null());

    // sess2 of the SAME account is still valid.
    let (st, me_a2) = call_json(
        &mut fx.router,
        "GET",
        "/api/account/me",
        Some(&acct_a_sess2),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me_a2["account"]["email"], "a@test");

    // Another account's session is still valid.
    let (st, me_b) = call_json(
        &mut fx.router,
        "GET",
        "/api/account/me",
        Some(&acct_b_sess),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(me_b["account"]["email"], "b@test");

    // Legacy fv_session is still valid.
    let (st, legacy_me) = call_json(
        &mut fx.router,
        "GET",
        "/api/auth/me",
        Some(&legacy_cookie),
        "",
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(legacy_me["authenticated"], true);
}
