import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(fileURLToPath(import.meta.url));

/** Same string as `static_files::HTML_CSP` in the Rust API. */
export function readHtmlPageCsp() {
  return readFileSync(path.resolve(root, "..", "csp", "html-page.csp"), "utf8").trim();
}

/** Cesium globe e2e: prod HTML CSP plus wasm/ArcGIS allowances. */
export function globeE2eCsp() {
  const base = readHtmlPageCsp();
  const script = "script-src 'self' 'wasm-unsafe-eval'";
  const withWasm = base.includes("script-src 'self'")
    ? base.replace("script-src 'self'", script)
    : `${base}; ${script}`;
  return `${withWasm}; worker-src 'self' blob:; connect-src 'self' https://services.arcgisonline.com https://elevation3d.arcgis.com; font-src 'self'; img-src 'self' https://services.arcgisonline.com`;
}