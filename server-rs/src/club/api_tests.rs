//! HTTP-level visibility tests.
use crate::{
    auth::Auth,
    build_router,
    club::{ClubDb, db::now_ms, visibility::FlightVisibility},
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
    cookie: &str,
    token: &str,
    visibility: &str,
) -> String {
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