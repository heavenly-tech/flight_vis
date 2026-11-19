import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

/** Matches `static_files::HTML_CSP` / `csp/html-page.csp`. */
export const HTML_PAGE_CSP = readFileSync(
  path.join(repoRoot, "server-rs", "csp", "html-page.csp"),
  "utf8",
).trim();