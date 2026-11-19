import { globeE2eCsp, readHtmlPageCsp } from "../scripts/read-html-csp.mjs";

/** Matches `static_files::HTML_CSP` / `csp/html-page.csp`. */
export const HTML_PAGE_CSP = readHtmlPageCsp();

/** Production globe CSP (e2e gateway); must match `scripts/e2e-csp-gateway.mjs`. */
export const PROD_HTML_CSP = globeE2eCsp();