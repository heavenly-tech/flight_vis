import {
  cspForPath,
  policyKindForPath,
  readHtmlGlobeCsp,
  readHtmlStrictCsp,
  HTML_CSP_ROUTE_TABLE,
} from "../scripts/read-html-csp.mjs";

/** Strict HTML CSP (`server-rs/csp/html-strict.csp`). */
export const HTML_STRICT_CSP = readHtmlStrictCsp();

/** Globe / map HTML CSP (`server-rs/csp/html-globe.csp`). */
export const HTML_GLOBE_CSP = readHtmlGlobeCsp();

/** @deprecated use HTML_STRICT_CSP or `cspForPath` */
export const HTML_PAGE_CSP = HTML_STRICT_CSP;

export function expectedCspForPath(path: string): string {
  return cspForPath(path);
}

export { HTML_CSP_ROUTE_TABLE, policyKindForPath };