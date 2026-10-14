//! Flight disk store with behavior parity to the Node implementation:
//! same slug rules, meta.json schema, error shapes, and sort order.
//! Writes go through temp-file + rename and are serialized by a global
//! mutex (Node interleaves async handlers the same way; serializing keeps
//! read-modify-write paths like rename/overwrite race-free).
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Mutex;

fn deserialize_opt_ms<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Some(i))
            } else if let Some(f) = n.as_f64() {
                if f.is_nan() || f.is_infinite() {
                    return Err(D::Error::custom("float value is NaN or infinite"));
                }
                if f < i64::MIN as f64 || f > i64::MAX as f64 {
                    return Err(D::Error::custom("float value out of i64 range"));
                }
                Ok(Some(f.round() as i64))
            } else {
                Err(D::Error::custom("number does not fit in i64"))
            }
        }
        _ => Err(D::Error::custom("expected number or null")),
    }
}

const RESERVED: &[&str] = &[
    "api",
    "assets",
    "cesiumstatic",
    "favicon.ico",
    "health",
    "login",
    "logout",
    "samples",
    "static",
];

#[derive(Debug)]
pub struct StoreError {
    pub status: u16,
    pub message: String,
    pub slugs: Option<Vec<String>>,
}

impl StoreError {
    pub(crate) fn status(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            slugs: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FlightMeta {
    pub slug: String,
    pub name: String,
    #[serde(rename = "fileName")]
    pub file_name: String,
    #[serde(
        rename = "trimStartMs",
        default,
        deserialize_with = "deserialize_opt_ms"
    )]
    pub trim_start_ms: Option<i64>,
    #[serde(rename = "trimEndMs", default, deserialize_with = "deserialize_opt_ms")]
    pub trim_end_ms: Option<i64>,
    #[serde(default)]
    pub tags: Vec<serde_json::Value>,
    // Kept on disk always, but omitted from API responses while empty so
    // payloads stay byte-identical to Node until videos exist (phase 4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub videos: Vec<serde_json::Value>,
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlightRecord {
    #[serde(flatten)]
    pub meta: FlightMeta,
    pub track: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FlightListItem {
    pub slug: String,
    pub name: String,
    #[serde(rename = "fileName")]
    pub file_name: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WriteFlightBody {
    pub name: Option<String>,
    #[serde(rename = "fileName", alias = "file_name")]
    pub file_name: Option<String>,
    #[serde(
        rename = "trimStartMs",
        default,
        deserialize_with = "deserialize_opt_ms"
    )]
    pub trim_start_ms: Option<i64>,
    #[serde(rename = "trimEndMs", default, deserialize_with = "deserialize_opt_ms")]
    pub trim_end_ms: Option<i64>,
    pub tags: Option<Vec<serde_json::Value>>,
    pub track: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UploadFile {
    #[serde(rename = "fileName")]
    pub file_name: Option<String>,
    pub track: Option<String>,
    pub name: Option<String>,
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RenameBody {
    pub name: Option<String>,
    pub slug: Option<String>,
    pub overwrite: Option<bool>,
}

pub fn normalize_slug(input: &str) -> String {
    let lower = input.trim().to_lowercase();
    // NFKD then strip combining marks, mirroring String.prototype.normalize.
    let decomposed: String = unicode_normalization::UnicodeNormalization::nfkd(lower.chars())
        .filter(|ch| !('\u{0300}'..='\u{036f}').contains(ch))
        .collect();
    let mut out = String::with_capacity(decomposed.len());
    let mut last_dash = true;
    for ch in decomposed.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    trimmed.chars().take(64).collect()
}

pub fn is_valid_slug(slug: &str) -> bool {
    if slug.is_empty() || slug.len() > 64 {
        return false;
    }
    if RESERVED.contains(&slug) {
        return false;
    }
    let bytes = slug.as_bytes();
    if bytes[0] == b'-' || bytes[bytes.len() - 1] == b'-' {
        return false;
    }
    let mut prev_dash = false;
    for &b in bytes {
        let ok = b.is_ascii_lowercase() || b.is_ascii_digit();
        if b == b'-' {
            if prev_dash {
                return false;
            }
            prev_dash = true;
            continue;
        }
        if !ok {
            return false;
        }
        prev_dash = false;
    }
    true
}

/// Mirrors `/\.[^.]+$/`: strip a trailing dot plus one-or-more non-dots.
fn strip_extension(file_name: &str) -> &str {
    match file_name.rfind('.') {
        Some(idx) => {
            let tail = &file_name[idx + 1..];
            if !tail.is_empty() && !tail.contains('.') {
                &file_name[..idx]
            } else {
                file_name
            }
        }
        None => file_name,
    }
}

pub fn slug_from_file_name(file_name: &str) -> String {
    let slug = normalize_slug(strip_extension(file_name));
    if is_valid_slug(&slug) {
        slug
    } else {
        "track".to_string()
    }
}

pub fn name_from_file_name(file_name: &str) -> String {
    let base = strip_extension(file_name);
    if base.is_empty() {
        "track".to_string()
    } else {
        base.to_string()
    }
}

pub fn allocate_slug(
    desired: &str,
    taken: &std::collections::HashSet<String>,
) -> Result<String, StoreError> {
    let base = if is_valid_slug(desired) {
        desired.to_string()
    } else {
        "track".to_string()
    };
    if !taken.contains(&base) {
        return Ok(base);
    }
    for n in 2..10000 {
        let suffix = format!("-{n}");
        let keep = 64usize.saturating_sub(suffix.len());
        let candidate_base: String = base.chars().take(keep).collect();
        let candidate = normalize_slug(&format!("{candidate_base}{suffix}"));
        let candidate = if is_valid_slug(&candidate) {
            candidate
        } else {
            format!("track{suffix}")
        };
        if !taken.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Err(StoreError::status(409, "Could not allocate slug"))
}

/// Current UTC time formatted like `Date.prototype.toISOString()`.
pub fn now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_secs() as i64, d.subsec_millis()))
        .unwrap_or((0, 0));
    iso_from_unix_ms(now.0 * 1000 + now.1 as i64)
}

fn iso_from_unix_ms(ms: i64) -> String {
    // Howard Hinnant's civil-from-days algorithm.
    let days = ms.div_euclid(86_400_000);
    let ms_of_day = ms.rem_euclid(86_400_000);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as i64;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as i64;
    let year = if m <= 2 { y + 1 } else { y };
    let h = ms_of_day / 3_600_000;
    let min = (ms_of_day % 3_600_000) / 60_000;
    let s = (ms_of_day % 60_000) / 1000;
    let millis = ms_of_day % 1000;
    format!("{year:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}.{millis:03}Z")
}

#[derive(Clone)]
pub struct Store {
    flights_dir: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

impl Store {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            flights_dir: data_dir.join("flights"),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn flight_dir(&self, slug: &str) -> PathBuf {
        self.flights_dir.join(slug)
    }

    /// Read-modify-write of meta.json under the write lock. Used for video
    /// record transitions; returns 404 when the flight is missing.
    pub(crate) async fn update_meta(
        &self,
        slug: &str,
        apply: impl FnOnce(&mut FlightMeta),
    ) -> Result<FlightMeta, StoreError> {
        let _guard = self.write_lock.lock().await;
        let meta_path = self.flight_dir(slug).join("meta.json");
        let raw = tokio::fs::read_to_string(&meta_path)
            .await
            .map_err(|_| StoreError::status(404, "Not found"))?;
        let mut meta: FlightMeta = serde_json::from_str(&raw)
            .map_err(|error| StoreError::status(500, error.to_string()))?;
        apply(&mut meta);
        meta.updated_at = Some(now_iso());
        Self::write_text_atomic(&meta_path, &serde_json::to_string_pretty(&meta).unwrap())
            .await
            .map_err(|error| StoreError::status(500, error.to_string()))?;
        Ok(meta)
    }

    pub async fn read_meta(&self, slug: &str) -> Option<FlightRecord> {
        let dir = self.flight_dir(slug);
        let meta_raw = tokio::fs::read_to_string(dir.join("meta.json"))
            .await
            .ok()?;
        let track = tokio::fs::read_to_string(dir.join("track")).await.ok()?;
        let mut meta: FlightMeta = serde_json::from_str(&meta_raw).ok()?;
        meta.slug = slug.to_string();
        Some(FlightRecord { meta, track })
    }

    pub fn exists(&self, slug: &str) -> bool {
        let dir = self.flight_dir(slug);
        dir.join("meta.json").is_file() && dir.join("track").is_file()
    }

    async fn write_text_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, contents).await?;
        tokio::fs::rename(&tmp, path).await?;
        Ok(())
    }

    pub async fn write_flight(
        &self,
        slug: &str,
        body: WriteFlightBody,
    ) -> Result<FlightRecord, StoreError> {
        let _guard = self.write_lock.lock().await;
        let dir = self.flight_dir(slug);
        let existing: Option<FlightMeta> = tokio::fs::read_to_string(dir.join("meta.json"))
            .await
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());
        let mut track = body.track.filter(|track| !track.is_empty());
        if track.is_none() {
            track = tokio::fs::read_to_string(dir.join("track"))
                .await
                .ok()
                .filter(|track| !track.is_empty());
        }
        let Some(track) = track else {
            return Err(StoreError::status(400, "Track file is required"));
        };
        let name = body
            .name
            .filter(|name| !name.is_empty())
            .or_else(|| existing.as_ref().map(|meta| meta.name.clone()))
            .unwrap_or_else(|| slug.to_string());
        let file_name = body
            .file_name
            .or_else(|| existing.as_ref().map(|meta| meta.file_name.clone()))
            .unwrap_or_else(|| "track".to_string());
        let meta = FlightMeta {
            slug: slug.to_string(),
            name,
            file_name,
            trim_start_ms: body
                .trim_start_ms
                .or_else(|| existing.as_ref().and_then(|m| m.trim_start_ms)),
            trim_end_ms: body
                .trim_end_ms
                .or_else(|| existing.as_ref().and_then(|m| m.trim_end_ms)),
            tags: body.tags.unwrap_or_else(|| {
                existing
                    .as_ref()
                    .map(|m| m.tags.clone())
                    .unwrap_or_default()
            }),
            videos: existing
                .as_ref()
                .map(|m| m.videos.clone())
                .unwrap_or_default(),
            updated_at: Some(now_iso()),
            created_at: existing
                .as_ref()
                .and_then(|m| m.created_at.clone())
                .or_else(|| Some(now_iso())),
        };
        Self::write_text_atomic(&dir.join("track"), &track)
            .await
            .map_err(|error| StoreError::status(500, error.to_string()))?;
        Self::write_text_atomic(
            &dir.join("meta.json"),
            &serde_json::to_string_pretty(&meta).unwrap(),
        )
        .await
        .map_err(|error| StoreError::status(500, error.to_string()))?;
        Ok(FlightRecord { meta, track })
    }

    fn summarize(&self, slug: &str, meta: &FlightMeta) -> FlightListItem {
        let size = std::fs::metadata(self.flight_dir(slug).join("track"))
            .map(|m| m.len())
            .unwrap_or(0);
        FlightListItem {
            slug: slug.to_string(),
            name: if meta.name.is_empty() {
                slug.to_string()
            } else {
                meta.name.clone()
            },
            file_name: if meta.file_name.is_empty() {
                "track".to_string()
            } else {
                meta.file_name.clone()
            },
            updated_at: meta.updated_at.clone(),
            created_at: meta.created_at.clone(),
            size,
        }
    }

    pub async fn list_flights(&self) -> Vec<FlightListItem> {
        let _ = tokio::fs::create_dir_all(&self.flights_dir).await;
        let mut entries = match tokio::fs::read_dir(&self.flights_dir).await {
            Ok(entries) => entries,
            Err(_) => return vec![],
        };
        let mut items = vec![];
        while let Ok(Some(entry)) = entries.next_entry().await {
            let slug = entry.file_name().to_string_lossy().into_owned();
            if !is_valid_slug(&slug) {
                continue;
            }
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            let dir = self.flight_dir(&slug);
            if !dir.join("meta.json").is_file() || !dir.join("track").is_file() {
                continue;
            }
            let Ok(raw) = tokio::fs::read_to_string(dir.join("meta.json")).await else {
                continue;
            };
            let Ok(meta) = serde_json::from_str::<FlightMeta>(&raw) else {
                continue;
            };
            items.push(self.summarize(&slug, &meta));
        }
        items.sort_by(|a, b| {
            b.updated_at
                .clone()
                .unwrap_or_default()
                .cmp(&a.updated_at.clone().unwrap_or_default())
        });
        items
    }

    pub async fn delete_flight(&self, slug: &str) -> Result<(), StoreError> {
        if !self.exists(slug) {
            return Err(StoreError::status(404, "Not found"));
        }
        let _guard = self.write_lock.lock().await;
        tokio::fs::remove_dir_all(self.flight_dir(slug))
            .await
            .map_err(|error| StoreError::status(500, error.to_string()))?;
        Ok(())
    }

    pub async fn rename_flight(
        &self,
        from_slug: &str,
        body: RenameBody,
    ) -> Result<FlightListItem, StoreError> {
        if !self.exists(from_slug) {
            return Err(StoreError::status(404, "Not found"));
        }
        let next_name = body
            .name
            .map(|name| name.trim().to_string())
            .unwrap_or_default();
        let slug_given = body.slug.is_some();
        let requested = body
            .slug
            .map(|slug| normalize_slug(&slug))
            .unwrap_or_else(|| from_slug.to_string());
        if slug_given && !is_valid_slug(&requested) {
            return Err(StoreError::status(400, "Invalid slug"));
        }
        let _guard = self.write_lock.lock().await;
        if requested != from_slug {
            if self.exists(&requested) && body.overwrite != Some(true) {
                // Node attaches the slug to the error object but the response
                // only carries `{ error }`, so neither do we.
                return Err(StoreError::status(409, "exists"));
            }
            if self.exists(&requested) && body.overwrite == Some(true) {
                tokio::fs::remove_dir_all(self.flight_dir(&requested))
                    .await
                    .map_err(|error| StoreError::status(500, error.to_string()))?;
            }
            tokio::fs::rename(self.flight_dir(from_slug), self.flight_dir(&requested))
                .await
                .map_err(|error| StoreError::status(500, error.to_string()))?;
        }
        let meta_path = self.flight_dir(&requested).join("meta.json");
        let raw = tokio::fs::read_to_string(&meta_path)
            .await
            .map_err(|error| StoreError::status(500, error.to_string()))?;
        let mut meta: FlightMeta = serde_json::from_str(&raw)
            .map_err(|error| StoreError::status(500, error.to_string()))?;
        if !next_name.is_empty() {
            meta.name = next_name;
        }
        meta.slug = requested.clone();
        meta.updated_at = Some(now_iso());
        Self::write_text_atomic(&meta_path, &serde_json::to_string_pretty(&meta).unwrap())
            .await
            .map_err(|error| StoreError::status(500, error.to_string()))?;
        Ok(self.summarize(&requested, &meta))
    }

    pub async fn upload_flights(
        &self,
        files: Option<Vec<UploadFile>>,
        conflict: Option<String>,
    ) -> Result<Vec<FlightListItem>, StoreError> {
        let files = files.unwrap_or_default();
        if files.is_empty() {
            return Err(StoreError::status(400, "Files are required"));
        }
        if files.len() > 50 {
            return Err(StoreError::status(400, "Too many files"));
        }
        let mode = match conflict.as_deref() {
            Some("overwrite") | Some("error") => conflict.unwrap(),
            _ => "suffix".to_string(),
        };
        let taken: HashSet<String> = self
            .list_flights()
            .await
            .into_iter()
            .map(|item| item.slug)
            .collect();
        let mut taken = taken;
        let mut existing = vec![];
        let mut prepared = vec![];
        for file in &files {
            let file_name = file
                .file_name
                .clone()
                .unwrap_or_else(|| "track".to_string());
            let track = file.track.clone().filter(|track| !track.is_empty());
            let Some(track) = track else {
                return Err(StoreError::status(400, "Track file is required"));
            };
            let desired = file
                .slug
                .as_ref()
                .map(|slug| normalize_slug(slug))
                .unwrap_or_else(|| slug_from_file_name(&file_name));
            let base = if is_valid_slug(&desired) {
                desired
            } else {
                slug_from_file_name(&file_name)
            };
            if taken.contains(&base) && mode == "error" {
                existing.push(base);
                continue;
            }
            let slug = if taken.contains(&base) && mode != "overwrite" {
                allocate_slug(&base, &taken)?
            } else {
                base
            };
            let name = file
                .name
                .clone()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| name_from_file_name(&file_name));
            taken.insert(slug.clone());
            prepared.push((slug, name, file_name, track));
        }
        if mode == "error" && !existing.is_empty() {
            let unique: Vec<String> = {
                let mut seen = HashSet::new();
                existing
                    .into_iter()
                    .filter(|slug| seen.insert(slug.clone()))
                    .collect()
            };
            let mut error = StoreError::status(409, "exists");
            error.slugs = Some(unique);
            return Err(error);
        }
        let mut items = vec![];
        for (slug, name, file_name, track) in prepared {
            let saved = self
                .write_flight(
                    &slug,
                    WriteFlightBody {
                        name: Some(name),
                        file_name: Some(file_name.clone()),
                        trim_start_ms: None,
                        trim_end_ms: None,
                        tags: Some(vec![]),
                        track: Some(track),
                    },
                )
                .await?;
            items.push(self.summarize(&slug, &saved.meta));
        }
        Ok(items)
    }

    pub async fn seed(
        &self,
        samples: Vec<PathBuf>,
        sample_text: Option<String>,
    ) -> Result<(), StoreError> {
        tokio::fs::create_dir_all(&self.flights_dir)
            .await
            .map_err(|error| StoreError::status(500, error.to_string()))?;
        if self.exists("vuelo-5") {
            return Ok(());
        }
        let track = match sample_text {
            Some(track) => track,
            None => {
                let mut found = None;
                for candidate in samples {
                    if let Ok(track) = tokio::fs::read_to_string(&candidate).await {
                        found = Some(track);
                        break;
                    }
                }
                let Some(track) = found else {
                    return Ok(());
                };
                track
            }
        };
        self.write_flight(
            "vuelo-5",
            WriteFlightBody {
                name: Some("Vuelo5".to_string()),
                file_name: Some("vuelo5.gpx".to_string()),
                trim_start_ms: None,
                trim_end_ms: None,
                tags: Some(vec![]),
                track: Some(track),
            },
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_rules_match_node() {
        assert_eq!(normalize_slug("Vuelo 5 --- Final!"), "vuelo-5-final");
        assert_eq!(normalize_slug("  Café del Mar  "), "cafe-del-mar");
        assert_eq!(normalize_slug("a"), "a");
        assert_eq!(normalize_slug(""), "");
        assert_eq!(normalize_slug("---"), "");
        assert!(is_valid_slug("vuelo-5"));
        assert!(is_valid_slug("a"));
        assert!(!is_valid_slug(""));
        assert!(!is_valid_slug("Vuelo"));
        assert!(!is_valid_slug("-a"));
        assert!(!is_valid_slug("a-"));
        assert!(!is_valid_slug("a--b"));
        assert!(!is_valid_slug("api"));
        assert!(!is_valid_slug("a b"));
        assert_eq!(slug_from_file_name("My Flight.IGC"), "my-flight");
        assert_eq!(slug_from_file_name("noext"), "noext");
        assert_eq!(slug_from_file_name(".gpx"), "track");
        assert_eq!(name_from_file_name("My Flight.IGC"), "My Flight");
        assert_eq!(name_from_file_name("x"), "x");
        let mut taken = HashSet::from(["vuelo-5".to_string()]);
        assert_eq!(allocate_slug("vuelo-5", &taken).unwrap(), "vuelo-5-2");
        taken.insert("vuelo-5-2".to_string());
        assert_eq!(allocate_slug("vuelo-5", &taken).unwrap(), "vuelo-5-3");
        assert_eq!(allocate_slug("fresh", &taken).unwrap(), "fresh");
    }

    #[test]
    fn iso_timestamps_sort_lexically() {
        let stamp = now_iso();
        assert!(stamp.ends_with('Z') && stamp.contains('T'));
        assert!(iso_from_unix_ms(0) == "1970-01-01T00:00:00.000Z");
        assert!(iso_from_unix_ms(1_700_000_000_123) == "2023-11-14T22:13:20.123Z");
    }

    #[test]
    fn deserialize_opt_ms_fractional_rounds() {
        let json = r#"{"slug":"t","name":"t","fileName":"t.gpx","trimStartMs": 1791033643687.5378, "trimEndMs": 1791038618395.9102}"#;
        let meta: FlightMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.trim_start_ms, Some(1791033643688));
        assert_eq!(meta.trim_end_ms, Some(1791038618396));
    }

    #[test]
    fn deserialize_opt_ms_integer() {
        let json = r#"{"slug":"t","name":"t","fileName":"t.gpx","trimStartMs": 1789310565000, "trimEndMs": 1789315036618}"#;
        let meta: FlightMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.trim_start_ms, Some(1789310565000));
        assert_eq!(meta.trim_end_ms, Some(1789315036618));
    }

    #[test]
    fn deserialize_opt_ms_null() {
        let json =
            r#"{"slug":"t","name":"t","fileName":"t.gpx","trimStartMs": null, "trimEndMs": null}"#;
        let meta: FlightMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.trim_start_ms, None);
        assert_eq!(meta.trim_end_ms, None);
    }

    #[test]
    fn deserialize_opt_ms_missing_field() {
        let json = r#"{"slug":"t","name":"t","fileName":"t.gpx"}"#;
        let meta: FlightMeta = serde_json::from_str(json).unwrap();
        assert_eq!(meta.trim_start_ms, None);
        assert_eq!(meta.trim_end_ms, None);
    }

    #[test]
    fn deserialize_opt_ms_round_trip_serializes_as_integer() {
        let json = r#"{"slug":"t","name":"t","fileName":"t.gpx","trimStartMs": 1791033643687.5378, "trimEndMs": 1791038618395.9102}"#;
        let meta: FlightMeta = serde_json::from_str(json).unwrap();
        let serialized = serde_json::to_string(&meta).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["trimStartMs"], 1791033643688_i64);
        assert_eq!(parsed["trimEndMs"], 1791038618396_i64);
        assert!(parsed["trimStartMs"].is_i64());
        assert!(parsed["trimEndMs"].is_i64());
    }

    #[test]
    fn deserialize_write_flight_body_fractional() {
        let json = r#"{"trimStartMs": 1791033643687.5378, "trimEndMs": 1791038618395.9102}"#;
        let body: WriteFlightBody = serde_json::from_str(json).unwrap();
        assert_eq!(body.trim_start_ms, Some(1791033643688));
        assert_eq!(body.trim_end_ms, Some(1791038618396));
    }

    #[test]
    fn prod_meta_fixtures_parse_successfully() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let fixtures_dir = format!("{}/tests/fixtures/prod-meta", manifest_dir);
        let entries = std::fs::read_dir(&fixtures_dir).expect("fixtures dir exists");
        for entry in entries {
            let entry = entry.expect("read entry");
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                let content = std::fs::read_to_string(&path)
                    .unwrap_or_else(|_| panic!("read {}", path.display()));
                let meta: FlightMeta = serde_json::from_str(&content)
                    .unwrap_or_else(|e| panic!("parse {}: {}", path.display(), e));
                assert!(!meta.slug.is_empty(), "slug missing in {}", path.display());
            }
        }
    }
}
