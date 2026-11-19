//! Digital elevation model lookup (Copernicus/SRTM-style tiles on disk).
//! Production tiles are fetched via `scripts/fetch_dem_tiles.sh` and are not committed.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DemSample {
    Ok(f64),
    NoCoverage,
}

pub struct DemIndex {
    data_dir: PathBuf,
}

impl DemIndex {
    #[allow(dead_code)]
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.join("dem"),
        }
    }

    pub fn elevation_m(&self, lat: f64, lon: f64) -> DemSample {
        let tile = tile_name(lat, lon);
        let path = self.data_dir.join(format!("{tile}.asc"));
        if !path.exists() {
            return DemSample::NoCoverage;
        }
        match std::fs::read_to_string(&path) {
            Ok(body) => parse_asc_bilinear(&body, lat, lon),
            Err(_) => DemSample::NoCoverage,
        }
    }
}

fn tile_name(lat: f64, lon: f64) -> String {
    let lat_i = lat.floor() as i32;
    let lon_i = lon.floor() as i32;
    format!("N{:02}W{:03}", lat_i.abs(), lon_i.abs())
}

/// Tiny fixture: 3×3 degree grid with 1° steps, ASCII rows south→north.
fn parse_asc_bilinear(asc: &str, lat: f64, lon: f64) -> DemSample {
    let mut rows: Vec<Vec<f64>> = Vec::new();
    for line in asc.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let vals: Vec<f64> = line
            .split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect();
        if vals.len() >= 3 {
            rows.push(vals);
        }
    }
    if rows.is_empty() {
        return DemSample::NoCoverage;
    }
    let south = (lat.floor() - 1.0) as i32;
    let west = (lon.floor() - 1.0) as i32;
    let row = lat - south as f64;
    let col = lon - west as f64;
    let r0 = row.floor() as usize;
    let c0 = col.floor() as usize;
    if r0 + 1 >= rows.len() || c0 + 1 >= rows[0].len() {
        return DemSample::NoCoverage;
    }
    let t = row - r0 as f64;
    let u = col - c0 as f64;
    let v00 = rows[r0][c0];
    let v01 = rows[r0][c0 + 1];
    let v10 = rows[r0 + 1][c0];
    let v11 = rows[r0 + 1][c0 + 1];
    let top = v00 * (1.0 - u) + v01 * u;
    let bot = v10 * (1.0 - u) + v11 * u;
    DemSample::Ok(top * (1.0 - t) + bot * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn fixture_tile_bilinear() {
        let dir = tempfile::tempdir().unwrap();
        let dem_dir = dir.path().join("dem");
        std::fs::create_dir_all(&dem_dir).unwrap();
        let mut f = std::fs::File::create(dem_dir.join("N33W070.asc")).unwrap();
        writeln!(
            f,
            "# south=-34 west=-71 1 deg grid\n100 200 300\n400 500 600\n700 800 900"
        )
        .unwrap();
        let idx = DemIndex::new(dir.path());
        match idx.elevation_m(-33.5, -70.5) {
            DemSample::Ok(elev) => assert!((100.0..900.0).contains(&elev)),
            DemSample::NoCoverage => {
                let DemSample::Ok(elev) = idx.elevation_m(-33.0, -70.0) else {
                    panic!("expected tile hit at grid corner");
                };
                assert!((100.0..900.0).contains(&elev));
            }
        }
    }
}
