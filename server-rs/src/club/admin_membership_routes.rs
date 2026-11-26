//! Club-scoped admin membership management (multi-admin).

use crate::club::{
    auth,
    db::now_ms,
    db_memberships_admin::MembershipAdminError,
    routes::{club_db, viewer},
    visibility::{PilotStatus, Viewer},
};
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

async fn require_active_club_admin(
    db: &crate::club::ClubDb,
    viewer: &Viewer,
) -> Result<(String, String), (StatusCode, Json<serde_json::Value>)> {
    let Viewer::Pilot {
        id,
        status: PilotStatus::Approved,
        ..
    } = viewer
    else {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        ));
    };
    let club_id = db.pilot_club_id(id).await.map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
    })?;
    if db
        .require_active_club_admin_role(id, &club_id)
        .await
        .is_err()
    {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        ));
    }
    Ok((club_id, id.to_string()))
}

async fn list_memberships_admin(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "memberships": [] })).into_response();
    };
    let Ok((club_id, _)) = require_active_club_admin(db, &v).await else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let rows = db.list_club_memberships_admin(&club_id).await;
    Json(json!({
        "memberships": rows.iter().map(|m| json!({
            "id": m.id,
            "pilotId": m.pilot_id,
            "name": m.name,
            "email": m.email,
            "role": m.role,
            "status": m.status,
        })).collect::<Vec<_>>(),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct InviteAdminBody {
    email: String,
    #[serde(default)]
    role: Option<String>,
}

async fn invite_admin(
    State(state): State<AppState>,
    headers: HeaderMap,
    client_ip: crate::ClientIp,
    Json(body): Json<InviteAdminBody>,
) -> impl IntoResponse {
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return (status, Json(json!({ "error": "csrf" }))).into_response();
    }
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return Json(json!({ "ok": true })).into_response();
    };
    let Ok((club_id, inviter)) = require_active_club_admin(db, &v).await else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    let (role, purpose, template) = match body.role.as_deref().map(str::trim) {
        None | Some("admin") => (
            "admin",
            crate::club::db_memberships_admin::ADMIN_INVITE_PURPOSE,
            "adminInvite",
        ),
        Some("pilot") => (
            "pilot",
            crate::club::db_memberships_admin::PILOT_INVITE_PURPOSE,
            "pilotInvite",
        ),
        Some(_) => return Json(json!({ "ok": true })).into_response(),
    };
    let email = body.email.trim();
    if email.is_empty() || !email.contains('@') {
        return Json(json!({ "ok": true })).into_response();
    }
    let now = now_ms();
    let ip = client_ip.0;
    if db.magic_send_allowed(email, &ip, now).await && state.mailer.allow_send(email, &ip) {
        if let Ok(token) = db
            .invite_membership(&club_id, email, &inviter, role, now)
            .await
        {
            let _ = db.log_magic_send(email, &ip, now).await;
            let url = format!(
                "{}/api/account/auth/magic?purpose={purpose}&token={token}",
                state.mailer.public_url.trim_end_matches('/')
            );
            let loc = auth::parse_fv_locale_cookie(&headers).unwrap_or_else(|| "es".to_string());
            let _ = state
                .mailer
                .send_localized(email, &loc, template, &[("minutes", "15"), ("url", &url)])
                .await;
        }
    }
    Json(json!({ "ok": true })).into_response()
}

fn map_membership_err(err: MembershipAdminError) -> (StatusCode, Json<serde_json::Value>) {
    match err {
        MembershipAdminError::LastAdmin => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "last_admin" })),
        ),
        MembershipAdminError::NotFound => {
            (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" })))
        }
        MembershipAdminError::InvalidRole => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_role" })),
        ),
        MembershipAdminError::ClubSuspended | MembershipAdminError::ClubClosed => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": err.message() })),
        ),
        MembershipAdminError::Db => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        ),
    }
}

async fn promote_membership(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return (status, Json(json!({ "error": "csrf" }))).into_response();
    }
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok((club_id, _)) = require_active_club_admin(db, &v).await else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    match db.set_membership_role(&club_id, &id, "admin").await {
        Ok(()) => Json(json!({ "ok": true, "role": "admin" })).into_response(),
        Err(e) => map_membership_err(e).into_response(),
    }
}

async fn demote_membership(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return (status, Json(json!({ "error": "csrf" }))).into_response();
    }
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok((club_id, _)) = require_active_club_admin(db, &v).await else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    match db.set_membership_role(&club_id, &id, "staff").await {
        Ok(()) => Json(json!({ "ok": true, "role": "staff" })).into_response(),
        Err(e) => map_membership_err(e).into_response(),
    }
}

async fn remove_membership(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(status) = super::csrf::cookie_post_allowed(&headers) {
        return (status, Json(json!({ "error": "csrf" }))).into_response();
    }
    let v = viewer(&state, &headers).await;
    let Some(db) = club_db(&state) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok((club_id, _)) = require_active_club_admin(db, &v).await else {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        )
            .into_response();
    };
    match db.remove_admin_membership(&club_id, &id).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => map_membership_err(e).into_response(),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/memberships", get(list_memberships_admin))
        .route("/api/admin/memberships/invite", post(invite_admin))
        .route(
            "/api/admin/memberships/{id}/promote",
            post(promote_membership),
        )
        .route(
            "/api/admin/memberships/{id}/demote",
            post(demote_membership),
        )
        .route(
            "/api/admin/memberships/{id}/remove",
            post(remove_membership),
        )
}
