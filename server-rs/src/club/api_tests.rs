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
    pub(crate) router: axum::Router,
    pub(crate) club: ClubDb,
    pilot_a: String,
    _pilot_b: String,
    device_token: String,
}

pub(crate) async fn setup() -> Fixture {
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
    let state = AppState {
        dist_dir: data_dir.path().join("dist"),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live,
        store: Store::new(data_dir.path()),
        club: Some(club.clone()),
        video_queue,
        login_throttle: crate::LoginThrottle::default(),
        device_register_throttle: crate::LoginThrottle::default(),
        share_lookup_throttle: crate::LoginThrottle::default(),
        mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
        webauthn: None,
        sse_limiter: std::sync::Arc::new(crate::club::sse_limit::SseLimiter::default()),
        superadmin_emails: Default::default(),
    };
    Fixture {
        _dir: data_dir,
        router: build_router(state),
        club,
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
    sqlx::query(
        "UPDATE club_memberships SET role = 'admin' WHERE pilot_id = $1 AND club_id = $2",
    )
    .bind(&fx.pilot_a)
    .bind(&club_id)
    .execute(&fx.club.pool)
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
        .ensure_membership_row(&fx.pilot_a, &club_a, "admin", "approved")
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
