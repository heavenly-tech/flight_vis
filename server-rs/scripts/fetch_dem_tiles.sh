#!/usr/bin/env bash
# Download Copernicus/SRTM tiles for Chile into $FLIGHT_DATA/dem (not committed).
set -euo pipefail
DATA_DIR="${FLIGHT_DATA:-./data}"
DEM_DIR="${DATA_DIR}/dem"
mkdir -p "${DEM_DIR}"
echo "Place .asc tiles under ${DEM_DIR} (see server-rs/src/club/dem.rs naming)."
echo "Production: fetch GLO-90 or SRTM 1° tiles for your club bounds before deploy."