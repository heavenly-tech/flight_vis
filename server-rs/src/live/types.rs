use serde::{Deserialize, Serialize};

/// Fix timestamp in Unix milliseconds (UTC).
pub type FixTime = i64;

pub const ALT_REF_WGS84_ELLIPSOID: &str = "wgs84_ellipsoid";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LiveFix {
    pub t: FixTime,
    pub lat: f64,
    pub lon: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub course: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accuracy: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionPhase {
    Active,
    Ended,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveSessionSummary {
    pub id: String,
    pub device: String,
    pub phase: SessionPhase,
    pub started_at: FixTime,
    pub ended_at: Option<FixTime>,
    pub last_fix_at: Option<FixTime>,
    pub fix_count: usize,
}

fn deserialize_present<'de, D>(de: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let _ = serde::de::IgnoredAny::deserialize(de)?;
    Ok(true)
}

#[derive(Deserialize)]
struct PeekFixesKey {
    #[serde(default, rename = "fixes", deserialize_with = "deserialize_present")]
    has_fixes: bool,
}

#[derive(Debug, Deserialize)]
pub struct IngestBatch {
    pub fixes: Vec<IngestFixInput>,
    #[serde(default, alias = "flightId")]
    pub flight_id: Option<String>,
    #[serde(default)]
    pub seq: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct IngestFixInput {
    pub t: FixTime,
    pub lat: f64,
    pub lon: f64,
    #[serde(default)]
    pub alt: Option<f64>,
    #[serde(default)]
    pub speed: Option<f64>,
    #[serde(default)]
    pub course: Option<f64>,
    #[serde(default)]
    pub accuracy: Option<f64>,
    #[serde(default, alias = "flightId")]
    pub flight_id: Option<String>,
    #[serde(default)]
    pub seq: Option<i64>,
}

/// Parse either the batch envelope (`{"fixes":[...], "flight_id"|"flightId"?: ..., "seq"?: ...}`)
/// or a single-fix object (`{"t":..., "lat":..., "lon":..., "flight_id"|"flightId"?: ..., "seq"?: ...}`).
/// If the top-level JSON object contains a `"fixes"` key (even malformed or `null`), it is parsed
/// strictly as a batch rather than falling through to the single-fix form.
pub fn parse_ingest_payload(body: &[u8]) -> Result<IngestBatch, &'static str> {
    if let Ok(peek) = serde_json::from_slice::<PeekFixesKey>(body) {
        if peek.has_fixes {
            return serde_json::from_slice::<IngestBatch>(body).map_err(|_| "invalid json");
        }
    }
    let single: IngestFixInput = serde_json::from_slice(body).map_err(|_| "invalid json")?;
    Ok(IngestBatch {
        fixes: vec![single],
        flight_id: None,
        seq: None,
    })
}

pub fn knots_to_mps(knots: f64) -> f64 {
    knots * 0.514_444
}

pub fn valid_lat_lon(lat: f64, lon: f64) -> bool {
    (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon)
}
