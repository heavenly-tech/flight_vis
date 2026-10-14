//! Flight videos: raw upload streaming, background 480p transcode, and
//! range-request file serving for frame-accurate player seeking.
//!
//! Storage layout (flat, inside the flight dir):
//!   video-<id>-orig.<ext>   the uploaded original
//!   video-<id>-480p.mp4     the streaming rendition
//! Records live in meta.json `videos[]`:
//!   {id, name, fileName, size, durationS, offsetMs, status, error?}
//! `offsetMs` is the track time (ms) at video t=0; the player computes
//! `videoTime = (cursorMs - offsetMs) / 1000`.
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::process::Command;

pub const MAX_VIDEO_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const TRANSCODE_WIDTH: u32 = 480;
/// In-flight transcode jobs. The worker consumes one at a time; a full queue
/// applies backpressure to upload and requeue instead of growing without bound.
pub(crate) const VIDEO_QUEUE_CAPACITY: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoRecord {
    pub id: String,
    pub name: String,
    #[serde(rename = "fileName")]
    pub file_name: String,
    pub size: u64,
    #[serde(rename = "durationS")]
    pub duration_s: Option<f64>,
    #[serde(rename = "offsetMs")]
    pub offset_ms: Option<i64>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct VideoPatch {
    #[serde(rename = "offsetMs")]
    pub offset_ms: Option<i64>,
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VideoJob {
    pub slug: String,
    pub video_id: String,
}

static VIDEO_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn new_video_id() -> String {
    // 10 base-36 chars from OS randomness (falls back to time+counter).
    // NOTE: read exactly 10 bytes — fs::read would never hit EOF on urandom.
    let random_suffix = (|| {
        use std::io::Read;
        let mut buf = [0u8; 10];
        std::fs::File::open("/dev/urandom")
            .ok()?
            .read_exact(&mut buf)
            .ok()?;
        Some(
            buf.iter()
                .map(|b| char::from_digit((b % 36) as u32, 36).unwrap())
                .collect::<String>(),
        )
    })();
    if let Some(suffix) = random_suffix {
        return format!("vid-{suffix}");
    }
    let mut n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        + u128::from(VIDEO_COUNTER.fetch_add(1, Ordering::SeqCst));
    let mut suffix = String::with_capacity(10);
    for _ in 0..10 {
        suffix.push(char::from_digit((n % 36) as u32, 36).unwrap());
        n /= 36;
    }
    format!("vid-{suffix}")
}

/// Route ids are one path segment, at most 32 characters.
/// New ids are `vid-` plus base-36. Ids already stored may use `vid_`.
pub fn is_valid_video_id(id: &str) -> bool {
    fn charset(bytes: &[u8]) -> bool {
        !bytes.is_empty()
            && bytes
                .iter()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'z' | b'-'))
    }
    let bytes = id.as_bytes();
    if bytes.is_empty() || bytes.len() > 32 {
        return false;
    }
    if let Some(rest) = id.strip_prefix("vid_") {
        return charset(rest.as_bytes());
    }
    charset(bytes)
}

pub fn sanitize_ext(file_name: &str) -> String {
    let ext = file_name
        .rfind('.')
        .map(|idx| &file_name[idx + 1..])
        .unwrap_or("")
        .to_lowercase();
    if ext.len() >= 2 && ext.len() <= 5 && ext.bytes().all(|b| b.is_ascii_alphanumeric()) {
        ext
    } else {
        "mp4".to_string()
    }
}

pub fn original_path(dir: &Path, id: &str, ext: &str) -> PathBuf {
    dir.join(format!("video-{id}-orig.{ext}"))
}

pub fn rendition_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("video-{id}-480p.mp4"))
}

fn find_original(dir: &Path, id: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(&format!("video-{id}-orig.")) && entry.path().is_file() {
                Some(entry.path())
            } else {
                None
            }
        })
        .next()
}

pub fn content_type_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        _ => "application/octet-stream",
    }
}

async fn run_capture(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| format!("{program} unavailable: {error}"))?;
    if !output.status.success() {
        let tail = String::from_utf8_lossy(&output.stderr);
        let tail: String = tail
            .lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");
        return Err(format!("{program} failed: {tail}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub async fn probe_duration(path: &Path) -> Result<f64, String> {
    let out = run_capture(
        "ffprobe",
        &[
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            &path.to_string_lossy(),
        ],
    )
    .await?;
    out.trim()
        .parse::<f64>()
        .map_err(|_| "could not read video duration".to_string())
        .and_then(|secs| {
            if secs.is_finite() && secs > 0.0 {
                Ok(secs)
            } else {
                Err("could not read video duration".to_string())
            }
        })
}

fn transcode_args<'a>(source: &'a str, dest: &'a str, filter: &'a str) -> Vec<&'a str> {
    vec![
        "-y",
        "-protocol_whitelist",
        "file",
        "-i",
        source,
        "-vf",
        filter,
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "28",
        "-c:a",
        "aac",
        "-b:a",
        "96k",
        "-movflags",
        "+faststart",
        dest,
    ]
}

pub async fn transcode_480p(source: &Path, dest: &Path) -> Result<(), String> {
    let filter = format!("scale=-2:{TRANSCODE_WIDTH}");
    let source = source.to_string_lossy().into_owned();
    let dest = dest.to_string_lossy().into_owned();
    let args = transcode_args(&source, &dest, &filter);
    run_capture("ffmpeg", &args).await?;
    Ok(())
}

impl Store {
    fn find_video_dir(&self, slug: &str) -> Option<PathBuf> {
        let dir = self.flight_dir(slug);
        if dir.join("meta.json").is_file() && dir.join("track").is_file() {
            Some(dir)
        } else {
            None
        }
    }

    pub(crate) async fn video_add(
        &self,
        slug: &str,
        record: &VideoRecord,
    ) -> Result<(), crate::store::StoreError> {
        let value = serde_json::to_value(record)
            .map_err(|e| crate::store::StoreError::status(500, e.to_string()))?;
        self.update_meta(slug, |meta| {
            meta.videos
                .retain(|v| v.get("id").and_then(|id| id.as_str()) != Some(record.id.as_str()));
            meta.videos.push(value.clone());
        })
        .await?;
        Ok(())
    }

    pub(crate) async fn video_set_status(
        &self,
        slug: &str,
        id: &str,
        status: &str,
        error: Option<String>,
    ) -> Result<(), crate::store::StoreError> {
        let status = status.to_string();
        self.update_meta(slug, |meta| {
            let mut videos: Vec<VideoRecord> =
                serde_json::from_value(serde_json::Value::Array(meta.videos.clone()))
                    .unwrap_or_default();
            if let Some(rec) = videos.iter_mut().find(|v| v.id == id) {
                rec.status = status.clone();
                rec.error = error.clone();
            }
            meta.videos = serde_json::to_value(&videos)
                .ok()
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
        })
        .await?;
        Ok(())
    }

    pub(crate) async fn video_patch(
        &self,
        slug: &str,
        id: &str,
        patch: &VideoPatch,
    ) -> Result<Option<VideoRecord>, crate::store::StoreError> {
        let patch = patch.clone();
        let meta = self
            .update_meta(slug, |meta| {
                let mut videos: Vec<VideoRecord> =
                    serde_json::from_value(serde_json::Value::Array(meta.videos.clone()))
                        .unwrap_or_default();
                if let Some(rec) = videos.iter_mut().find(|v| v.id == id) {
                    if let Some(offset) = patch.offset_ms {
                        rec.offset_ms = Some(offset);
                    }
                    if let Some(name) = &patch.name {
                        if !name.trim().is_empty() {
                            rec.name = name.trim().to_string();
                        }
                    }
                }
                meta.videos = serde_json::to_value(&videos)
                    .ok()
                    .and_then(|v| v.as_array().cloned())
                    .unwrap_or_default();
            })
            .await?;
        let videos: Vec<VideoRecord> =
            serde_json::from_value(serde_json::Value::Array(meta.videos)).unwrap_or_default();
        Ok(videos.into_iter().find(|v| v.id == id))
    }

    pub(crate) async fn video_remove(
        &self,
        slug: &str,
        id: &str,
    ) -> Result<bool, crate::store::StoreError> {
        let id = id.to_string();
        let mut removed = false;
        self.update_meta(slug, |meta| {
            let before = meta.videos.len();
            meta.videos
                .retain(|v| v.get("id").and_then(|v| v.as_str()) != Some(id.as_str()));
            removed = meta.videos.len() != before;
        })
        .await?;
        Ok(removed)
    }

    pub(crate) async fn video_list(
        &self,
        slug: &str,
    ) -> Result<Vec<VideoRecord>, crate::store::StoreError> {
        let dir = self
            .find_video_dir(slug)
            .ok_or_else(|| crate::store::StoreError::status(404, "Not found"))?;
        let raw = tokio::fs::read_to_string(dir.join("meta.json"))
            .await
            .map_err(|_| crate::store::StoreError::status(404, "Not found"))?;
        let meta: crate::store::FlightMeta = serde_json::from_str(&raw)
            .map_err(|e| crate::store::StoreError::status(500, e.to_string()))?;
        Ok(serde_json::from_value(serde_json::Value::Array(meta.videos)).unwrap_or_default())
    }
}

pub fn spawn_transcode_worker(
    store: Store,
    mut queue: tokio::sync::mpsc::Receiver<VideoJob>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(job) = queue.recv().await {
            let dir = store.flight_dir(&job.slug);
            let result = async {
                let original = find_original(&dir, &job.video_id)
                    .ok_or_else(|| "original file missing".to_string())?;
                let dest = rendition_path(&dir, &job.video_id);
                transcode_480p(&original, &dest).await?;
                Ok::<_, String>(())
            }
            .await;
            match result {
                Ok(()) => {
                    let _ = store
                        .video_set_status(&job.slug, &job.video_id, "ready", None)
                        .await;
                }
                Err(error) => {
                    eprintln!("transcode {}:{} failed: {error}", job.slug, job.video_id);
                    let _ = store
                        .video_set_status(&job.slug, &job.video_id, "failed", Some(error))
                        .await;
                }
            }
        }
    })
}

/// Requeue interrupted transcodes after a restart.
pub async fn requeue_transcoding(store: &Store, queue: &tokio::sync::mpsc::Sender<VideoJob>) {
    for item in store.list_flights().await {
        let Ok(videos) = store.video_list(&item.slug).await else {
            continue;
        };
        for video in videos {
            if video.status == "transcoding"
                && queue
                    .send(VideoJob {
                        slug: item.slug.clone(),
                        video_id: video.id.clone(),
                    })
                    .await
                    .is_err()
            {
                eprintln!(
                    "video queue closed; skipping requeue {}:{}",
                    item.slug, video.id
                );
            }
        }
    }
}

use crate::{
    require_auth,
    store::{is_valid_slug, name_from_file_name, normalize_slug},
    AppState,
};
use axum::{
    body::{Body, Bytes},
    extract::{Path as RoutePath, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json},
};
use futures_util::StreamExt;
use http_body_util::BodyDataStream;
use serde_json::json;
use std::collections::HashMap;
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

fn parse_range(value: &str, size: u64) -> Result<(u64, u64), ()> {
    let spec = value.strip_prefix("bytes=").ok_or(())?;
    if spec.contains(',') {
        return Err(());
    }
    let (start_s, end_s) = spec.split_once('-').ok_or(())?;
    if size == 0 {
        return Err(());
    }
    if start_s.is_empty() {
        let n: u64 = end_s.parse().map_err(|_| ())?;
        if n == 0 {
            return Err(());
        }
        let n = n.min(size);
        Ok((size - n, size - 1))
    } else {
        let start: u64 = start_s.parse().map_err(|_| ())?;
        if start >= size {
            return Err(());
        }
        let end = if end_s.is_empty() {
            size - 1
        } else {
            end_s.parse().map_err(|_| ())?
        };
        if end < start {
            return Err(());
        }
        Ok((start, end.min(size - 1)))
    }
}

async fn post_video(
    State(state): State<AppState>,
    headers: HeaderMap,
    RoutePath(raw_slug): RoutePath<String>,
    Query(query): Query<HashMap<String, String>>,
    body: Body,
) -> axum::response::Response {
    if let Err(denied) = require_auth(&state, &headers) {
        return *denied;
    }
    let slug = normalize_slug(&raw_slug);
    if !is_valid_slug(&slug) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid slug" })),
        )
            .into_response();
    }
    if !state.store.exists(&slug) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "Not found" }))).into_response();
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !(content_type.starts_with("video/") || content_type == "application/octet-stream") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "video file required" })),
        )
            .into_response();
    }
    let file_name = query
        .get("fileName")
        .cloned()
        .unwrap_or_else(|| "video.mp4".to_string());
    let ext = sanitize_ext(&file_name);
    let name = {
        let base = name_from_file_name(&file_name);
        if base.is_empty() {
            "video".to_string()
        } else {
            base
        }
    };
    let id = new_video_id();
    let dir = state.store.flight_dir(&slug);
    let part = dir.join(format!("video-{id}.part"));
    let mut file = match File::create(&part).await {
        Ok(file) => file,
        Err(error) => {
            eprintln!("create video upload failed for {slug}: {error}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "could not store video" })),
            )
                .into_response();
        }
    };
    let mut size = 0u64;
    let mut stream = BodyDataStream::new(body);
    while let Some(frame) = stream.next().await {
        let data = match frame {
            Ok(data) => data,
            Err(_) => {
                let _ = tokio::fs::remove_file(&part).await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "upload interrupted" })),
                )
                    .into_response();
            }
        };
        size += data.len() as u64;
        if size > MAX_VIDEO_BYTES {
            drop(file);
            let _ = tokio::fs::remove_file(&part).await;
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(json!({ "error": "video too large" })),
            )
                .into_response();
        }
        if file.write_all(&data).await.is_err() {
            let _ = tokio::fs::remove_file(&part).await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "could not store video" })),
            )
                .into_response();
        }
    }
    drop(file);
    let duration = match probe_duration(&part).await {
        Ok(secs) => Some(secs),
        Err(error) if error.starts_with("ffprobe unavailable") => None,
        Err(_) => {
            let _ = tokio::fs::remove_file(&part).await;
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "not a readable video" })),
            )
                .into_response();
        }
    };
    let dest = original_path(&dir, &id, &ext);
    if tokio::fs::rename(&part, &dest).await.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "could not store video" })),
        )
            .into_response();
    }
    let record = VideoRecord {
        id: id.clone(),
        name,
        file_name,
        size,
        duration_s: duration,
        offset_ms: None,
        status: "transcoding".to_string(),
        error: None,
    };
    if state.store.video_add(&slug, &record).await.is_err() {
        let _ = tokio::fs::remove_file(&dest).await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "could not store video" })),
        )
            .into_response();
    }
    if state
        .video_queue
        .send(VideoJob {
            slug: slug.clone(),
            video_id: id,
        })
        .await
        .is_err()
    {
        eprintln!("video queue closed; transcode for {slug} will not start");
    }
    (StatusCode::CREATED, Json(record)).into_response()
}

async fn list_videos(
    State(state): State<AppState>,
    RoutePath(raw_slug): RoutePath<String>,
) -> axum::response::Response {
    let slug = normalize_slug(&raw_slug);
    match state.store.video_list(&slug).await {
        Ok(videos) => Json(videos).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, Json(json!({ "error": "Not found" }))).into_response(),
    }
}

async fn patch_video(
    State(state): State<AppState>,
    headers: HeaderMap,
    RoutePath((raw_slug, id)): RoutePath<(String, String)>,
    body: Bytes,
) -> axum::response::Response {
    if let Err(denied) = require_auth(&state, &headers) {
        return *denied;
    }
    let slug = normalize_slug(&raw_slug);
    if !is_valid_slug(&slug) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid slug" })),
        )
            .into_response();
    }
    if !is_valid_video_id(&id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid video id" })),
        )
            .into_response();
    }
    let patch: VideoPatch = serde_json::from_slice(&body).unwrap_or_default();
    match state.store.video_patch(&slug, &id, &patch).await {
        Ok(Some(record)) => Json(record).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "Not found" }))).into_response(),
        Err(error) => (
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({ "error": error.message })),
        )
            .into_response(),
    }
}

async fn delete_video(
    State(state): State<AppState>,
    headers: HeaderMap,
    RoutePath((raw_slug, id)): RoutePath<(String, String)>,
) -> axum::response::Response {
    if let Err(denied) = require_auth(&state, &headers) {
        return *denied;
    }
    let slug = normalize_slug(&raw_slug);
    if !is_valid_slug(&slug) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid slug" })),
        )
            .into_response();
    }
    if !is_valid_video_id(&id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid video id" })),
        )
            .into_response();
    }
    let dir = state.store.flight_dir(&slug);
    match state.store.video_remove(&slug, &id).await {
        Ok(true) => {
            if let Some(orig) = find_original(&dir, &id) {
                let _ = tokio::fs::remove_file(orig).await;
            }
            let _ = tokio::fs::remove_file(rendition_path(&dir, &id)).await;
            Json(json!({ "ok": true })).into_response()
        }
        Ok(false) => (StatusCode::NOT_FOUND, Json(json!({ "error": "Not found" }))).into_response(),
        Err(error) => (
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({ "error": error.message })),
        )
            .into_response(),
    }
}

async fn get_video_file(
    State(state): State<AppState>,
    headers: HeaderMap,
    RoutePath((raw_slug, id)): RoutePath<(String, String)>,
    Query(query): Query<HashMap<String, String>>,
) -> axum::response::Response {
    if !is_valid_video_id(&id) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Invalid video id" })),
        )
            .into_response();
    }
    let slug = normalize_slug(&raw_slug);
    let videos = match state.store.video_list(&slug).await {
        Ok(videos) => videos,
        Err(_) => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "Not found" }))).into_response();
        }
    };
    if !videos.iter().any(|v| v.id == id) {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "Not found" }))).into_response();
    }
    let dir = state.store.flight_dir(&slug);
    let rendition = rendition_path(&dir, &id);
    let rendition_ready = tokio::fs::metadata(&rendition)
        .await
        .map(|m| m.is_file())
        .unwrap_or(false);
    let path = match query.get("quality").map(String::as_str) {
        Some("original") => match find_original(&dir, &id) {
            Some(path) => path,
            None => {
                return (StatusCode::NOT_FOUND, Json(json!({ "error": "Not found" })))
                    .into_response();
            }
        },
        Some("480p") => {
            if !rendition_ready {
                return (
                    StatusCode::NOT_FOUND,
                    Json(json!({ "error": "rendition not ready" })),
                )
                    .into_response();
            }
            rendition
        }
        Some(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "unknown quality" })),
            )
                .into_response();
        }
        None => {
            if rendition_ready {
                rendition
            } else {
                match find_original(&dir, &id) {
                    Some(path) => path,
                    None => {
                        return (StatusCode::NOT_FOUND, Json(json!({ "error": "Not found" })))
                            .into_response();
                    }
                }
            }
        }
    };
    let size = match tokio::fs::metadata(&path).await {
        Ok(meta) => meta.len(),
        Err(_) => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "Not found" }))).into_response();
        }
    };
    let content_type = content_type_for(&path).to_string();
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if range.is_empty() || size == 0 {
        let Ok(file) = File::open(&path).await else {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "Not found" }))).into_response();
        };
        let body = Body::from_stream(tokio_util::io::ReaderStream::new(file));
        return (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type),
                (header::ACCEPT_RANGES, "bytes".to_string()),
                (header::CONTENT_LENGTH, size.to_string()),
            ],
            body,
        )
            .into_response();
    }
    let (start, end) = match parse_range(range, size) {
        Ok(range) => range,
        Err(()) => {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{size}"))],
                Json(json!({ "error": "range not satisfiable" })),
            )
                .into_response();
        }
    };
    let mut file = match File::open(&path).await {
        Ok(file) => file,
        Err(_) => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "Not found" }))).into_response();
        }
    };
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "could not read video" })),
        )
            .into_response();
    }
    let len = end - start + 1;
    let body = Body::from_stream(tokio_util::io::ReaderStream::new(file.take(len)));
    (
        StatusCode::PARTIAL_CONTENT,
        [
            (header::CONTENT_TYPE, content_type),
            (header::ACCEPT_RANGES, "bytes".to_string()),
            (header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}")),
            (header::CONTENT_LENGTH, len.to_string()),
        ],
        body,
    )
        .into_response()
}

pub fn router() -> axum::Router<crate::AppState> {
    axum::Router::new()
        .route(
            "/api/flights/{slug}/videos",
            axum::routing::get(list_videos).post(post_video),
        )
        .route(
            "/api/flights/{slug}/videos/{id}/file",
            axum::routing::get(get_video_file),
        )
        .route(
            "/api/flights/{slug}/videos/{id}",
            axum::routing::patch(patch_video).delete(delete_video),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{auth::Auth, build_router, store::Store, AppState};
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    use tower::ServiceExt;

    #[test]
    fn video_id_and_helpers() {
        let a = new_video_id();
        let b = new_video_id();
        assert!(a.starts_with("vid-") && a.len() == 14);
        assert!(is_valid_video_id(&a));
        assert_ne!(a, b);
        assert!(is_valid_video_id("a"));
        assert!(is_valid_video_id("vid-0123abcd"));
        assert!(is_valid_video_id("vid_0123abcd"));
        assert!(!is_valid_video_id(""));
        assert!(!is_valid_video_id("vid_"));
        assert!(!is_valid_video_id("bad_id"));
        assert!(!is_valid_video_id("vid__abc"));
        assert!(!is_valid_video_id("ABC"));
        assert!(!is_valid_video_id("bad/id"));
        assert!(!is_valid_video_id("has.dot"));
        assert!(!is_valid_video_id(&"a".repeat(33)));
        assert_eq!(sanitize_ext("GoPro.MP4"), "mp4");
        assert_eq!(sanitize_ext("clip.mov"), "mov");
        assert_eq!(sanitize_ext("noext"), "mp4");
        assert_eq!(sanitize_ext("evil.exe"), "exe");
        assert_eq!(sanitize_ext("a.b"), "mp4");
        assert_eq!(content_type_for(Path::new("x.mp4")), "video/mp4");
        assert_eq!(content_type_for(Path::new("x.webm")), "video/webm");
        assert_eq!(
            content_type_for(Path::new("x.foo")),
            "application/octet-stream"
        );
        assert_eq!(parse_range("bytes=0-99", 1000), Ok((0, 99)));
        assert_eq!(parse_range("bytes=500-", 1000), Ok((500, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Ok((900, 999)));
        assert_eq!(parse_range("bytes=0-9999", 1000), Ok((0, 999)));
        assert!(parse_range("bytes=1000-", 1000).is_err());
        assert!(parse_range("bytes=5-3", 1000).is_err());
        assert!(parse_range("bytes=0-99,200-299", 1000).is_err());
        assert!(parse_range("bogus", 1000).is_err());
    }

    #[test]
    fn ffmpeg_transcode_limits_protocols_to_file() {
        let args = transcode_args("in.mp4", "out.mp4", "scale=-2:480");
        let whitelist = args
            .iter()
            .position(|arg| *arg == "-protocol_whitelist")
            .expect("protocol whitelist");
        assert_eq!(args[whitelist + 1], "file");
        let input = args.iter().position(|arg| *arg == "-i").expect("input");
        assert!(whitelist < input);
    }

    static VIDEO_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    async fn ffmpeg_present() -> bool {
        tokio::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .await
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    async fn make_fixture_mp4(path: &Path) {
        let status = tokio::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=2:size=320x240:rate=10",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-pix_fmt",
                "yuv420p",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-c:a",
                "aac",
                &path.to_string_lossy(),
            ])
            .status()
            .await
            .expect("spawn ffmpeg");
        assert!(status.success());
    }

    async fn video_state(password: &str) -> Option<(axum::Router, PathBuf)> {
        if !ffmpeg_present().await {
            eprintln!("skipping video integration: ffmpeg not available");
            return None;
        }
        Some(video_app(password).await)
    }

    async fn video_app(password: &str) -> (axum::Router, PathBuf) {
        let id = VIDEO_DIR_COUNTER.fetch_add(1, Ordering::SeqCst);
        let data_dir =
            std::env::temp_dir().join(format!("flight-vis-rs-video-{}-{id}", std::process::id()));
        let store = Store::new(&data_dir);
        let (video_queue, queue_rx) = tokio::sync::mpsc::channel(VIDEO_QUEUE_CAPACITY);
        spawn_transcode_worker(store.clone(), queue_rx);
        let state = AppState {
            dist_dir: data_dir.join("dist"),
            auth: Auth::with_password(password),
            store,
            video_queue,
            login_throttle: crate::LoginThrottle::default(),
        };
        (build_router(state), data_dir)
    }

    async fn authed_cookie(router: &mut axum::Router, password: &str) -> String {
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

    async fn get_json(
        router: &mut axum::Router,
        method: &str,
        uri: &str,
        cookie: Option<&str>,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> (StatusCode, serde_json::Value, Vec<u8>) {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(cookie) = cookie {
            builder = builder.header(header::COOKIE, cookie);
        }
        if let Some(content_type) = content_type {
            builder = builder.header(header::CONTENT_TYPE, content_type);
        }
        let request: Request<Body> = builder.body(Body::from(body)).unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec();
        let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        let _ = headers;
        (status, json, bytes)
    }

    const TRACK: &str = "<?xml version=\"1.0\"?><gpx version=\"1.1\"><trk><name>T</name><trkseg><trkpt lat=\"-33.4\" lon=\"-70.6\"><ele>800</ele><time>2024-01-01T12:00:00Z</time></trkpt></trkseg></trk></gpx>";

    #[tokio::test]
    async fn post_video_invalid_slug_is_400() {
        let (mut router, data_dir) = video_app("secret").await;
        let cookie = authed_cookie(&mut router, "secret").await;
        let (status, body, _) = get_json(
            &mut router,
            "POST",
            "/api/flights/api/videos?fileName=test.mp4",
            Some(&cookie),
            Some("video/mp4"),
            b"not-a-video".to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["error"], "Invalid slug");

        let (status, body, _) = get_json(
            &mut router,
            "POST",
            "/api/flights/missing-flight/videos?fileName=test.mp4",
            Some(&cookie),
            Some("video/mp4"),
            b"not-a-video".to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"], "Not found");
        let _ = tokio::fs::remove_dir_all(&data_dir).await;
    }

    #[tokio::test]
    async fn video_id_outside_charset_is_400() {
        let (mut router, data_dir) = video_app("secret").await;
        let cookie = authed_cookie(&mut router, "secret").await;
        for uri in [
            "/api/flights/demo/videos/bad_id",
            "/api/flights/demo/videos/HASUPPER",
            "/api/flights/demo/videos/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            let (status, body, _) = get_json(
                &mut router,
                "PATCH",
                uri,
                Some(&cookie),
                Some("application/json"),
                b"{\"name\":\"x\"}".to_vec(),
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
            assert_eq!(body["error"], "Invalid video id");
            let (status, body, _) = get_json(
                &mut router,
                "GET",
                &format!("{uri}/file"),
                None,
                None,
                vec![],
            )
            .await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}/file");
            assert_eq!(body["error"], "Invalid video id");
        }
        let _ = tokio::fs::remove_dir_all(&data_dir).await;
    }

    #[tokio::test]
    async fn video_id_accepts_vid_dash_and_vid_underscore() {
        let (mut router, data_dir) = video_app("secret").await;
        let cookie = authed_cookie(&mut router, "secret").await;
        for id in ["vid-0123abcd", "vid_0123abcd"] {
            let uri = format!("/api/flights/demo/videos/{id}");
            let (status, body, _) = get_json(
                &mut router,
                "PATCH",
                &uri,
                Some(&cookie),
                Some("application/json"),
                b"{\"name\":\"x\"}".to_vec(),
            )
            .await;
            assert_ne!(status, StatusCode::BAD_REQUEST, "{id}");
            assert_eq!(status, StatusCode::NOT_FOUND, "{id}");
            assert_eq!(body["error"], "Not found");
            let (status, body, _) = get_json(
                &mut router,
                "GET",
                &format!("{uri}/file"),
                None,
                None,
                vec![],
            )
            .await;
            assert_ne!(status, StatusCode::BAD_REQUEST, "{id}/file");
            assert_eq!(status, StatusCode::NOT_FOUND, "{id}/file");
            assert_eq!(body["error"], "Not found");
        }
        let generated = new_video_id();
        assert!(generated.starts_with("vid-"));
        assert!(!generated.contains('_'));
        let _ = tokio::fs::remove_dir_all(&data_dir).await;
    }

    #[tokio::test]
    async fn video_roundtrip_with_transcode() {
        let Some((mut router, data_dir)) = video_state("secret").await else {
            return;
        };
        let cookie = authed_cookie(&mut router, "secret").await;

        let put_body =
            serde_json::json!({ "name": "V", "fileName": "v.gpx", "track": TRACK, "tags": [] })
                .to_string();
        let (status, _, _) = get_json(
            &mut router,
            "PUT",
            "/api/flights/vid",
            Some(&cookie),
            Some("application/json"),
            put_body.into_bytes(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let fixture = data_dir.join("fixture.mp4");
        make_fixture_mp4(&fixture).await;
        let bytes = tokio::fs::read(&fixture).await.unwrap();

        let (status, record, _) = get_json(
            &mut router,
            "POST",
            "/api/flights/vid/videos?fileName=test.mp4",
            Some(&cookie),
            Some("video/mp4"),
            bytes.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let id = record["id"].as_str().unwrap().to_string();
        assert_eq!(record["status"], "transcoding");

        let (status, _, _) = get_json(
            &mut router,
            "POST",
            "/api/flights/vid/videos?fileName=test.mp4",
            None,
            Some("video/mp4"),
            bytes.clone(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let (status, _, _) = get_json(
            &mut router,
            "POST",
            "/api/flights/vid/videos?fileName=note.txt",
            Some(&cookie),
            Some("video/mp4"),
            b"not a video".to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        let (status, _, _) = get_json(
            &mut router,
            "POST",
            "/api/flights/vid/videos?fileName=test.mp4",
            Some(&cookie),
            Some("application/json"),
            b"{}".to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);

        // Wait for the background transcode.
        let mut ready = false;
        for _ in 0..60 {
            let (_, flight, _) =
                get_json(&mut router, "GET", "/api/flights/vid", None, None, vec![]).await;
            let videos = flight["videos"].as_array().cloned().unwrap_or_default();
            if let Some(video) = videos.iter().find(|v| v["id"] == id) {
                if video["status"] == "ready" {
                    ready = true;
                    let duration = video["durationS"].as_f64().unwrap();
                    assert!((duration - 2.0).abs() < 0.6, "duration {duration}");
                    break;
                }
                assert_ne!(video["status"], "failed");
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
        assert!(ready, "transcode did not finish in time");

        let (status, videos, _) = get_json(
            &mut router,
            "GET",
            "/api/flights/vid/videos",
            None,
            None,
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(videos.as_array().unwrap().len(), 1);

        // Range request against the rendition.
        let request: Request<Body> = Request::builder()
            .method("GET")
            .uri(format!("/api/flights/vid/videos/{id}/file"))
            .header(header::RANGE, "bytes=0-99")
            .body(Body::from(vec![]))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        let content_range = response
            .headers()
            .get(header::CONTENT_RANGE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(content_range.starts_with("bytes 0-99/"), "{content_range}");
        let chunk = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(chunk.len(), 100);

        // Full file + original + bad quality + bad range.
        let (status, _, full) = get_json(
            &mut router,
            "GET",
            &format!("/api/flights/vid/videos/{id}/file"),
            None,
            None,
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(!full.is_empty());
        let (status, _, _) = get_json(
            &mut router,
            "GET",
            &format!("/api/flights/vid/videos/{id}/file?quality=original"),
            None,
            None,
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = get_json(
            &mut router,
            "GET",
            &format!("/api/flights/vid/videos/{id}/file?quality=bogus"),
            None,
            None,
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let request: Request<Body> = Request::builder()
            .method("GET")
            .uri(format!("/api/flights/vid/videos/{id}/file"))
            .header(header::RANGE, "bytes=999999999-")
            .body(Body::from(vec![]))
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        // Offset patch flows back into the flight record.
        let (status, record, _) = get_json(
            &mut router,
            "PATCH",
            &format!("/api/flights/vid/videos/{id}"),
            Some(&cookie),
            Some("application/json"),
            b"{\"offsetMs\":12345}".to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(record["offsetMs"], 12345);

        let (status, _, _) = get_json(
            &mut router,
            "DELETE",
            &format!("/api/flights/vid/videos/{id}"),
            Some(&cookie),
            None,
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, _, _) = get_json(
            &mut router,
            "GET",
            &format!("/api/flights/vid/videos/{id}/file"),
            None,
            None,
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, videos, _) = get_json(
            &mut router,
            "GET",
            "/api/flights/vid/videos",
            None,
            None,
            vec![],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(videos.as_array().unwrap().len(), 0);
        let _ = tokio::fs::remove_dir_all(&data_dir).await;
    }
}
