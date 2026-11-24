//! Public live map endpoint metadata & visibility tests.
use crate::club::{api_tests::setup, auth::new_id, db::now_ms};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use serial_test::serial;
use tower::Service;

#[tokio::test]
#[serial]
async fn public_map_returns_club_name_and_base_with_zero_flights() {
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    let slug: String = sqlx::query_scalar("SELECT slug FROM clubs WHERE id = $1")
        .bind(&club_id)
        .fetch_one(&fx.club.pool)
        .await
        .unwrap();
    sqlx::query("UPDATE clubs SET name = $1 WHERE id = $2")
        .bind("Club Aéreo de Santiago")
        .bind(&club_id)
        .execute(&fx.club.pool)
        .await
        .unwrap();
    fx.club
        .set_club_base_aerodrome(&club_id, "SCTB", -33.456, -70.548)
        .await
        .unwrap();

    for uri in [
        format!("/api/club/map/public?club={slug}"),
        "/api/club/map/public".to_string(),
    ] {
        let res = fx
            .router
            .call(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "uri={uri}");
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["aircraft"].as_array().unwrap().len(), 0);
        assert_eq!(json["clubName"], "Club Aéreo de Santiago");
        assert_eq!(json["base"]["icao"], "SCTB");
        assert!((json["base"]["lat"].as_f64().unwrap() - (-33.456)).abs() < 1e-6);
        assert!((json["base"]["lon"].as_f64().unwrap() - (-70.548)).abs() < 1e-6);
    }
}

#[tokio::test]
#[serial]
async fn public_map_null_base_when_club_has_no_coordinates() {
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    let slug: String = sqlx::query_scalar("SELECT slug FROM clubs WHERE id = $1")
        .bind(&club_id)
        .fetch_one(&fx.club.pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE clubs SET aerodrome_icao = NULL, latitude = NULL, longitude = NULL WHERE id = $1",
    )
    .bind(&club_id)
    .execute(&fx.club.pool)
    .await
    .unwrap();

    let res = fx
        .router
        .call(
            Request::builder()
                .uri(format!("/api/club/map/public?club={slug}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(json["clubName"].as_str().is_some());
    assert!(json["base"]["icao"].is_null());
    assert!(json["base"]["lat"].is_null());
    assert!(json["base"]["lon"].is_null());
}

#[tokio::test]
#[serial]
async fn public_map_never_exposes_unverified_or_suspended_club_metadata() {
    let mut fx = setup().await;
    let now = now_ms();

    // Pending / unverified club
    let pending_slug = format!("pending-club-{}", &new_id()[..8]);
    let pending_id = new_id();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at, aerodrome_icao, latitude, longitude)
         VALUES ($1, 'Secret Pending Club', $2, $3, 'pending', NULL, 'SCEL', -33.393, -70.786)",
    )
    .bind(&pending_id)
    .bind(&pending_slug)
    .bind(now)
    .execute(&fx.club.pool)
    .await
    .unwrap();

    let pending_res = fx
        .router
        .call(
            Request::builder()
                .uri(format!("/api/club/map/public?club={pending_slug}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(pending_res.status(), StatusCode::NOT_FOUND);
    let pending_body = String::from_utf8(
        to_bytes(pending_res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!pending_body.contains("Secret Pending Club"));
    assert!(!pending_body.contains("SCEL"));

    // Suspended club
    let susp_slug = format!("susp-club-{}", &new_id()[..8]);
    let susp_id = new_id();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at, suspended_at, aerodrome_icao, latitude, longitude)
         VALUES ($1, 'Suspended Secret Club', $2, $3, 'live', $3, $3, 'SCVM', -32.95, -71.48)",
    )
    .bind(&susp_id)
    .bind(&susp_slug)
    .bind(now)
    .execute(&fx.club.pool)
    .await
    .unwrap();

    let susp_res = fx
        .router
        .call(
            Request::builder()
                .uri(format!("/api/club/map/public?club={susp_slug}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(susp_res.status(), StatusCode::FORBIDDEN);
    let susp_body = String::from_utf8(
        to_bytes(susp_res.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(!susp_body.contains("Suspended Secret Club"));
    assert!(!susp_body.contains("SCVM"));

    // Unknown club
    let missing_res = fx
        .router
        .call(
            Request::builder()
                .uri("/api/club/map/public?club=no-such-club-xyz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing_res.status(), StatusCode::NOT_FOUND);
}
