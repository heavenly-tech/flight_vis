/**
 * Builds a 0.25° EGM96 undulation grid from NGA sample data (egm96-universal dev tool).
 * Output: public/geoid/egm96-025deg.bin + egm96-025deg.json
 */
import fs from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { fileURLToPath } from "node:url";
import { meanSeaLevel } from "egm96-universal";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const outDir = path.join(root, "public", "geoid");
const stepDeg = 0.25;
const latMin = -90;
const latMax = 90;
const lonMin = -180;
const lonMax = 180 - stepDeg;

const latCount = Math.round((latMax - latMin) / stepDeg) + 1;
const lonCount = Math.round((lonMax - lonMin) / stepDeg) + 1;
const values = new Float32Array(latCount * lonCount);

let i = 0;
for (let lat = latMax; lat >= latMin; lat -= stepDeg) {
  for (let lon = lonMin; lon <= lonMax; lon += stepDeg) {
    values[i] = meanSeaLevel(lat, lon);
    i += 1;
  }
}

fs.mkdirSync(outDir, { recursive: true });
const binPath = path.join(outDir, "egm96-025deg.bin");
fs.writeFileSync(binPath, Buffer.from(values.buffer));
const meta = {
  source: "NGA EGM96 via egm96-universal",
  license: "MIT (grid generator); NGA geoid data (public domain)",
  stepDeg,
  latMin,
  latMax,
  lonMin,
  lonMax,
  latCount,
  lonCount,
};
const jsonPath = path.join(outDir, "egm96-025deg.json");
fs.writeFileSync(jsonPath, JSON.stringify(meta, null, 2));
const binBytes = Buffer.from(values.buffer);
fs.writeFileSync(`${binPath}.gz`, zlib.gzipSync(binBytes, { level: 9 }));
fs.writeFileSync(
  `${binPath}.br`,
  zlib.brotliCompressSync(binBytes, {
    params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11 },
  }),
);
const jsonBytes = fs.readFileSync(jsonPath);
fs.writeFileSync(`${jsonPath}.gz`, zlib.gzipSync(jsonBytes, { level: 9 }));
fs.writeFileSync(
  `${jsonPath}.br`,
  zlib.brotliCompressSync(jsonBytes, {
    params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11 },
  }),
);
console.log(`Wrote ${binPath} (${(values.byteLength / 1024 / 1024).toFixed(2)} MiB)`);