//! Disk flight library auth: legacy `fv_session` or club pilot ownership (`disk_flight_owners`).

use crate::{
    auth,
    club::{
        auth::parse_session_cookie,
        visibility::{PilotStatus, Viewer},
    },
    store::{is_valid_slug, normalize_slug, Store},
    AppState,
};
use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json},
};
use serde_json::json;

fn unauthorized(state: &AppState) -> axum::response::Response {
    let error = if state.auth.configured() {
        "unauthorized"
    } else {
        "locked"
    };
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": error }))).into_response()
}

pub(crate) fn legacy_session_ok(state: &AppState, headers: &HeaderMap) -> bool {
    auth::parse_cookie(headers.get(header::COOKIE).and_then(|v| v.to_str().ok()))
        .map(|token| {
            state.auth.verify(
                &token,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            )
        })
        .unwrap_or(false)
}

pub async fn storage_slug_for(
    state: &AppState,
    store: &Store,
    public_slug: &str,
) -> Option<String> {
    if let Some(db) = state.club.as_ref() {
        if let Some(storage) = db.disk_storage_slug(public_slug).await {
            if store.exists(&storage) {
                return Some(storage);
            }
        }
    }
    if store.exists(public_slug) {
        Some(public_slug.to_string())
    } else {
        None
    }
}

#[allow(clippy::result_large_err)]
pub async fn require_flight_write(
    state: &AppState,
    headers: &HeaderMap,
    public_slug: &str,
) -> Result<(), axum::response::Response> {
    if legacy_session_ok(state, headers) {
        return Ok(());
    }
    let Some(db) = state.club.as_ref() else {
        return Err(unauthorized(state));
    };
    let session = parse_session_cookie(headers);
    let now = crate::club::now_ms();
    let viewer = db.viewer_from_session(session.as_deref(), now).await;
    match viewer {
        Viewer::Pilot {
            id,
            status: PilotStatus::Approved,
            ..
        } if db.pilot_owns_disk_slug(&id, public_slug).await => Ok(()),
        _ => Err(unauthorized(state)),
    }
}

#[allow(clippy::result_large_err)]
pub async fn list_library_flights(
    state: &AppState,
    store: &Store,
    headers: &HeaderMap,
) -> Result<Vec<crate::store::FlightListItem>, axum::response::Response> {
    if legacy_session_ok(state, headers) {
        let mut items = store.list_flights().await;
        if let Some(db) = state.club.as_ref() {
            for item in &mut items {
                if let Some(public) = db.disk_public_slug(&item.slug).await {
                    item.slug = public;
                }
            }
        }
        return Ok(items);
    }
    let Some(db) = state.club.as_ref() else {
        return Err(unauthorized(state));
    };
    let session = parse_session_cookie(headers);
    let now = crate::club::now_ms();
    let viewer = db.viewer_from_session(session.as_deref(), now).await;
    let Viewer::Pilot {
        id,
        status: PilotStatus::Approved,
        ..
    } = viewer
    else {
        return Err(unauthorized(state));
    };
    let mut out = Vec::new();
    for public in db.list_disk_public_slugs_for_pilot(&id).await {
        let public = normalize_slug(&public);
        if !is_valid_slug(&public) {
            continue;
        }
        let Some(storage) = storage_slug_for(state, store, &public).await else {
            continue;
        };
        if let Some(record) = store.read_meta(&storage).await {
            let mut item = store.summarize(&storage, &record.meta);
            item.slug = public;
            out.push(item);
        }
    }
    Ok(out)
}
