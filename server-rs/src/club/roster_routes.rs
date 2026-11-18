//! Club admin roster API (CRUD, matrix, CSV import, audit).

use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::json;

use crate::club::{
    csv_util::{parse_csv_bytes, CsvParseError},
    db::ClubDb,
    db_roster_pg::RosterError,
    routes::{club_db, require_club_admin_scope, viewer},
    visibility::Viewer,
};
use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/pilots", post(admin_create_pilot))
        .route(
            "/api/admin/pilots/{id}",
            axum::routing::patch(admin_patch_pilot).delete(admin_delete_pilot),
        )
        .route(
            "/api/admin/aircraft/{id}",
            axum::routing::patch(admin_patch_aircraft).delete(admin_delete_aircraft),
        )
        .route("/api/admin/matrix", get(admin_matrix))
        .route("/api/admin/checkouts/toggle", post(admin_checkout_toggle))
        .route("/api/admin/checkouts/bulk", post(admin_checkout_bulk))
        .route("/api/admin/checkouts/{id}", delete(admin_delete_checkout))
        .route(
            "/api/admin/import/{kind}/preview",
            post(admin_import_preview),
        )
        .route("/api/admin/import/{kind}/commit", post(admin_import_commit))
        .route(
            "/api/admin/import/templates/{kind}",
            get(admin_import_template),
        )
        .route("/api/admin/audit", get(admin_audit_list))
}

#[derive(Deserialize)]
struct CreatePilotBody {
    name: String,
    email: String,
    #[serde(default)]
    license_no: String,
    #[serde(default = "default_approved")]
    status: String,
    #[serde(default)]
    role: String,
}

fn default_approved() -> String {
    "approved".into()
}

#[derive(Deserialize)]
struct PatchPilotBody {
    name: String,
    email: String,
    #[serde(default)]
    license_no: String,
    status: String,
}

#[derive(Deserialize)]
struct PatchAircraftBody {
    tail: String,
    #[serde(rename = "type")]
    type_code: String,
    #[serde(default = "default_true")]
    active: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize)]
struct ToggleCheckoutBody {
    pilot_id: String,
    #[serde(rename = "type")]
    type_code: String,
    tail: Option<String>,
    enable: bool,
    expires_at: Option<i64>,
}

#[derive(Deserialize)]
struct BulkCheckoutBody {
    pilot_ids: Vec<String>,
    #[serde(rename = "type")]
    type_code: String,
    tail: Option<String>,
    assign: bool,
    expires_at: Option<i64>,
}

#[derive(Deserialize)]
struct ImportCommitBody {
    batch_id: String,
}

#[derive(Deserialize)]
struct AuditQuery {
    #[serde(default)]
    entity_type: Option<String>,
    #[serde(default)]
    offset: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
}

fn roster_err(e: RosterError) -> (StatusCode, Json<serde_json::Value>) {
    match e {
        RosterError::NotFound => (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))),
        RosterError::EmailExists => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "email exists" })),
        ),
        RosterError::DuplicateTail => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "tail already exists" })),
        ),
        RosterError::InvalidTail => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid tail; use CC-XXX" })),
        ),
        RosterError::InvalidStatus | RosterError::InvalidEmail => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid request" })),
        ),
        RosterError::UnknownType => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unknown aircraft type" })),
        ),
        RosterError::UnknownTail => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unknown tail; tail must belong to your club" })),
        ),
        RosterError::PilotWrongClub | RosterError::BatchMismatch => {
            (StatusCode::FORBIDDEN, Json(json!({ "error": "forbidden" })))
        }
        RosterError::BatchNotFound | RosterError::BatchExpired => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "import batch not found or expired" })),
        ),
        RosterError::NothingToCommit => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "empty import" })),
        ),
        RosterError::Db(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        ),
    }
}

async fn admin_scope(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(String, String), (StatusCode, Json<serde_json::Value>)> {
    let v = viewer(state, headers).await;
    let Some(db) = club_db(state) else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "club disabled" })),
        ));
    };
    let club_id = require_club_admin_scope(db, &v).await?;
    let Viewer::Pilot { id, .. } = v else {
        return Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "admin only" })),
        ));
    };
    Ok((club_id, id))
}

fn admin_db(state: &AppState) -> Result<&ClubDb, (StatusCode, Json<serde_json::Value>)> {
    club_db(state).ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "club disabled" })),
    ))
}

async fn admin_create_pilot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreatePilotBody>,
) -> impl IntoResponse {
    let (club_id, admin_id) = match admin_scope(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let db = match admin_db(&state) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match db
        .admin_create_pilot(
            &club_id,
            &admin_id,
            &body.name,
            &body.email,
            &body.license_no,
            &body.status,
            &body.role,
        )
        .await
    {
        Ok(id) => Json(json!({ "id": id })).into_response(),
        Err(e) => roster_err(e).into_response(),
    }
}

async fn admin_patch_pilot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pilot_id): Path<String>,
    Json(body): Json<PatchPilotBody>,
) -> impl IntoResponse {
    let (club_id, admin_id) = match admin_scope(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let db = match admin_db(&state) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match db
        .admin_update_pilot(
            &club_id,
            &admin_id,
            &pilot_id,
            &body.name,
            &body.email,
            &body.license_no,
            &body.status,
        )
        .await
    {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => roster_err(e).into_response(),
    }
}

async fn admin_delete_pilot(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(pilot_id): Path<String>,
) -> impl IntoResponse {
    let (club_id, admin_id) = match admin_scope(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let db = match admin_db(&state) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match db.admin_remove_pilot(&club_id, &admin_id, &pilot_id).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => roster_err(e).into_response(),
    }
}

async fn admin_patch_aircraft(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(aircraft_id): Path<String>,
    Json(body): Json<PatchAircraftBody>,
) -> impl IntoResponse {
    let (club_id, admin_id) = match admin_scope(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let db = match admin_db(&state) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match db
        .admin_update_aircraft_by_id(
            &club_id,
            &admin_id,
            &aircraft_id,
            &body.tail,
            &body.type_code,
            body.active,
        )
        .await
    {
        Ok(id) => Json(json!({ "id": id })).into_response(),
        Err(e) => roster_err(e).into_response(),
    }
}

async fn admin_delete_aircraft(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(aircraft_id): Path<String>,
) -> impl IntoResponse {
    let (club_id, admin_id) = match admin_scope(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let db = match admin_db(&state) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match db
        .admin_remove_aircraft(&club_id, &admin_id, &aircraft_id)
        .await
    {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => roster_err(e).into_response(),
    }
}

async fn admin_matrix(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let (club_id, _admin_id) = match admin_scope(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let db = match admin_db(&state) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match db.auth_matrix(&club_id).await {
        Ok(m) => Json(m).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}

async fn admin_checkout_toggle(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ToggleCheckoutBody>,
) -> impl IntoResponse {
    let (club_id, admin_id) = match admin_scope(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let db = match admin_db(&state) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match db
        .toggle_checkout_roster(
            &club_id,
            &admin_id,
            &body.pilot_id,
            &body.type_code,
            body.tail.as_deref(),
            body.enable,
            body.expires_at,
        )
        .await
    {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => roster_err(e).into_response(),
    }
}

async fn admin_checkout_bulk(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<BulkCheckoutBody>,
) -> impl IntoResponse {
    let (club_id, admin_id) = match admin_scope(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let db = match admin_db(&state) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match db
        .bulk_checkout_assign(
            &club_id,
            &admin_id,
            &body.pilot_ids,
            &body.type_code,
            body.tail.as_deref(),
            body.assign,
            body.expires_at,
        )
        .await
    {
        Ok(batch_id) => Json(json!({ "ok": true, "batch_id": batch_id })).into_response(),
        Err(e) => roster_err(e).into_response(),
    }
}

async fn admin_delete_checkout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(checkout_id): Path<String>,
) -> impl IntoResponse {
    let (club_id, admin_id) = match admin_scope(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let db = match admin_db(&state) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match db
        .remove_checkout_by_id(&club_id, &admin_id, &checkout_id)
        .await
    {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(e) => roster_err(e).into_response(),
    }
}

async fn admin_import_preview(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
    body: Bytes,
) -> impl IntoResponse {
    let (club_id, admin_id) = match admin_scope(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let db = match admin_db(&state) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let rows = match parse_csv_bytes(&body) {
        Ok(r) => r,
        Err(CsvParseError::TooLarge) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({ "error": "csv too large" })),
            )
                .into_response();
        }
        Err(CsvParseError::TooManyRows) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "too many rows" })),
            )
                .into_response();
        }
        Err(CsvParseError::Empty) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "empty csv" })),
            )
                .into_response();
        }
    };
    match db.import_preview(&club_id, &admin_id, &kind, &rows).await {
        Ok(p) => Json(p).into_response(),
        Err(e) => roster_err(e).into_response(),
    }
}

async fn admin_import_commit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(kind): Path<String>,
    Json(body): Json<ImportCommitBody>,
) -> impl IntoResponse {
    let (club_id, admin_id) = match admin_scope(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let db = match admin_db(&state) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let _ = kind;
    match db.import_commit(&club_id, &admin_id, &body.batch_id).await {
        Ok(n) => Json(json!({ "ok": true, "applied": n })).into_response(),
        Err(e) => roster_err(e).into_response(),
    }
}

async fn admin_import_template(Path(kind): Path<String>) -> impl IntoResponse {
    if let Some(tpl) = ClubDb::import_template(&kind) {
        let body = tpl;
        return ([(header::CONTENT_TYPE, "text/csv; charset=utf-8")], body).into_response();
    };
    (StatusCode::NOT_FOUND, "not found").into_response()
}

async fn admin_audit_list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<AuditQuery>,
) -> impl IntoResponse {
    let (club_id, _admin_id) = match admin_scope(&state, &headers).await {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let db = match admin_db(&state) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let offset = q.offset.unwrap_or(0);
    let limit = q.limit.unwrap_or(50);
    match db
        .list_audit(&club_id, q.entity_type.as_deref(), offset, limit)
        .await
    {
        Ok((entries, total)) => {
            Json(json!({ "entries": entries, "total": total, "offset": offset, "limit": limit }))
                .into_response()
        }
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "db error" })),
        )
            .into_response(),
    }
}
