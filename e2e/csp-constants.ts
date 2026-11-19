import { globeE2eCsp, readHtmlPageCsp } from "../scripts/read-html-csp.mjs";

/** Matches `static_files::HTML_CSP` / `server-rs/csp/html-page.csp`. */
export const HTML_PAGE_CSP = readHtmlPageCsp();

/** Production CSP on release server HTML (same as HTML_PAGE_CSP). */
export const PROD_HTML_CSP = HTML_PAGE_CSP;

/** Globe CSP e2e gateway (same policy string). */
export const GLOBE_E2E_CSP = globeE2eCsp();