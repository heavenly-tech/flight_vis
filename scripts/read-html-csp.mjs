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

/** Production CSP for globe e2e gateway (matches release HTML responses). */
export function globeE2eCsp() {
  return readHtmlPageCsp();
}