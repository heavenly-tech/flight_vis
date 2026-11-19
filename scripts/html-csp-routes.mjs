/**
 * Mirror of `server-rs/src/html_csp.rs` `HTML_CSP_ROUTE_TABLE`.
 * @typedef {"strict" | "globe"} HtmlCspKind
 */

/** @type {Readonly<Record<string, HtmlCspKind>>} */
export const HTML_CSP_ROUTE_TABLE = Object.freeze({
  "/index.html": "globe",
  "/__spa_index__": "globe",
  "/club-map.html": "globe",
  "/public-map.html": "globe",
  "/club-screen.html": "globe",
  "/e2e-live.html": "globe",
  "/login": "strict",
  "/login.html": "strict",
  "/login-legacy.html": "strict",
  "/signup": "strict",
  "/signup.html": "strict",
  "/account-clubs.html": "strict",
  "/api/account/auth/magic": "strict",
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
  "/club-admin-map.html": "strict",
  "/club-admin-kiosk-layout.html": "strict",
  "/club-admin-requests.html": "strict",
  "/club-checkin-landing.html": "strict",
  "/club-checkin-expired.html": "strict",
  "/share-flight.html": "strict",
});

/**
 * @param {string} path
 * @returns {HtmlCspKind | undefined}
 */
export function policyKindForPath(path) {
  const pathOnly = path.split("?")[0];
  if (pathOnly in HTML_CSP_ROUTE_TABLE) {
    return HTML_CSP_ROUTE_TABLE[pathOnly];
  }
  if (pathOnly === "/" || /^\/live\/[^/]+/.test(pathOnly) || !pathOnly.includes(".")) {
    return HTML_CSP_ROUTE_TABLE["/__spa_index__"];
  }
  return undefined;
}