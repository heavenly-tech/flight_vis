import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { policyKindForPath, HTML_CSP_ROUTE_TABLE } from "./html-csp-routes.mjs";

const root = path.dirname(fileURLToPath(import.meta.url));
const cspDir = path.resolve(root, "..", "server-rs", "csp");

export function readHtmlStrictCsp() {
  return readFileSync(path.join(cspDir, "html-strict.csp"), "utf8").trim();
}

export function readHtmlGlobeCsp() {
  return readFileSync(path.join(cspDir, "html-globe.csp"), "utf8").trim();
}

/** @deprecated name kept for imports; strict login/form policy */
export function readHtmlPageCsp() {
  return readHtmlStrictCsp();
}

/** CSP header string for a request path (prod server or e2e gateway). */
export function cspForPath(urlPath) {
  const kind = policyKindForPath(urlPath);
  if (kind === "globe") return readHtmlGlobeCsp();
  if (kind === "strict") return readHtmlStrictCsp();
  throw new Error(`no HTML CSP mapping for path ${urlPath}`);
}

/** Production globe routes (e2e gateway default for viewer HTML). */
export function globeE2eCsp() {
  return readHtmlGlobeCsp();
}

export { HTML_CSP_ROUTE_TABLE, policyKindForPath };