#!/usr/bin/env node
/**
 * Validates locale catalogs: same keys as canonical (es), plus UI English-literal guard.
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const localesDir = path.join(root, "locales");
const manifestPath = path.join(localesDir, "manifest.json");

function flattenKeys(obj, prefix = "") {
  const keys = [];
  for (const [k, v] of Object.entries(obj)) {
    const p = prefix ? `${prefix}.${k}` : k;
    if (v !== null && typeof v === "object" && !Array.isArray(v)) {
      keys.push(...flattenKeys(v, p));
    } else {
      keys.push(p);
    }
  }
  return keys;
}

function loadJson(p) {
  return JSON.parse(fs.readFileSync(p, "utf8"));
}

function catalogKeysOnly(data) {
  const { _meta, ...rest } = data;
  return flattenKeys(rest).sort();
}

const manifest = loadJson(manifestPath);
const canonical = manifest.canonical ?? "es";
const canonicalPath = path.join(localesDir, `${canonical}.json`);
if (!fs.existsSync(canonicalPath)) {
  console.error(`missing canonical catalog ${canonicalPath}`);
  process.exit(1);
}
const canonKeys = catalogKeysOnly(loadJson(canonicalPath));
const localeIds = Object.keys(manifest.locales ?? {});
let failed = false;

for (const id of localeIds) {
  const file = path.join(localesDir, `${id}.json`);
  if (!fs.existsSync(file)) {
    console.error(`locale ${id} in manifest but missing ${file}`);
    failed = true;
    continue;
  }
  const keys = catalogKeysOnly(loadJson(file));
  const missing = canonKeys.filter((k) => !keys.includes(k));
  const extra = keys.filter((k) => !canonKeys.includes(k));
  if (missing.length || extra.length) {
    failed = true;
    console.error(`locale ${id}: missing ${missing.length}, extra ${extra.length}`);
    for (const m of missing.slice(0, 20)) console.error(`  - missing: ${m}`);
    for (const e of extra.slice(0, 20)) console.error(`  + extra: ${e}`);
  }
}

const orphanFiles = fs
  .readdirSync(localesDir)
  .filter((f) => f.endsWith(".json") && f !== "manifest.json")
  .map((f) => f.replace(/\.json$/, ""))
  .filter((id) => !localeIds.includes(id));
if (orphanFiles.length) {
  failed = true;
  console.error("orphan locale files not in manifest:", orphanFiles.join(", "));
}

const EN_PATTERNS = [
  /\bLogin\b/,
  /\bRegister\b/,
  /\bPassword\b/,
  /\bEmail\b/,
  /\bApprove\b/,
  /\bReject\b/,
  /\bDelete\b/,
  /\bAdmin\b/,
  /\bSign in\b/,
  /\bLog out\b/,
  /\bClub map\b/,
  /\bPublic live map\b/,
  /\bLoading release\b/,
  /\bDownload APK\b/,
  /\bSideload\b/,
  /\bMissing flight\b/,
  /\bInvalid display token\b/,
  /\bDisplay activated\b/,
  /\bAdmin login required\b/,
  /\bLive flight\b/,
  /\bChecking session\b/,
];

const scanRoots = [
  path.join(root, "src/club"),
  ...fs
    .readdirSync(root)
    .filter(
      (f) =>
        f.endsWith(".html") &&
        (f.startsWith("club") || f.startsWith("share") || f === "app.html" || f === "public-map.html"),
    ),
].flatMap((p) => (fs.existsSync(p) && fs.statSync(p).isDirectory() ? fs.readdirSync(p).map((f) => path.join(p, f)) : [p]));

const ALLOW_SUBSTR = ["GPX", "Kiosk", "qr", "data-i18n", "api/", "/src/"];

const uiFiles = scanRoots.filter((p) => fs.existsSync(p) && (p.endsWith(".html") || p.endsWith(".ts")));

for (const file of uiFiles) {
  const text = fs.readFileSync(file, "utf8");
  if (file.includes("/i18n/")) continue;
  if (ALLOW_SUBSTR.some((s) => text.includes(s)) && !file.endsWith(".html")) continue;
  for (const pat of EN_PATTERNS) {
    if (file.endsWith(".html") && text.includes("data-i18n")) {
      continue;
    }
    if (pat.test(text)) {
      failed = true;
      console.error(`hardcoded English (${pat}): ${path.relative(root, file)}`);
      break;
    }
  }
}

if (failed) {
  process.exit(1);
}
console.log(`i18n OK: ${localeIds.length} locales, ${canonKeys.length} keys (canonical ${canonical})`);