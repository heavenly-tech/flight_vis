//! Club roster CRUD, CSV import, audit, and cross-club isolation.

use crate::club::api_tests::{login_cookie, setup};
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
};
use serial_test::serial;
use tower::Service;

#[tokio::test]
#[serial]
async fn roster_pilot_crud_writes_audit() {
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    sqlx::query("UPDATE pilots SET role = 'admin' WHERE email = 'a@test'")
        .execute(&fx.club.pool)
        .await
        .unwrap();
    let admin_id: String = sqlx::query_scalar("SELECT id FROM pilots WHERE email = 'a@test'")
        .fetch_one(&fx.club.pool)
        .await
        .unwrap();
    fx.club
        .ensure_membership_row(&admin_id, &club_id, "admin", "approved")
        .await
        .unwrap();
    let cookie = login_cookie(&mut fx.router, "a@test").await;
    let create = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/pilots")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(
                    r#"{"name":"New","email":"new@test","license_no":"L-1","status":"approved"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(create.status(), StatusCode::OK);
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audit_log WHERE club_id = $1 AND entity_type = 'pilot' AND action = 'create'",
    )
    .bind(&club_id)
    .fetch_one(&fx.club.pool)
    .await
    .unwrap();
    assert!(n >= 1);
}

#[tokio::test]
#[serial]
async fn roster_csv_preview_commit_atomic() {
    let mut fx = setup().await;
    let club_id = fx.club.default_club_id().await.unwrap();
    sqlx::query("UPDATE pilots SET role = 'admin' WHERE email = 'a@test'")
        .execute(&fx.club.pool)
        .await
        .unwrap();
    let admin_id: String = sqlx::query_scalar("SELECT id FROM pilots WHERE email = 'a@test'")
        .fetch_one(&fx.club.pool)
        .await
        .unwrap();
    fx.club
        .ensure_membership_row(&admin_id, &club_id, "admin", "approved")
        .await
        .unwrap();
    let cookie = login_cookie(&mut fx.router, "a@test").await;
    let csv = "name,email,license_no,status\nRoster One,roster1@test,L1,approved\n";
    let prev = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/import/pilots/preview")
                .header(header::COOKIE, &cookie)
                .body(Body::from(csv))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(prev.status(), StatusCode::OK);
    let bytes = to_bytes(prev.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let batch_id = json["batch_id"].as_str().unwrap();
    let before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pilots WHERE email = 'roster1@test'")
            .fetch_one(&fx.club.pool)
            .await
            .unwrap();
    assert_eq!(before, 0);
    let commit = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/import/pilots/commit")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .body(Body::from(format!(r#"{{"batch_id":"{batch_id}"}}"#)))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(commit.status(), StatusCode::OK);
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pilots WHERE email = 'roster1@test'")
        .fetch_one(&fx.club.pool)
        .await
        .unwrap();
    assert_eq!(after, 1);
}

#[tokio::test]
#[serial]
async fn roster_cross_club_import_rejects_other_tail() {
    let mut fx = setup().await;
    let club_a = fx.club.default_club_id().await.unwrap();
    let club_b = format!("club-b-{}", &crate::club::auth::new_id()[..8]);
    let now = crate::club::db::now_ms();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at, latitude, longitude, show_public_tracks)
         VALUES ($1, 'B', $2, $3, 'live', $3, -33, -70, TRUE)",
    )
    .bind(crate::club::auth::new_id())
    .bind(&club_b)
    .bind(now)
    .execute(&fx.club.pool)
    .await
    .unwrap();
    let club_b_id: String = sqlx::query_scalar("SELECT id FROM clubs WHERE slug = $1")
        .bind(&club_b)
        .fetch_one(&fx.club.pool)
        .await
        .unwrap();
    fx.club.upsert_type(&club_b_id, "C152").await.unwrap();
    fx.club
        .upsert_aircraft(&club_b_id, "CC-BBB", "C152", true)
        .await
        .unwrap();
    sqlx::query("UPDATE pilots SET role = 'admin' WHERE email = 'a@test'")
        .execute(&fx.club.pool)
        .await
        .unwrap();
    let admin_id: String = sqlx::query_scalar("SELECT id FROM pilots WHERE email = 'a@test'")
        .fetch_one(&fx.club.pool)
        .await
        .unwrap();
    fx.club
        .ensure_membership_row(&admin_id, &club_a, "admin", "approved")
        .await
        .unwrap();
    let cookie = login_cookie(&mut fx.router, "a@test").await;
    let csv = "pilot_email,type,tail,expires_at,action\na@test,C152,CC-BBB,,assign\n";
    let prev = fx
        .router
        .call(
            Request::builder()
                .method("POST")
                .uri("/api/admin/import/authorizations/preview")
                .header(header::COOKIE, &cookie)
                .body(Body::from(csv))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(prev.status(), StatusCode::OK);
    let bytes = to_bytes(prev.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(json["can_commit"].as_bool(), Some(false));
    let errs = json["rows"][0]["errors"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(errs
        .iter()
        .any(|e| e.as_str().unwrap_or("").contains("club")));
}

#[tokio::test]
#[serial]
async fn roster_cas_club_unchanged() {
    let fx = setup().await;
    let cas: String = sqlx::query_scalar("SELECT id FROM clubs WHERE slug = 'cas'")
        .fetch_optional(&fx.club.pool)
        .await
        .unwrap()
        .unwrap_or_default();
    if !cas.is_empty() {
        let name: String = sqlx::query_scalar("SELECT name FROM clubs WHERE id = $1")
            .bind(&cas)
            .fetch_one(&fx.club.pool)
            .await
            .unwrap();
        assert!(!name.is_empty());
    }
}
