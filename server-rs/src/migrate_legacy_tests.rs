//! Postgres integration tests for `migrate-legacy` (requires `DATABASE_URL`).

use crate::{
    auth::Auth,
    build_router,
    club::{auth, database::DbConnectError, db::ClubDb, visibility::FlightVisibility},
    demo_flight::{DEMO_FLIGHT_SLUG, LEGACY_DEMO_SLUG},
    migrate_legacy::{parse_cli_args, run_legacy_migrate, LegacyMigrateOpts},
    store::Store,
    AppState,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serial_test::serial;
use tower::ServiceExt;

const FIXTURE_EMAIL: &str = "legacy-migrate-fixture@test.invalid";
const LEGACY_EMAIL: &str = "legacy-owner-fixture@test.invalid";

async fn seed_legacy_owner(db: &ClubDb, club_id: &str) -> (String, String, String) {
    let hash = auth::hash_password("password12").unwrap();
    let legacy_id = auth::new_id();
    let now = crate::club::now_ms();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Legacy', $3, '', 'approved', 'admin', $4, $5)",
    )
    .bind(&legacy_id)
    .bind(club_id)
    .bind(LEGACY_EMAIL)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.upsert_type(club_id, "C152").await.unwrap();
    db.upsert_aircraft(club_id, "CC-LEG", "C152", true)
        .await
        .unwrap();
    let (dev, _) = db.create_device(&legacy_id, "phone").await.unwrap();
    db.issue_open_checkin(&legacy_id, &dev, club_id, now)
        .await
        .unwrap();
    let fid = db
        .start_flight(
            &legacy_id,
            &dev,
            "CC-LEG",
            FlightVisibility::Public,
            true,
            now,
        )
        .await
        .unwrap();
    let (_row, token) = db.create_flight_share(&legacy_id, &fid).await.unwrap();
    (legacy_id, fid, token)
}

fn test_router(data_dir: &std::path::Path, db: ClubDb, password: &str) -> AppState {
    let live = std::sync::Arc::new(crate::live::LiveEngine::new(data_dir));
    let (mut state, _rx) = crate::app_state(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../dist"),
        Auth::with_password(password),
        crate::device_auth::DeviceAuth::empty(),
        live,
        Store::new(data_dir),
    );
    state.club = Some(db);
    state
}

#[serial]
#[tokio::test]
async fn migrate_legacy_dry_run_and_apply_idempotent() {
    let db = match ClubDb::open_test().await {
        Ok(db) => db,
        Err(DbConnectError::MissingUrl) => {
            eprintln!("skipping migrate_legacy_tests: DATABASE_URL unset");
            return;
        }
        Err(e) => panic!("db: {e}"),
    };
    let club_id = db.default_club_id().await.unwrap();
    let slug: String = sqlx::query_scalar("SELECT slug FROM clubs WHERE id = $1")
        .bind(&club_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();

    let (_legacy, flight_id, share_token) = seed_legacy_owner(&db, &club_id).await;

    let data_dir = tempfile::tempdir().unwrap();
    let store = Store::new(data_dir.path());
    let track = include_str!("../../public/samples/vuelo5.gpx");
    store
        .write_flight(
            LEGACY_DEMO_SLUG,
            crate::store::WriteFlightBody {
                name: Some("Demo".into()),
                file_name: Some("vuelo5.gpx".into()),
                trim_start_ms: None,
                trim_end_ms: None,
                tags: Some(vec![]),
                track: Some(track.to_string()),
            },
        )
        .await
        .unwrap();

    let opts = LegacyMigrateOpts {
        email: FIXTURE_EMAIL.to_string(),
        club_slug: slug.clone(),
        dry_run: true,
    };
    let dry = run_legacy_migrate(&opts, data_dir.path())
        .await
        .expect("dry-run");
    assert!(dry.rows.get("flights.pilot_id").copied().unwrap_or(0) >= 1);
    assert!(
        dry.rows
            .get("disk_flight_owners.pending")
            .copied()
            .unwrap_or(0)
            >= 1
    );
    eprintln!("CI dry-run sample: {:?}", dry.rows);

    let mut apply = opts.clone();
    apply.dry_run = false;
    run_legacy_migrate(&apply, data_dir.path())
        .await
        .expect("apply");
    assert_eq!(
        db.share_flight_id(&share_token).await.as_deref(),
        Some(flight_id.as_str())
    );

    run_legacy_migrate(&apply, data_dir.path())
        .await
        .expect("second apply");

    let app = build_router(test_router(data_dir.path(), db.clone(), ""));
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/flights/{DEMO_FLIGHT_SLUG}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/flights/{LEGACY_DEMO_SLUG}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.oneshot(req).await.unwrap().status(),
        StatusCode::PERMANENT_REDIRECT
    );

    let app = build_router(test_router(data_dir.path(), db.clone(), "secret"));
    let login = Request::builder()
        .method("POST")
        .uri("/login")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"password":"secret"}"#))
        .unwrap();
    assert_eq!(app.oneshot(login).await.unwrap().status(), StatusCode::OK);

    let gate_off = build_router(test_router(data_dir.path(), db.clone(), ""));
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/flights/{DEMO_FLIGHT_SLUG}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        gate_off.oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );
}

#[test]
fn cli_requires_email() {
    let err = parse_cli_args(&["--club".to_string(), "cas".to_string()]).unwrap_err();
    assert!(err.to_string().contains("--email"));
}
