/**
 * Mirror of `server-rs/src/html_csp.rs` `HTML_CSP_ROUTE_TABLE`.
 * @typedef {"strict" | "map" | "globe"} HtmlCspKind
 */

/** @type {Readonly<Record<string, HtmlCspKind>>} */
export const HTML_CSP_ROUTE_TABLE = Object.freeze({
  "/index.html": "globe",
  "/__spa_index__": "globe",
  "/share-flight.html": "globe",
  "/club-admin-map.html": "map",
  "/public-map.html": "map",
  "/club-screen.html": "map",
  "/login": "strict",
  "/login.html": "strict",
  "/login-legacy.html": "strict",
  "/signup": "strict",
  "/signup.html": "strict",
  "/account-clubs.html": "strict",
  "/account.html": "strict",
  "/api/account/auth/magic": "strict",
  "/api/club/auth/magic": "strict",
  "/app.html": "strict",
  "/club-pilot.html": "strict",
  "/club-flight.html": "strict",
  "/club-register.html": "strict",
  "/club-signup.html": "strict",
  "/club-suspended.html": "strict",
  "/club-admin-types.html": "strict",
  "/club-admin-tails.html": "strict",
  "/club-admin-pilots.html": "strict",
  "/club-admin-checkouts.html": "strict",
  "/club-admin-kiosk-layout.html": "strict",
  "/club-admin-requests.html": "strict",
  "/club-admin-checkin-qr.html": "strict",
  "/club-map.html": "strict",
  "/e2e-live.html": "strict",
  "/club-checkin-landing.html": "strict",
  "/club-checkin-expired.html": "strict",
});

const SPA_GLOBE = HTML_CSP_ROUTE_TABLE["/__spa_index__"];

/**
 * @param {string} path
 * @returns {HtmlCspKind | undefined}
 */
export function policyKindForPath(path) {
  const pathOnly = path.split("?")[0];
  if (pathOnly in HTML_CSP_ROUTE_TABLE) {
    return HTML_CSP_ROUTE_TABLE[pathOnly];
  }
  if (pathOnly === "/" || /^\/live\/[^/]+/.test(pathOnly)) {
    return SPA_GLOBE;
  }
  // Extensionless single-segment paths (except /c/..., /s/..., /api/...) fall back to SPA
  if (!pathOnly.includes(".") && !pathOnly.endsWith("/")) {
    const segments = pathOnly.split("/").filter(Boolean);
    if (segments.length === 1 && segments[0] !== "c" && segments[0] !== "s" && segments[0] !== "api") {
      return SPA_GLOBE;
    }
  }
  return undefined;
}
