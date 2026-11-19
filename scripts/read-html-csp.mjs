import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(fileURLToPath(import.meta.url));

/** Same string as `static_files::HTML_CSP` / `server-rs/csp/html-page.csp`. */
export function readHtmlPageCsp() {
  return readFileSync(
    path.resolve(root, "..", "server-rs", "csp", "html-page.csp"),
    "utf8",
  ).trim();
}

/** Cesium globe e2e: prod HTML CSP plus wasm/ArcGIS allowances (single img-src directive). */
export function globeE2eCsp() {
  let csp = readHtmlPageCsp();
  if (csp.includes("script-src 'self'")) {
    csp = csp.replace(
      "script-src 'self'",
      "script-src 'self' 'wasm-unsafe-eval'",
    );
  } else {
    csp = `${csp}; script-src 'self' 'wasm-unsafe-eval'`;
  }
  const imgSrc = "img-src 'self' https://services.arcgisonline.com";
  if (/img-src\s[^;]+/.test(csp)) {
    csp = csp.replace(/img-src\s[^;]+/, imgSrc);
  } else {
    csp = `${csp}; ${imgSrc}`;
  }
  return `${csp}; worker-src 'self' blob:; connect-src 'self' https://services.arcgisonline.com https://elevation3d.arcgis.com; font-src 'self'`;
}