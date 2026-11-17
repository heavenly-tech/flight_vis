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
use std::sync::Arc;
use tower::Service;

struct Fixture {
    _dir: tempfile::TempDir,
    router: axum::Router,
    club: ClubDb,
    pilot_a: String,
    _pilot_b: String,
    device_token: String,
}

fn setup() -> Fixture {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let club = ClubDb::open(data_dir.path()).expect("club db");
    let club_id = club.default_club_id().unwrap();
    club.upsert_type(&club_id, "C152").unwrap();
    club.upsert_aircraft(&club_id, "N1", "C152", true).unwrap();
    let pilot_a = club
        .register(&club_id, "A", "a@test", "", "password12")
        .unwrap();
    let pilot_b = club
        .register(&club_id, "B", "b@test", "", "password12")
        .unwrap();
    club.approve_pilot(&pilot_a).unwrap();
    club.approve_pilot(&pilot_b).unwrap();
    club.add_checkout(&pilot_a, "C152", None, None).unwrap();
    club.add_checkout(&pilot_b, "C152", None, None).unwrap();
    let (_dev, device_token) = club.create_device(&pilot_a, "phone").unwrap();
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
        mailer: crate::club::mail::Mailer::from_env(crate::club::mail::MailOutbox::default()),
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
    let club_id = club.default_club_id().expect("club");
    let qr = club.mint_checkin_qr_token(&club_id, now_ms()).expect("qr");
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
async fn fixes_denied_after_mid_flight_private() {
    let mut fx = setup();
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
async fn anonymous_public_map_and_name_flag() {
    let mut fx = setup();
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
async fn server_qr_svg_endpoints() {
    let mut fx = setup();
    let cookie_a = login_cookie(&mut fx.router, "a@test").await;
    let (device_id, _) = fx.club.create_device(&fx.pilot_a, "qr-phone").unwrap();

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
async fn api_me_devices_auto_register_idempotent_and_revoke_on_logout() {
    let mut fx = setup();
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
    assert!(fx.club.resolve_device_token(token2).is_none());

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
                .body(Body::from(
                    r#"{"installId":"install-api-test-01"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(logout.status(), StatusCode::OK);
    assert!(fx.club.resolve_device_token(token3).is_none());
}

#[tokio::test]
async fn pending_pilot_cannot_load_club_map() {
    let mut fx = setup();
    let club_id = fx.club.default_club_id().unwrap();
    let pending = fx
        .club
        .register(&club_id, "P", "p@test", "", "password12")
        .unwrap();
    let cookie = login_cookie(&mut fx.router, "p@test").await;
    let _ = pending;
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
