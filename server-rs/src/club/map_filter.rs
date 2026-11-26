use crate::club::{
    dem::{DemIndex, DemSample},
    geo::{egm96_undulation_m, haversine_km, METERS_PER_FOOT},
    map::{DeviceFix, TailLivePosition},
    visibility::FlightVisibility,
};
use crate::live::LiveFix;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AltitudeMode {
    Agl,
    Msl,
}

impl AltitudeMode {
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("msl") {
            Self::Msl
        } else {
            Self::Agl
        }
    }
}

#[derive(Debug, Clone)]
pub struct MapViewFilter {
    pub center_lat: f64,
    pub center_lon: f64,
    pub radius_km: f64,
    pub show_public_tracks: bool,
    pub cap: usize,
    pub altitude_mode: AltitudeMode,
    pub altitude_min_ft: f64,
    #[allow(dead_code)]
    pub club_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AltitudeEval {
    Pass,
    Fail,
    NoDemFallbackMsl,
}

pub fn evaluate_public_altitude(
    fix: &LiveFix,
    mode: AltitudeMode,
    min_ft: f64,
    dem: &DemIndex,
) -> AltitudeEval {
    let Some(alt_m) = fix.alt else {
        return AltitudeEval::Fail;
    };
    let und = egm96_undulation_m(fix.lat, fix.lon);
    let msl_m = match fix.alt_ref.as_deref() {
        Some("msl") => alt_m,
        Some("hae") | None => alt_m - und,
        _ => alt_m - und,
    };
    match mode {
        AltitudeMode::Msl => {
            let ft = msl_m / METERS_PER_FOOT;
            if ft >= min_ft {
                AltitudeEval::Pass
            } else {
                AltitudeEval::Fail
            }
        }
        AltitudeMode::Agl => match dem.elevation_m(fix.lat, fix.lon) {
            DemSample::Ok(terrain) => {
                let agl_m = msl_m - terrain;
                if agl_m / METERS_PER_FOOT >= min_ft {
                    AltitudeEval::Pass
                } else {
                    AltitudeEval::Fail
                }
            }
            DemSample::NoCoverage => {
                let ft = msl_m / METERS_PER_FOOT;
                if ft >= min_ft {
                    AltitudeEval::NoDemFallbackMsl
                } else {
                    AltitudeEval::Fail
                }
            }
        },
    }
}

/// Drop or keep a **non-member** public track; members are exempt (caller skips).
pub fn public_track_passes(row: &DeviceFix, filter: &MapViewFilter, dem: &DemIndex) -> bool {
    if !filter.show_public_tracks {
        return false;
    }
    let km = haversine_km(
        filter.center_lat,
        filter.center_lon,
        row.fix.lat,
        row.fix.lon,
    );
    if km > filter.radius_km {
        return false;
    }
    matches!(
        evaluate_public_altitude(&row.fix, filter.altitude_mode, filter.altitude_min_ft, dem),
        AltitudeEval::Pass | AltitudeEval::NoDemFallbackMsl
    )
}

pub fn apply_public_cap(
    mut positions: Vec<TailLivePosition>,
    cap: usize,
    center: (f64, f64),
) -> Vec<TailLivePosition> {
    if positions.len() <= cap {
        return positions;
    }
    positions.sort_by(|a, b| {
        let da = haversine_km(center.0, center.1, a.fix.lat, a.fix.lon);
        let db = haversine_km(center.0, center.1, b.fix.lat, b.fix.lon);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
    });
    positions.truncate(cap);
    positions.sort_by(|a, b| a.tail.cmp(&b.tail));
    positions
}

pub fn visibility_is_public(vis: &str) -> bool {
    FlightVisibility::parse(vis)
        .map(|v| v == FlightVisibility::Public)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::club::map::DeviceFix;
    use crate::live::LiveFix;

    fn fix_at(lat: f64, lon: f64, alt_m: f64) -> DeviceFix {
        DeviceFix {
            tail: "N1".into(),
            flight_id: "f1".into(),
            pilot_id: "p1".into(),
            pilot_name: "Guest".into(),
            visibility: "public".into(),
            club_id: "c1".into(),
            trail_minutes: 5,
            fix: LiveFix {
                t: 0,
                lat,
                lon,
                alt: Some(alt_m),
                alt_ref: Some("hae".into()),
                speed: None,
                course: None,
                accuracy: None,
            },
        }
    }

    #[test]
    fn radius_inside_outside() {
        let dem = DemIndex::new(std::path::Path::new("/nonexistent"));
        let filter = MapViewFilter {
            center_lat: -33.0,
            center_lon: -70.0,
            radius_km: 10.0,
            show_public_tracks: true,
            cap: 50,
            altitude_mode: AltitudeMode::Msl,
            altitude_min_ft: 0.0,
            club_id: "c1".into(),
        };
        let near = fix_at(-33.05, -70.0, 2000.0);
        let far = fix_at(-34.5, -70.0, 2000.0);
        assert!(public_track_passes(&near, &filter, &dem));
        assert!(!public_track_passes(&far, &filter, &dem));
    }

    #[test]
    fn cap_keeps_nearest() {
        let center = (-33.0, -70.0);
        let pos = vec![
            TailLivePosition {
                tail: "FAR".into(),
                flight_id: "1".into(),
                pilot_id: "p".into(),
                pilot_name: None,
                fix: LiveFix {
                    t: 0,
                    lat: -34.0,
                    lon: -70.0,
                    alt: None,
                    alt_ref: None,
                    speed: None,
                    course: None,
                    accuracy: None,
                },
                visibility: "public".into(),
                trail_minutes: 5,
            },
            TailLivePosition {
                tail: "NEAR".into(),
                flight_id: "2".into(),
                pilot_id: "p".into(),
                pilot_name: None,
                fix: LiveFix {
                    t: 0,
                    lat: -33.01,
                    lon: -70.0,
                    alt: None,
                    alt_ref: None,
                    speed: None,
                    course: None,
                    accuracy: None,
                },
                visibility: "public".into(),
                trail_minutes: 5,
            },
        ];
        let capped = apply_public_cap(pos, 1, center);
        assert_eq!(capped.len(), 1);
        assert_eq!(capped[0].tail, "NEAR");
    }
}
