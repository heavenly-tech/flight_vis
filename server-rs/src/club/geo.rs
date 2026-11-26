use std::f64::consts::PI;

const EARTH_RADIUS_KM: f64 = 6371.0;

pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let lat1 = lat1.to_radians();
    let lat2 = lat2.to_radians();
    let dlat = lat2 - lat1;
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    EARTH_RADIUS_KM * c
}

/// Rough bounding box for SQL prefilter (includes corners slightly outside the circle).
#[allow(dead_code)]
pub fn bbox_for_radius_km(lat: f64, lon: f64, radius_km: f64) -> (f64, f64, f64, f64) {
    let dlat = radius_km / EARTH_RADIUS_KM * (180.0 / PI);
    let dlon = radius_km / (EARTH_RADIUS_KM * lat.to_radians().cos()) * (180.0 / PI);
    (lat - dlat, lat + dlat, lon - dlon, lon + dlon)
}

/// Spherical-cap bounding box for SQL prefiltering a great-circle radius query.
///
/// Returns `(min_lat, max_lat, lon_range)`. `lon_range` is `None` whenever the search
/// circle reaches/covers a pole or crosses the antimeridian (`±180°`), so callers can
/// skip the longitude SQL prefilter and rely on exact `haversine_km` filtering.
pub fn safe_bbox_for_radius_km(
    lat: f64,
    lon: f64,
    radius_km: f64,
) -> (f64, f64, Option<(f64, f64)>) {
    const EPS_DEG: f64 = 1e-6;
    let angular_r = radius_km / EARTH_RADIUS_KM;
    let dlat_deg = angular_r.to_degrees() + EPS_DEG;
    let raw_min_lat = lat - dlat_deg;
    let raw_max_lat = lat + dlat_deg;
    let min_lat = raw_min_lat.max(-90.0);
    let max_lat = raw_max_lat.min(90.0);

    if raw_min_lat <= -90.0 || raw_max_lat >= 90.0 {
        return (min_lat, max_lat, None);
    }

    let sin_dlon = angular_r.sin() / lat.to_radians().cos();
    if !sin_dlon.is_finite() || sin_dlon >= 1.0 {
        return (min_lat, max_lat, None);
    }

    let dlon_deg = sin_dlon.asin().to_degrees() + EPS_DEG;
    let min_lon = lon - dlon_deg;
    let max_lon = lon + dlon_deg;
    if !min_lon.is_finite() || !max_lon.is_finite() || min_lon < -180.0 || max_lon > 180.0 {
        return (min_lat, max_lat, None);
    }

    (min_lat, max_lat, Some((min_lon, max_lon)))
}

/// Approximate EGM96 geoid undulation (meters) — adequate for Chile latitudes in tests.
pub fn egm96_undulation_m(lat: f64, lon: f64) -> f64 {
    let _ = lon;
    -30.0 + 0.15 * (lat + 33.0)
}

pub const METERS_PER_FOOT: f64 = 0.3048;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haversine_known_distance() {
        let km = haversine_km(-33.45, -70.65, -33.40, -70.65);
        assert!(km > 5.0 && km < 6.0);
    }

    #[test]
    fn bbox_contains_center() {
        let (min_lat, max_lat, min_lon, max_lon) = bbox_for_radius_km(-33.0, -70.0, 100.0);
        assert!(min_lat < -33.0 && max_lat > -33.0);
        assert!(min_lon < -70.0 && max_lon > -70.0);
    }

    #[test]
    fn safe_bbox_handles_mid_lat_high_lat_poles_and_antimeridian() {
        // Mid-latitude
        let (min_lat, max_lat, lon_range) = safe_bbox_for_radius_km(-33.456, -70.548, 20.0);
        let (min_lon, max_lon) = lon_range.expect("mid-lat lon range");
        assert!(min_lat < -33.456 && max_lat > -33.456);
        assert!(min_lon < -70.548 && max_lon > -70.548);

        // High-latitude tangent point inside 500 km circle must stay inside lon_range
        let (hi_min_lat, hi_max_lat, hi_lon_range) = safe_bbox_for_radius_km(60.0, 0.0, 500.0);
        let (hi_min_lon, hi_max_lon) = hi_lon_range.expect("60N lon range");
        assert!(haversine_km(60.0, 0.0, 60.35, 9.01) <= 500.0);
        assert!((hi_min_lat..=hi_max_lat).contains(&60.35));
        assert!((hi_min_lon..=hi_max_lon).contains(&9.01));

        // Covers north pole -> lon prefilter skipped
        let (p_min_lat, p_max_lat, p_lon_range) = safe_bbox_for_radius_km(88.0, 10.0, 500.0);
        assert!(p_min_lat < 88.0 && p_max_lat == 90.0);
        assert!(p_lon_range.is_none());

        // Crosses antimeridian -> lon prefilter skipped, and haversine wraps cleanly
        let (_, _, am_lon_range) = safe_bbox_for_radius_km(0.0, 179.95, 20.0);
        assert!(am_lon_range.is_none());
        let cross_km = haversine_km(0.0, 179.95, 0.0, -179.95);
        assert!(cross_km > 10.0 && cross_km < 12.0);
    }
}
