//! Club live-ingest validation (parity with legacy live engine + speed gate).
use crate::live::engine::{
    MAX_FUTURE_MS, MAX_INGEST_BATCH, MAX_INGEST_PER_MINUTE, MIN_FIX_MS,
};
use crate::live::types::{valid_lat_lon, LiveFix};
use std::f64::consts::PI;

/// WGS84 mean Earth radius (m).
const EARTH_RADIUS_M: f64 = 6_371_000.0;
/// Max ground speed between fixes (600 km/h).
const MAX_SPEED_MPS: f64 = 600_000.0 / 3600.0;

#[derive(Debug, Clone, PartialEq)]
pub enum FixReject {
    InvalidLatLon,
    TimestampTooOld,
    TimestampTooFarFuture,
    BatchTooLarge,
    SpeedTooHigh,
    NonPositiveDt,
}

impl FixReject {
    pub fn reason(&self) -> &'static str {
        match self {
            Self::InvalidLatLon => "invalid lat/lon",
            Self::TimestampTooOld => "timestamp too old",
            Self::TimestampTooFarFuture => "timestamp too far in future",
            Self::BatchTooLarge => "batch too large",
            Self::SpeedTooHigh => "speed too high",
            Self::NonPositiveDt => "non-positive dt",
        }
    }
}

pub fn validate_batch_len(n: usize) -> Result<(), FixReject> {
    if n > MAX_INGEST_BATCH {
        return Err(FixReject::BatchTooLarge);
    }
    Ok(())
}

/// Normalize device time: use server `now` when device timestamp is unusable.
pub fn normalize_fix_time(fix: &mut LiveFix, now: i64) {
    if fix.t < MIN_FIX_MS || fix.t > now + MAX_FUTURE_MS {
        fix.t = now;
    }
}

pub fn validate_fix_coords_and_time(fix: &LiveFix, now: i64) -> Result<(), FixReject> {
    if !valid_lat_lon(fix.lat, fix.lon) {
        return Err(FixReject::InvalidLatLon);
    }
    if fix.t < MIN_FIX_MS {
        return Err(FixReject::TimestampTooOld);
    }
    if fix.t > now + MAX_FUTURE_MS {
        return Err(FixReject::TimestampTooFarFuture);
    }
    Ok(())
}

fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let rlat1 = lat1.to_radians();
    let rlat2 = lat2.to_radians();
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2) + rlat1.cos() * rlat2.cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    EARTH_RADIUS_M * c
}

/// Speed gate vs last **accepted** fix (server receive times).
pub fn speed_ok(
    prev_lat: f64,
    prev_lon: f64,
    prev_received_ms: i64,
    fix: &LiveFix,
    received_ms: i64,
) -> Result<(), FixReject> {
    let dt_ms = received_ms - prev_received_ms;
    if dt_ms <= 0 {
        return Err(FixReject::NonPositiveDt);
    }
    let dist = haversine_m(prev_lat, prev_lon, fix.lat, fix.lon);
    let speed = dist / (dt_ms as f64 / 1000.0);
    if speed > MAX_SPEED_MPS {
        return Err(FixReject::SpeedTooHigh);
    }
    Ok(())
}

pub fn allow_ingest_rate(hits: &mut Vec<i64>, now: i64) -> bool {
    let window_start = now - 60_000;
    hits.retain(|t| *t >= window_start);
    if hits.len() >= MAX_INGEST_PER_MINUTE {
        return false;
    }
    hits.push(now);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::live::types::ALT_REF_WGS84_ELLIPSOID;

    #[test]
    fn rejects_impossible_speed() {
        let prev_t = 1_700_000_000_000_i64;
        let fix = LiveFix {
            t: prev_t + 1000,
            lat: -33.0,
            lon: -70.0,
            alt: Some(1000.0),
            alt_ref: Some(ALT_REF_WGS84_ELLIPSOID.to_string()),
            speed: None,
            course: None,
            accuracy: None,
        };
        assert_eq!(
            speed_ok(-33.0, -70.0, prev_t, &fix, prev_t + 1000),
            Err(FixReject::SpeedTooHigh)
        );
    }

    #[test]
    fn allows_reasonable_speed() {
        let prev_t = 1_700_000_000_000_i64;
        let fix = LiveFix {
            t: prev_t + 10_000,
            lat: -33.0001,
            lon: -70.0001,
            alt: Some(1000.0),
            alt_ref: Some(ALT_REF_WGS84_ELLIPSOID.to_string()),
            speed: None,
            course: None,
            accuracy: None,
        };
        assert!(speed_ok(-33.0, -70.0, prev_t, &fix, prev_t + 10_000).is_ok());
    }

    #[test]
    fn batch_cap() {
        assert!(validate_batch_len(MAX_INGEST_BATCH).is_ok());
        assert_eq!(
            validate_batch_len(MAX_INGEST_BATCH + 1),
            Err(FixReject::BatchTooLarge)
        );
    }
}