//! Self-serve club signup and public club listing.

use crate::club::{
    db::now_ms,
    db_club_lifecycle::{validate_icao, ClubSignupError},
    routes::club_db,
    slug::normalize_slug,
};
use crate::{AppState, ClientIp};
use axum::{
    extract::State,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct ClubSignupBody {
    name: String,
    slug: String,
    email: String,
    #[serde(default)]
    aerodrome_icao: Option<String>,
    #[serde(default)]
    website: Option<String>,
}

async fn post_club_signup(
    State(state): State<AppState>,
    client_ip: ClientIp,
    Json(body): Json<ClubSignupBody>,
) -> impl IntoResponse {
    if body
        .website
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
    {
        return Json(json!({ "ok": true })).into_response();
    }
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if !state
        .device_register_throttle
        .allow_up_to(&client_ip.0, now_secs, 10)
    {
        return Json(json!({ "ok": true })).into_response();
    }
    let Some(db) = club_db(&state) else {
        return Json(json!({ "ok": true })).into_response();
    };
    let now = now_ms();
    let email = body.email.trim();
    let ip = client_ip.0;
    if !db.magic_send_allowed(email, &ip, now).await {
        return Json(json!({ "ok": true })).into_response();
    }
    let icao = body.aerodrome_icao.as_deref();
    if icao.is_some_and(|s| !s.trim().is_empty() && !validate_icao(s)) {
        return Json(json!({ "ok": true })).into_response();
    }
    let club_name = body.name.trim();
    match db
        .signup_club(club_name, &normalize_slug(&body.slug), email, icao, now)
        .await
    {
        Ok((_, token)) => {
            let _ = db.log_magic_send(email, &ip, now).await;
            let url = format!(
                "{}/api/club/auth/magic?token={token}",
                state.mailer.public_url.trim_end_matches('/')
            );
            let _ = state
                .mailer
                .send_localized(
                    email,
                    "es",
                    "verifyClub",
                    &[("minutes", "15"), ("url", &url), ("name", club_name)],
                )
                .await;
            Json(json!({ "ok": true })).into_response()
        }
        Err(ClubSignupError::InvalidSlug) | Err(ClubSignupError::InvalidEmail) => {
            Json(json!({ "ok": true })).into_response()
        }
        Err(ClubSignupError::InvalidIcao) => Json(json!({ "ok": true })).into_response(),
        Err(ClubSignupError::Db) => Json(json!({ "ok": true })).into_response(),
    }
}

async fn get_public_clubs(State(state): State<AppState>) -> impl IntoResponse {
    let Some(db) = club_db(&state) else {
        return Json(json!({ "clubs": [] })).into_response();
    };
    let clubs: Vec<_> = db
        .list_public_clubs()
        .await
        .into_iter()
        .map(|(slug, name)| json!({ "slug": slug, "name": name }))
        .collect();
    Json(json!({ "clubs": clubs })).into_response()
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/clubs/signup", post(post_club_signup))
        .route("/api/clubs/public", get(get_public_clubs))
}
