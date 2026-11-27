//! Club slug rename, history redirects, and check-in token stability.

use crate::club::auth::{hash_password, new_id, set_session_cookie};
use crate::club::db_club_lifecycle::ClubSignupError;
use crate::club::slug::validate_slug;
use crate::club::{db::now_ms, ClubDb};
use crate::store::WriteFlightBody;
use crate::{
    auth::Auth, build_router, device_auth::DeviceAuth, live::LiveEngine, store::Store, AppState,
};
use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use serial_test::serial;
use std::sync::Arc;
use tower::Service;

struct SlugFx {
    router: axum::Router,
    db: ClubDb,
    store: Store,
    club_a: String,
    club_b: String,
    admin_a_cookie: String,
    admin_b_cookie: String,
    pilot_cookie: String,
    staff_cookie: String,
    superadmin_cookie: String,
}

async fn session_for_pilot(db: &ClubDb, club_id: &str, email: &str, role: &str) -> String {
    let now = now_ms();
    let pid = new_id();
    let hash = hash_password("password12").unwrap();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at, email_verified_at)
         VALUES ($1, $2, $3, $4, '', 'approved', $5, $6, $7, $7)",
    )
    .bind(&pid)
    .bind(club_id)
    .bind(email)
    .bind(email)
    .bind(role)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    if role == "admin" {
        sqlx::query(
            "INSERT INTO club_memberships (id, pilot_id, club_id, role, status, created_at)
             VALUES ($1, $2, $3, 'admin', 'active', $4)",
        )
        .bind(new_id())
        .bind(&pid)
        .bind(club_id)
        .bind(now)
        .execute(&db.pool)
        .await
        .unwrap();
    }
    let sess = db.create_pilot_session(&pid, now).await.unwrap();
    set_session_cookie(&sess, false)
}

async fn slug_fixture() -> SlugFx {
    let data_dir = tempfile::tempdir().unwrap();
    let db = ClubDb::open_test().await.unwrap();
    let store = Store::new(data_dir.path());
    let now = now_ms();
    let club_a = db.default_club_id().await.unwrap();
    let slug_b = format!("club-b-{}", &new_id()[..8]);
    let club_b = new_id();
    sqlx::query(
        "INSERT INTO clubs (id, name, slug, created_at, status, email_verified_at, latitude, longitude, show_public_tracks)
         VALUES ($1, 'Club B', $2, $3, 'live', $3, -33.4489, -70.6693, TRUE)",
    )
    .bind(&club_b)
    .bind(&slug_b)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let (video_queue, _) = tokio::sync::mpsc::channel(8);
    let state = AppState {
        dist_dir: data_dir.path().join("dist"),
        auth: Auth::with_password("x"),
        device_auth: DeviceAuth::empty(),
        live: Arc::new(LiveEngine::new(data_dir.path())),
        store: store.clone(),
        club: Some(db.clone()),
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
        superadmin_emails: std::collections::HashSet::new(),
    };
    let router = build_router(state);
    let admin_a_cookie = session_for_pilot(&db, &club_a, "slug-adm-a@test", "admin").await;
    let boot_slug = db.club_slug(&club_a).await.unwrap_or_default();
    if boot_slug == "default" || validate_slug(&boot_slug).is_err() {
        let fixed = format!("slug-a-{}", &new_id()[..8]);
        db.rename_club_slug(&club_a, &fixed, None, now_ms())
            .await
            .expect("seed slug");
    }
    let admin_b_cookie = session_for_pilot(&db, &club_b, "slug-adm-b@test", "admin").await;
    let pilot_cookie = session_for_pilot(&db, &club_a, "slug-pilot@test", "pilot").await;
    let staff_cookie = session_for_pilot(&db, &club_a, "slug-staff@test", "staff").await;
    let sa_pid = new_id();
    let hash = hash_password("password12").unwrap();
    sqlx::query(
        "INSERT INTO pilots (id, club_id, name, email, license_no, status, role, password_hash, created_at, email_verified_at, global_role)
         VALUES ($1, $2, 'SA', 'slug-sa@test', '', 'approved', 'pilot', $3, $4, $4, 'superadmin')",
    )
    .bind(&sa_pid)
    .bind(&club_a)
    .bind(&hash)
    .bind(now)
    .execute(&db.pool)
    .await
    .unwrap();
    let sa_sess = db.create_pilot_session(&sa_pid, now).await.unwrap();
    let superadmin_cookie = set_session_cookie(&sa_sess, false);
    SlugFx {
        router,
        db,
        store,
        club_a,
        club_b,
        admin_a_cookie,
        admin_b_cookie,
        pilot_cookie,
        staff_cookie,
        superadmin_cookie,
    }
}

async fn call(
    router: &mut axum::Router,
    req: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, Option<String>, String) {
    let res = Service::call(router, req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let loc = headers
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap_or_default();
    let text = String::from_utf8(body.to_vec()).unwrap_or_default();
    (status, headers, loc, text)
}

#[tokio::test]
#[serial]
async fn club_slug_rename_redirects_single_hop_after_two_renames() {
    let mut fx = slug_fixture().await;
    let old0 = db_slug(&fx.db, &fx.club_a).await;
    let mid = format!("{old0}-mid-{}", &new_id()[..6]);
    let fin = format!("{old0}-fin-{}", &new_id()[..6]);
    rename_ok(&mut fx, &mid).await;
    rename_ok(&mut fx, &fin).await;
    let paths = [
        format!("/c/{old0}/registro"),
        format!("/c/{old0}/screen?kiosk=1"),
        format!("/c/{old0}"),
        format!("/api/clubs/{old0}"),
    ];
    for path in paths {
        let (status, headers, loc, _) = call(
            &mut fx.router,
            Request::get(&path).body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FOUND, "path {path}");
        assert_eq!(
            headers
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("no-store")
        );
        let loc = loc.unwrap();
        assert!(loc.starts_with('/') && !loc.starts_with("//"));
        assert!(
            loc.contains(&fin),
            "expected final slug in {loc} for {path}"
        );
        assert!(!loc.contains(&mid), "must not chain through mid");
    }
    let (st, headers, loc, _) = call(
        &mut fx.router,
        Request::get(format!("/public-map.html?club={old0}&x=1"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::FOUND);
    assert_eq!(
        headers
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let loc = loc.unwrap();
    assert!(loc.contains(&format!("club={fin}")));
    assert!(loc.contains("x=1"));
}

#[tokio::test]
#[serial]
async fn club_slug_reclaim_redirect_does_not_loop() {
    let mut fx = slug_fixture().await;
    let start = db_slug(&fx.db, &fx.club_a).await;
    let mid = format!("{start}-loop-mid");
    rename_ok(&mut fx, &mid).await;
    rename_ok(&mut fx, &start).await;
    let (status, _, loc, _) = call(
        &mut fx.router,
        Request::get(format!("/c/{mid}/registro"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FOUND);
    let loc = loc.unwrap();
    assert!(loc.contains(&format!("/c/{start}/")));
    let (status2, _, _, _) = call(
        &mut fx.router,
        Request::get(format!("/c/{start}/registro"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_ne!(status2, StatusCode::FOUND);
}

#[tokio::test]
#[serial]
async fn club_slug_query_redirect_open_redirect_blocked() {
    let mut fx = slug_fixture().await;
    let old = db_slug(&fx.db, &fx.club_a).await;
    let mid = format!("{old}-sec-mid");
    rename_ok(&mut fx, &mid).await;
    for path in ["/\\evil.com", "//evil.com/x", "/x\\y"] {
        let (status, _, loc, _) = call(
            &mut fx.router,
            Request::get(format!("{path}?club={old}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_ne!(status, StatusCode::FOUND, "path {path}");
        assert!(loc.is_none());
    }
    let (status, _, loc, _) = call(
        &mut fx.router,
        Request::get(format!("/public-map.html?club={}", old.to_uppercase()))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    if status == StatusCode::FOUND {
        let loc = loc.unwrap();
        assert!(!loc.starts_with("//"));
        assert!(
            loc.contains(&mid)
                || loc.contains(&format!("club={}", db_slug(&fx.db, &fx.club_a).await))
        );
    }
}

async fn db_slug(db: &ClubDb, club_id: &str) -> String {
    db.club_slug(club_id).await.unwrap()
}

async fn post_slug_path(
    fx: &mut SlugFx,
    path: &str,
    cookie: Option<&str>,
    slug: &str,
) -> StatusCode {
    let body = format!(r#"{{"slug":"{slug}"}}"#);
    let mut req = Request::post(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header("sec-fetch-site", "same-origin")
        .body(Body::from(body))
        .unwrap();
    if let Some(c) = cookie {
        req.headers_mut().insert(header::COOKIE, c.parse().unwrap());
    }
    let (status, _, _, _) = call(&mut fx.router, req).await;
    status
}

async fn post_slug(fx: &mut SlugFx, cookie: Option<&str>, slug: &str) -> StatusCode {
    post_slug_path(fx, "/api/admin/club/slug", cookie, slug).await
}

async fn rename_ok(fx: &mut SlugFx, slug: &str) {
    let body = format!(r#"{{"slug":"{slug}"}}"#);
    let (status, _, _, text) = call(
        &mut fx.router,
        Request::post("/api/admin/club/slug")
            .header(header::COOKIE, &fx.admin_a_cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .header("sec-fetch-site", "same-origin")
            .body(Body::from(body))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{text}");
}

#[tokio::test]
#[serial]
async fn flight_slugs_never_redirect_as_club_history() {
    let mut fx = slug_fixture().await;
    fx.store
        .write_flight(
            "vuelo-5",
            WriteFlightBody {
                name: Some("Vuelo 5".into()),
                file_name: None,
                trim_start_ms: None,
                trim_end_ms: None,
                tags: None,
                track: Some("LINESTRING()".into()),
            },
        )
        .await
        .unwrap();
    let flight_slug = format!("flt-{}", &new_id()[..6]);
    fx.store
        .write_flight(
            &flight_slug,
            WriteFlightBody {
                name: Some("t".into()),
                file_name: None,
                trim_start_ms: None,
                trim_end_ms: None,
                tags: None,
                track: Some("LINESTRING()".into()),
            },
        )
        .await
        .unwrap();
    let admin_cookie = fx.admin_a_cookie.clone();
    assert_eq!(
        post_slug(&mut fx, Some(&admin_cookie), "vuelo-5").await,
        StatusCode::BAD_REQUEST
    );
    let held = "vuelo-5".to_string();
    let start = db_slug(&fx.db, &fx.club_a).await;
    let now = now_ms();
    fx.db
        .rename_club_slug(&fx.club_a, &held, None, now)
        .await
        .unwrap();
    let away = format!("{held}-away-{}", &new_id()[..4]);
    fx.db
        .rename_club_slug(&fx.club_a, &away, None, now + 1)
        .await
        .unwrap();
    let _ = start;
    for path in ["/vuelo-5".to_string(), format!("/{flight_slug}")] {
        let (status, _, loc, _) = call(
            &mut fx.router,
            Request::get(&path).body(Body::empty()).unwrap(),
        )
        .await;
        assert_ne!(status, StatusCode::FOUND, "path {path}");
        assert!(loc.is_none());
    }
}

#[tokio::test]
#[serial]
async fn club_slug_history_blocks_cross_club_signup_and_rename() {
    let mut fx = slug_fixture().await;
    let old = db_slug(&fx.db, &fx.club_a).await;
    let taken = format!("{old}-hist-{}", &new_id()[..6]);
    rename_ok(&mut fx, &taken).await;
    let now = now_ms();
    let signup = fx
        .db
        .signup_club("Other", &old, "other-slug@test", None, now)
        .await;
    assert!(matches!(signup, Err(ClubSignupError::SlugTaken)));
    let cookie_b = fx.admin_b_cookie.clone();
    rename_conflict(&mut fx, &old, &cookie_b).await;
}

#[tokio::test]
#[serial]
async fn confusable_slug_rejected_as_taken() {
    let mut fx = slug_fixture().await;
    let base = db_slug(&fx.db, &fx.club_b).await;
    let lookalike = base.replacen('l', "1", 1);
    if lookalike == base {
        return;
    }
    let admin = fx.admin_a_cookie.clone();
    let status = post_slug(&mut fx, Some(&admin), &lookalike).await;
    assert_eq!(status, StatusCode::CONFLICT);
}

async fn rename_conflict(fx: &mut SlugFx, slug: &str, cookie: &str) {
    let body = format!(r#"{{"slug":"{slug}"}}"#);
    let (status, _, _, _) = call(
        &mut fx.router,
        Request::post("/api/admin/club/slug")
            .header(header::COOKIE, cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .header("sec-fetch-site", "same-origin")
            .body(Body::from(body))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
#[serial]
async fn club_slug_same_club_reclaims_old_slug() {
    let mut fx = slug_fixture().await;
    let start = db_slug(&fx.db, &fx.club_a).await;
    let mid = format!("{start}-reclaim-mid");
    rename_ok(&mut fx, &mid).await;
    rename_ok(&mut fx, &start).await;
    assert!(fx.db.club_id_for_slug(&start).await.is_some());
    let hist: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM club_slug_history WHERE old_slug = $1")
            .bind(&start)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(hist, 0);
}

#[tokio::test]
#[serial]
async fn club_slug_rename_authz_matrix() {
    let mut fx = slug_fixture().await;
    let slug = format!("authz-{}", &new_id()[..6]);
    let club_a_before = db_slug(&fx.db, &fx.club_a).await;
    let pilot_cookie = fx.pilot_cookie.clone();
    let staff_cookie = fx.staff_cookie.clone();
    let admin_b_cookie = fx.admin_b_cookie.clone();
    let admin_a_cookie = fx.admin_a_cookie.clone();
    let superadmin_cookie = fx.superadmin_cookie.clone();
    let club_b_id = fx.club_b.clone();
    assert_eq!(
        post_slug(&mut fx, None, &slug).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_slug(&mut fx, Some(&pilot_cookie), &slug).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_slug(&mut fx, Some(&staff_cookie), &slug).await,
        StatusCode::FORBIDDEN
    );
    let slug_b_before = db_slug(&fx.db, &fx.club_b).await;
    assert_eq!(
        post_slug(&mut fx, Some(&admin_b_cookie), &format!("{slug}-b")).await,
        StatusCode::OK
    );
    assert_ne!(db_slug(&fx.db, &fx.club_b).await, slug_b_before);
    assert_eq!(db_slug(&fx.db, &fx.club_a).await, club_a_before);
    assert_eq!(
        post_slug(&mut fx, Some(&admin_a_cookie), &format!("{slug}-a")).await,
        StatusCode::OK
    );
    assert_eq!(
        post_slug_path(
            &mut fx,
            &format!("/api/superadmin/clubs/{club_b_id}/slug"),
            Some(&superadmin_cookie),
            &format!("{slug}-sa"),
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(
        post_slug_path(
            &mut fx,
            &format!("/api/superadmin/clubs/{club_b_id}/slug"),
            Some(&admin_a_cookie),
            "nope-slug",
        )
        .await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
#[serial]
async fn club_slug_rename_csrf_cross_site_rejected() {
    let mut fx = slug_fixture().await;
    let body = r#"{"slug":"csrf-slug-test"}"#;
    let (status, _, _, text) = call(
        &mut fx.router,
        Request::post("/api/admin/club/slug")
            .header(header::COOKIE, &fx.admin_a_cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .header("sec-fetch-site", "cross-site")
            .body(Body::from(body))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(text.contains("csrf"));
}

#[tokio::test]
#[serial]
async fn club_slug_rename_requires_json_content_type() {
    let mut fx = slug_fixture().await;
    let (status, _, _, _) = call(
        &mut fx.router,
        Request::post("/api/admin/club/slug")
            .header(header::COOKIE, &fx.admin_a_cookie)
            .header(header::CONTENT_TYPE, "text/plain")
            .header("sec-fetch-site", "same-origin")
            .body(Body::from(r#"{"slug":"x"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
#[serial]
async fn suspended_club_cannot_rename() {
    let mut fx = slug_fixture().await;
    let now = now_ms();
    let before = db_slug(&fx.db, &fx.club_a).await;
    fx.db.set_club_suspended(&fx.club_a, now).await.unwrap();
    let slug = format!("susp-{}", &new_id()[..6]);
    let admin = fx.admin_a_cookie.clone();
    let status = post_slug(&mut fx, Some(&admin), &slug).await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
        "suspended club rename must be refused, got {status}"
    );
    assert_eq!(db_slug(&fx.db, &fx.club_a).await, before);
}

#[tokio::test]
#[serial]
async fn checkin_qr_token_minted_before_rename_still_valid() {
    let mut fx = slug_fixture().await;
    let now = now_ms();
    let token = fx.db.mint_checkin_qr_token(&fx.club_a, now).await.unwrap();
    let old_slug = db_slug(&fx.db, &fx.club_a).await;
    let new_slug = format!("{old_slug}-qr-{}", &new_id()[..6]);
    rename_ok(&mut fx, &new_slug).await;
    let (status, _, _, text) = call(
        &mut fx.router,
        Request::get(format!("/api/c/{old_slug}/q/{token}/landing"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(
        status == StatusCode::OK || status == StatusCode::FOUND,
        "{status} {text}"
    );
    assert!(fx.db.club_id_for_slug_any(&old_slug).await.as_deref() == Some(fx.club_a.as_str()));
    let token_pre = fx
        .db
        .mint_checkin_qr_token(&fx.club_a, now - 1000)
        .await
        .unwrap();
    rename_ok(&mut fx, &format!("{new_slug}-again")).await;
    assert!(
        crate::club::checkin::parse_checkin_token_club_id(&token_pre).as_deref()
            == Some(fx.club_a.as_str())
    );
    let (st, _, _, _) = call(
        &mut fx.router,
        Request::post("/api/checkin")
            .header(header::COOKIE, &fx.pilot_cookie)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(format!(r#"{{"token":"{token_pre}"}}"#)))
            .unwrap(),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
}

#[tokio::test]
#[serial]
async fn club_slug_rename_isolation_between_clubs() {
    let mut fx = slug_fixture().await;
    let b_before = db_slug(&fx.db, &fx.club_b).await;
    let a_slug = db_slug(&fx.db, &fx.club_a).await;
    rename_ok(&mut fx, &format!("{a_slug}-iso-a")).await;
    assert_eq!(db_slug(&fx.db, &fx.club_b).await, b_before);
    let hist_b: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM club_slug_history WHERE club_id = $1")
            .bind(&fx.club_b)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(hist_b, 0);
}

#[tokio::test]
#[serial]
async fn concurrent_renames_to_same_target_slug_one_wins() {
    let fx = slug_fixture().await;
    let target = format!("race-{}", &new_id()[..8]);
    let a_slug = db_slug(&fx.db, &fx.club_a).await;
    let b_slug = db_slug(&fx.db, &fx.club_b).await;
    let db_a = fx.db.clone();
    let db_b = fx.db.clone();
    let club_a = fx.club_a.clone();
    let club_b = fx.club_b.clone();
    let now = now_ms();
    let (ra, rb) = tokio::join!(
        db_a.rename_club_slug(&club_a, &target, None, now),
        db_b.rename_club_slug(&club_b, &target, None, now),
    );
    let oks = [ra.is_ok(), rb.is_ok()];
    assert_eq!(oks.iter().filter(|&&x| x).count(), 1);
    let in_registry: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM club_slug_registry WHERE slug = $1")
            .bind(&target)
            .fetch_one(&fx.db.pool)
            .await
            .unwrap();
    assert_eq!(in_registry, 1);
    let _ = (a_slug, b_slug);
}

#[tokio::test]
#[serial]
async fn concurrent_renames_same_club_preserves_history() {
    let fx = slug_fixture().await;
    let start = db_slug(&fx.db, &fx.club_a).await;
    let target = format!("{start}-par-same");
    let db1 = fx.db.clone();
    let db2 = fx.db.clone();
    let club = fx.club_a.clone();
    let now = now_ms();
    let (r1, r2) = tokio::join!(
        db1.rename_club_slug(&club, &target, None, now),
        db2.rename_club_slug(&club, &target, None, now),
    );
    assert!(r1.is_ok());
    assert!(r2.is_err());
    let hist: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM club_slug_history WHERE club_id = $1 AND old_slug = $2",
    )
    .bind(&fx.club_a)
    .bind(&start)
    .fetch_one(&fx.db.pool)
    .await
    .unwrap();
    assert_eq!(hist, 1);
    assert!(fx.db.club_slug_globally_taken(&start, None).await.unwrap());
}
