//! Postgres integration tests for `migrate-legacy` (requires `DATABASE_URL`).

use crate::{
    auth::Auth,
    build_router,
    club::{hash_password, new_id, ClubDb, DbConnectError, FlightVisibility},
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
use tower::Service;

const FIXTURE_EMAIL: &str = "legacy-migrate-fixture@test.invalid";
const LEGACY_EMAIL: &str = "legacy-owner-fixture@test.invalid";
const DECOY_EMAIL: &str = "unverified-decoy-fixture@test.invalid";

fn migrate_opts(club_slug: &str, dry_run: bool, source_email: &str) -> LegacyMigrateOpts {
    LegacyMigrateOpts {
        email: FIXTURE_EMAIL.to_string(),
        club_slug: club_slug.to_string(),
        dry_run,
        source_pilot_ids: Vec::new(),
        source_emails: vec![source_email.to_string()],
    }
}

async fn seed_legacy_owner(db: &ClubDb, club_id: &str) -> (String, String, String) {
    let hash = hash_password("password12").unwrap();
    let legacy_id = new_id();
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
    db.add_checkout(&legacy_id, "C152", Some("CC-LEG"), None)
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
    let (_row, token) = db
        .create_flight_share(&legacy_id, &fid)
        .await
        .expect("create_flight_share");
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

    let opts = migrate_opts(&slug, true, LEGACY_EMAIL);
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

    let mut app = build_router(test_router(data_dir.path(), db.clone(), ""));
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/flights/{DEMO_FLIGHT_SLUG}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(app.call(req).await.unwrap().status(), StatusCode::OK);
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/flights/{LEGACY_DEMO_SLUG}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.call(req).await.unwrap().status(),
        StatusCode::PERMANENT_REDIRECT
    );

    let mut app = build_router(test_router(data_dir.path(), db.clone(), "secret"));
    let login = Request::builder()
        .method("POST")
        .uri("/login")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"password":"secret"}"#))
        .unwrap();
    assert_eq!(app.call(login).await.unwrap().status(), StatusCode::OK);

    let mut gate_off = build_router(test_router(data_dir.path(), db, ""));
    let req = Request::builder()
        .method("GET")
        .uri(format!("/api/flights/{DEMO_FLIGHT_SLUG}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(gate_off.call(req).await.unwrap().status(), StatusCode::OK);
}

#[test]
fn cli_requires_email() {
    let err = parse_cli_args(&["--club".to_string(), "cas".to_string()]).unwrap_err();
    assert!(err.to_string().contains("--email"));
}

#[test]
fn cli_requires_explicit_source() {
    let err = parse_cli_args(&[
        "--email".to_string(),
        FIXTURE_EMAIL.to_string(),
        "--club".to_string(),
        "cas".to_string(),
    ])
    .unwrap_err();
    assert!(err.to_string().contains("--source"));
}

async fn seed_second_club_pilot_with_flight(db: &ClubDb, slug: &str) -> (String, String, String) {
    let club_id = new_id();
    let now = crate::club::now_ms();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at, latitude, longitude, show_public_tracks)
         VALUES ($1, 'Other', $2, $3, 'live', $3, -33.0, -70.0, TRUE)",
    )
    .bind(&club_id)
    .bind(slug)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let hash = hash_password("password12").unwrap();
    let pilot_id = new_id();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Other', $3, '', 'approved', 'pilot', $4, $5)",
    )
    .bind(&pilot_id)
    .bind(&club_id)
    .bind("other-club-pilot@test.invalid")
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.upsert_type(&club_id, "C152").await.unwrap();
    db.upsert_aircraft(&club_id, "CC-OTH", "C152", true)
        .await
        .unwrap();
    db.add_checkout(&pilot_id, "C152", Some("CC-OTH"), None)
        .await
        .unwrap();
    let (dev, _) = db.create_device(&pilot_id, "tablet").await.unwrap();
    db.issue_open_checkin(&pilot_id, &dev, &club_id, now)
        .await
        .unwrap();
    let flight_id = db
        .start_flight(
            &pilot_id,
            &dev,
            "CC-OTH",
            FlightVisibility::Private,
            false,
            now,
        )
        .await
        .unwrap();
    (club_id, pilot_id, flight_id)
}

#[serial]
#[tokio::test]
async fn migrate_legacy_does_not_touch_other_club_rows() {
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

    let (_legacy_id, cas_flight_id, _) = seed_legacy_owner(&db, &club_id).await;
    let slug_b = format!("other-club-{}", &new_id()[..8]);
    let (club_b, other_pilot, other_flight) =
        seed_second_club_pilot_with_flight(&db, &slug_b).await;

    let data_dir = tempfile::tempdir().unwrap();
    let opts = migrate_opts(&slug, false, LEGACY_EMAIL);
    run_legacy_migrate(&opts, data_dir.path())
        .await
        .expect("apply");

    let cas_owner: String = sqlx::query_scalar("SELECT pilot_id FROM flights WHERE id = $1")
        .bind(&cas_flight_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_ne!(cas_owner, other_pilot);

    let other_owner: String = sqlx::query_scalar("SELECT pilot_id FROM flights WHERE id = $1")
        .bind(&other_flight)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(other_owner, other_pilot);

    let target_id: String =
        sqlx::query_scalar("SELECT id FROM pilots WHERE lower(email) = lower($1)")
            .bind(FIXTURE_EMAIL)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(cas_owner, target_id);

    let legacy_still: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pilots WHERE club_id = $1 AND lower(email) = lower($2)",
    )
    .bind(&club_id)
    .bind(LEGACY_EMAIL)
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(legacy_still, 1);

    let other_club_pilots: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pilots WHERE club_id = $1")
            .bind(&club_b)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(other_club_pilots, 1);
}

async fn seed_unverified_decoy_with_flight(db: &ClubDb, club_id: &str) -> (String, String) {
    let hash = hash_password("password12").unwrap();
    let pilot_id = new_id();
    let now = crate::club::now_ms();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at)
         VALUES ($1, $2, 'Decoy', $3, '', 'approved', 'pilot', $4, $5)",
    )
    .bind(&pilot_id)
    .bind(club_id)
    .bind(DECOY_EMAIL)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    db.upsert_type(club_id, "C172").await.unwrap();
    db.upsert_aircraft(club_id, "CC-DEC", "C172", true)
        .await
        .unwrap();
    db.add_checkout(&pilot_id, "C172", Some("CC-DEC"), None)
        .await
        .unwrap();
    let (dev, _) = db.create_device(&pilot_id, "decoy").await.unwrap();
    db.issue_open_checkin(&pilot_id, &dev, club_id, now)
        .await
        .unwrap();
    let flight_id = db
        .start_flight(
            &pilot_id,
            &dev,
            "CC-DEC",
            FlightVisibility::Private,
            false,
            now,
        )
        .await
        .unwrap();
    (pilot_id, flight_id)
}

#[serial]
#[tokio::test]
async fn migrate_legacy_leaves_unnamed_unverified_pilot_untouched() {
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

    let (_legacy_id, legacy_flight, _) = seed_legacy_owner(&db, &club_id).await;
    let (decoy_id, decoy_flight) = seed_unverified_decoy_with_flight(&db, &club_id).await;

    let data_dir = tempfile::tempdir().unwrap();
    let opts = migrate_opts(&slug, false, LEGACY_EMAIL);
    run_legacy_migrate(&opts, data_dir.path())
        .await
        .expect("apply");

    let decoy_owner: String = sqlx::query_scalar("SELECT pilot_id FROM flights WHERE id = $1")
        .bind(&decoy_flight)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(decoy_owner, decoy_id);

    let target_id: String =
        sqlx::query_scalar("SELECT id FROM pilots WHERE lower(email) = lower($1)")
            .bind(FIXTURE_EMAIL)
            .fetch_one(&db.pool)
            .await
            .unwrap();
    let legacy_owner: String = sqlx::query_scalar("SELECT pilot_id FROM flights WHERE id = $1")
        .bind(&legacy_flight)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(legacy_owner, target_id);
}
