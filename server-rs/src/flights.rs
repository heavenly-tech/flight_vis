//! Flight CRUD routes with response parity to the Node implementation.
use crate::{
    require_auth,
    store::{is_valid_slug, normalize_slug, RenameBody, StoreError, UploadFile, WriteFlightBody},
    AppState,
};
use axum::{
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json},
};
use serde_json::json;

fn store_error(error: StoreError) -> axum::response::Response {
    let mut body = serde_json::Map::new();
    body.insert("error".to_string(), json!(error.message));
    if let Some(slugs) = error.slugs {
        body.insert("slugs".to_string(), json!(slugs));
    }
    (
        StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(serde_json::Value::Object(body)),
    )
        .into_response()
}

async fn list_flights(State(state): State<AppState>, headers: HeaderMap) -> axum::response::Response {
    if let Err(denied) = require_auth(&state, &headers) {
        return denied;
    }
    Json(state.store.list_flights().await).into_response()
}

async fn get_flight(
    State(state): State<AppState>,
    Path(raw): Path<String>,
) -> axum::response::Response {
    let slug = normalize_slug(&raw);
    if !is_valid_slug(&slug) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid slug" })),
        )
            .into_response();
    }
    match state.store.read_meta(&slug).await {
        Some(record) => Json(record).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "Not found" })),
        )
            .into_response(),
    }
}

async fn put_flight(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    body: Bytes,
) -> axum::response::Response {
    if let Err(denied) = require_auth(&state, &headers) {
        return denied;
    }
    let slug = normalize_slug(&raw);
    if !is_valid_slug(&slug) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid slug" })),
        )
            .into_response();
    }
    // Malformed JSON falls through to a 500 here in Node (uncaught throw);
    // Axum rejects earlier with 4xx. Frontend only ever sends valid JSON.
    let parsed: WriteFlightBody = match serde_json::from_slice(&body) {
        Ok(parsed) => parsed,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "Invalid JSON" })),
            )
                .into_response();
        }
    };
    match state.store.write_flight(&slug, parsed).await {
        Ok(record) => Json(record).into_response(),
        Err(error) => store_error(error),
    }
}

#[derive(serde::Deserialize, Default)]
struct UploadBody {
    files: Option<Vec<UploadFile>>,
    conflict: Option<String>,
}

async fn post_flights(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    if let Err(denied) = require_auth(&state, &headers) {
        return denied;
    }
    let parsed: UploadBody = serde_json::from_slice(&body).unwrap_or_default();
    match state
        .store
        .upload_flights(parsed.files, parsed.conflict)
        .await
    {
        Ok(items) => Json(json!({ "items": items })).into_response(),
        Err(error) => store_error(error),
    }
}

async fn patch_flight(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
    body: Bytes,
) -> axum::response::Response {
    if let Err(denied) = require_auth(&state, &headers) {
        return denied;
    }
    let slug = normalize_slug(&raw);
    if !is_valid_slug(&slug) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid slug" })),
        )
            .into_response();
    }
    let parsed: RenameBody = serde_json::from_slice(&body).unwrap_or(RenameBody {
        name: None,
        slug: None,
        overwrite: None,
    });
    match state.store.rename_flight(&slug, parsed).await {
        Ok(item) => Json(item).into_response(),
        Err(error) => store_error(error),
    }
}

async fn delete_flight(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(raw): Path<String>,
) -> axum::response::Response {
    if let Err(denied) = require_auth(&state, &headers) {
        return denied;
    }
    let slug = normalize_slug(&raw);
    if !is_valid_slug(&slug) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid slug" })),
        )
            .into_response();
    }
    match state.store.delete_flight(&slug).await {
        Ok(()) => Json(json!({ "ok": true })).into_response(),
        Err(error) => store_error(error),
    }
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new()
        .route("/api/flights", axum::routing::get(list_flights).post(post_flights))
        .route(
            "/api/flights/{slug}",
            axum::routing::get(get_flight)
                .put(put_flight)
                .patch(patch_flight)
                .delete(delete_flight),
        )
}

#[cfg(test)]
mod tests {
    use crate::{auth::Auth, build_router, store::Store, AppState};
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use tower::ServiceExt;

    const TRACK: &str = "<?xml version=\"1.0\"?>\n<gpx version=\"1.1\"><trk><name>Test</name><trkseg>\n<trkpt lat=\"-33.4\" lon=\"-70.6\"><ele>800</ele><time>2024-01-01T12:00:00Z</time></trkpt>\n<trkpt lat=\"-33.41\" lon=\"-70.61\"><ele>820</ele><time>2024-01-01T12:01:00Z</time></trkpt>\n</trkseg></trk></gpx>";

    static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    async fn test_state(password: &str) -> (axum::Router, PathBuf) {
        let id = TEST_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        let data_dir = std::env::temp_dir().join(format!(
            "flight-vis-rs-test-{}-{id}",
            std::process::id()
        ));
        let dist_dir = data_dir.join("dist");
        tokio::fs::create_dir_all(&dist_dir).await.unwrap();
        tokio::fs::write(dist_dir.join("index.html"), "<html>app</html>")
            .await
            .unwrap();
        tokio::fs::write(dist_dir.join("login.html"), "<html>login</html>")
            .await
            .unwrap();
        let store = Store::new(&data_dir);
        let (video_queue, queue_rx) = tokio::sync::mpsc::unbounded_channel();
        crate::video::spawn_transcode_worker(store.clone(), queue_rx);
        let state = AppState {
            dist_dir,
            auth: Auth::with_password(password),
            store,
            video_queue,
        };
        let router = build_router(state);
        (router, data_dir)
    }

    async fn cleanup(data_dir: &Path) {
        let _ = tokio::fs::remove_dir_all(data_dir).await;
    }

    async fn call(
        router: &mut axum::Router,
        method: &str,
        uri: &str,
        cookie: Option<&str>,
        content_type: Option<&str>,
        body: &str,
    ) -> (StatusCode, serde_json::Value, String) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        if let Some(content_type) = content_type {
            builder = builder.header(header::CONTENT_TYPE, content_type);
        }
        let request: Request<Body> = builder.body(Body::from(body.to_string())).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let json = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        (status, json, text)
    }

    async fn login_cookie(router: &mut axum::Router, password: &str) -> String {
        let request: Request<Body> = Request::builder()
            .method("POST")
            .uri("/login")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json")
            .body(Body::from(format!("{{\"password\":\"{password}\"}}")))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        set_cookie.split(';').next().unwrap().to_string()
    }

    #[tokio::test]
    async fn health_and_public_reads_and_auth_flows() {
        let (mut router, data_dir) = test_state("secret").await;
        let (status, _, _) = call(&mut router, "GET", "/api/health", None, None, "").await;
        assert_eq!(status, StatusCode::OK);

        let put_body = serde_json::json!({ "name": "Demo", "fileName": "demo.gpx", "track": TRACK, "tags": [] }).to_string();
        let (status, _, _) = call(
            &mut router, "PUT", "/api/flights/demo", None, Some("application/json"), &put_body,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let cookie = login_cookie(&mut router, "secret").await;
        assert!(cookie.starts_with("fv_session="));
        let (status, _, _) = call(
            &mut router, "PUT", "/api/flights/demo", Some(&cookie), Some("application/json"), &put_body,
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body, _) = call(&mut router, "GET", "/api/flights/demo", None, None, "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["slug"], "demo");
        assert!(body["track"].as_str().unwrap().contains("<gpx"));
        // Byte-parity guard: no videos key until videos exist, Node order.
        let keys: Vec<&str> = body.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            vec![
                "slug", "name", "fileName", "trimStartMs", "trimEndMs", "tags", "updatedAt",
                "createdAt", "track",
            ]
        );

        let (status, _, _) = call(&mut router, "GET", "/api/flights", None, None, "").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, body, _) = call(&mut router, "GET", "/api/auth/me", Some(&cookie), None, "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!({ "authenticated": true, "configured": true }));

        let (status, body, _) = call(
            &mut router, "POST", "/login", None, Some("application/json"),
            "{\"password\":\"wrong\"}",
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "unauthorized");

        let (status, _, _) = call(&mut router, "GET", "/login", None, None, "").await;
        assert_eq!(status, StatusCode::OK);

        let request = Request::builder()
            .method("POST")
            .uri("/logout")
            .header(header::ACCEPT, "application/json")
            .header(header::COOKIE, &cookie)
            .body(Body::from(""))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap();
        assert!(set_cookie.contains("Max-Age=0"));

        let (status, body, _) = call(&mut router, "GET", "/api/auth/me", None, None, "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!({ "authenticated": false, "configured": true }));
        cleanup(&data_dir).await;
    }

    #[tokio::test]
    async fn library_crud_matches_node() {
        let (mut router, data_dir) = test_state("secret").await;
        let cookie = login_cookie(&mut router, "secret").await;

        async fn put_flight(
            router: &mut axum::Router,
            cookie: &str,
            slug: &str,
            name: &str,
        ) -> (StatusCode, serde_json::Value, String) {
            let body = serde_json::json!({ "name": name, "fileName": format!("{slug}.gpx"), "track": TRACK, "tags": [] }).to_string();
            call(router, "PUT", &format!("/api/flights/{slug}"), Some(cookie), Some("application/json"), &body).await
        }

        let (status, _, _) = put_flight(&mut router, &cookie, "alpha", "Alpha").await;
        assert_eq!(status, StatusCode::OK);

        let (status, body, _) = call(&mut router, "GET", "/api/flights", Some(&cookie), None, "").await;
        assert_eq!(status, StatusCode::OK);
        let alpha = body.as_array().unwrap().iter().find(|item| item["slug"] == "alpha").unwrap().clone();
        assert_eq!(alpha["name"], "Alpha");
        assert_eq!(alpha["fileName"], "alpha.gpx");
        assert!(alpha["size"].as_u64().unwrap() > 0);
        assert!(alpha["updatedAt"].is_string());

        let upload_body = serde_json::json!({ "files": [
            { "fileName": "Cerro Provincia.igc", "track": TRACK },
            { "fileName": "cerro-provincia.gpx", "track": TRACK },
        ] })
        .to_string();
        let (status, body, _) = call(
            &mut router, "POST", "/api/flights", Some(&cookie), Some("application/json"), &upload_body,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let mut slugs: Vec<String> = body["items"].as_array().unwrap().iter().map(|item| item["slug"].as_str().unwrap().to_string()).collect();
        slugs.sort();
        assert_eq!(slugs, vec!["cerro-provincia".to_string(), "cerro-provincia-2".to_string()]);

        let clash_body = serde_json::json!({ "conflict": "error", "files": [{ "fileName": "cerro-provincia.gpx", "track": TRACK }] }).to_string();
        let (status, body, _) = call(
            &mut router, "POST", "/api/flights", Some(&cookie), Some("application/json"), &clash_body,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["slugs"], serde_json::json!(["cerro-provincia"]));

        let (status, _, _) = put_flight(&mut router, &cookie, "source", "Source").await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = put_flight(&mut router, &cookie, "taken", "Taken").await;
        assert_eq!(status, StatusCode::OK);

        let blocked_body = serde_json::json!({ "slug": "taken", "name": "Nope" }).to_string();
        let (status, body, _) = call(
            &mut router, "PUT", "/api/flights/source", Some(&cookie), Some("application/json"), &blocked_body,
        )
        .await;
        // NOTE: TS test uses PATCH for rename; PUT with a slug body only writes.
        // Rename-path coverage follows below via PATCH.
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["slug"], "source");

        let rename_body = serde_json::json!({ "slug": "taken", "name": "Nope" }).to_string();
        let (status, body, _) = call(
            &mut router, "PATCH", "/api/flights/source", Some(&cookie), Some("application/json"), &rename_body,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "exists");

        let (status, body, _) = call(&mut router, "GET", "/api/flights/taken", None, None, "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["name"], "Taken");

        let rename_body = serde_json::json!({ "slug": "renamed-source", "name": "Renamed" }).to_string();
        let (status, body, _) = call(
            &mut router, "PATCH", "/api/flights/source", Some(&cookie), Some("application/json"), &rename_body,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["slug"], "renamed-source");
        assert_eq!(body["name"], "Renamed");

        let (status, _, _) = call(&mut router, "GET", "/api/flights/source", None, None, "").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body, _) = call(&mut router, "GET", "/api/flights/renamed-source", None, None, "").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["track"].as_str().unwrap().contains("<gpx"));

        let overwrite_body = serde_json::json!({ "slug": "taken", "overwrite": true }).to_string();
        let (status, body, _) = call(
            &mut router, "PATCH", "/api/flights/renamed-source", Some(&cookie), Some("application/json"), &overwrite_body,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["slug"], "taken");
        let (status, _, _) = call(&mut router, "GET", "/api/flights/renamed-source", None, None, "").await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let (status, _, _) = put_flight(&mut router, &cookie, "keep-me", "keep-me").await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = put_flight(&mut router, &cookie, "drop-me", "drop-me").await;
        assert_eq!(status, StatusCode::OK);
        let (status, body, _) = call(&mut router, "DELETE", "/api/flights/drop-me", Some(&cookie), None, "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, serde_json::json!({ "ok": true }));
        let (status, _, _) = call(&mut router, "GET", "/api/flights/drop-me", None, None, "").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, body, _) = call(&mut router, "GET", "/api/flights/keep-me", None, None, "").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["slug"], "keep-me");

        let (status, _, _) = call(&mut router, "POST", "/api/flights", None, Some("application/json"), "{\"files\":[]}").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, _) = call(&mut router, "PATCH", "/api/flights/demo", None, Some("application/json"), "{\"slug\":\"other\"}").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _, _) = call(&mut router, "DELETE", "/api/flights/demo", None, None, "").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        cleanup(&data_dir).await;
    }

    #[tokio::test]
    async fn locked_mode_rejects_writes() {
        let (mut router, data_dir) = test_state("").await;
        let put_body = serde_json::json!({ "name": "Locked", "fileName": "x.gpx", "track": TRACK }).to_string();
        let (status, body, _) = call(
            &mut router, "PUT", "/api/flights/locked", None, Some("application/json"), &put_body,
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"], "locked");

        let (status, _, _) = call(&mut router, "DELETE", "/api/flights/keep-me", None, None, "").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, headers_text, _) = {
            let request: Request<Body> = Request::builder()
                .method("POST")
                .uri("/login")
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from("password=anything"))
                .unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let location = response.headers().get(header::LOCATION).cloned();
            (status, location, String::new())
        };
        assert_eq!(status, StatusCode::FOUND);
        assert_eq!(headers_text.unwrap().to_str().unwrap(), "/login?locked=1");
        cleanup(&data_dir).await;
    }
}
