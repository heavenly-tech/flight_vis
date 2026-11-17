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
pub fn bbox_for_radius_km(lat: f64, lon: f64, radius_km: f64) -> (f64, f64, f64, f64) {
    let dlat = radius_km / EARTH_RADIUS_KM * (180.0 / PI);
    let dlon = radius_km / (EARTH_RADIUS_KM * lat.to_radians().cos()) * (180.0 / PI);
    (lat - dlat, lat + dlat, lon - dlon, lon + dlon)
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
}