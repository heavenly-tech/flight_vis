#!/usr/bin/env node
import { existsSync } from "node:fs";
import { HTML_CSP_ROUTE_TABLE } from "./html-csp-routes.mjs";
import { cspForPath } from "./read-html-csp.mjs";

let failed = false;
for (const route of Object.keys(HTML_CSP_ROUTE_TABLE)) {
  try {
    if (!cspForPath(route)) {
      console.error(`empty CSP for route ${route}`);
      failed = true;
    }
  } catch (err) {
    console.error(`failed to resolve CSP for route ${route}: ${err instanceof Error ? err.message : String(err)}`);
    failed = true;
  }
  if (!route.endsWith(".html")) continue;
  const file = route.slice(1);
  const distPath = `dist/${file}`;
  if (!existsSync(distPath)) {
    console.error(`missing built HTML for CSP table row ${route}: ${distPath}`);
    failed = true;
  }
}
if (failed) process.exit(1);