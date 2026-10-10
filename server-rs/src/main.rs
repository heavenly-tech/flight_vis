mod static_files;

use axum::{extract::State, http::StatusCode, response::Json, routing::get, Router};
use serde_json::json;
use std::{net::SocketAddr, path::PathBuf};

#[derive(Clone)]
struct AppConfig {
    dist_dir: PathBuf,
}

fn config_from_env() -> (AppConfig, u16) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let raw_dist = std::env::var("FLIGHT_VIS_DIST")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("../dist"));
    // Normalize (resolves the `..`) so prefix checks behave; fall back to the
    // raw path when the directory does not exist yet (all reads then 404/500).
    let dist_dir = std::fs::canonicalize(&raw_dist).unwrap_or(raw_dist);
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(8080);
    (AppConfig { dist_dir }, port)
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

async fn fallback(
    State(config): State<AppConfig>,
    uri: axum::http::Uri,
) -> Result<axum::response::Response, StatusCode> {
    static_files::serve(&config.dist_dir, uri.path()).await
}

#[tokio::main]
async fn main() {
    let (config, port) = config_from_env();
    let app = Router::new()
        .route("/api/health", get(health))
        .fallback(fallback)
        .with_state(config);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    eprintln!("flight_vis (rust) listening on {port}");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind listener");
    axum::serve(listener, app).await.expect("serve app");
}
