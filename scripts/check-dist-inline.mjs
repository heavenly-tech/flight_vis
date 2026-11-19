#!/usr/bin/env node
/**
 * Fail if built HTML still contains inline scripts (other than empty markers).
 * External scripts via src= and live-route-boot.js are allowed.
 */
import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

const dist = path.resolve("dist");
const bad = [];

for (const name of readdirSync(dist)) {
  if (!name.endsWith(".html")) continue;
  const html = readFileSync(path.join(dist, name), "utf8");
  const inline = [...html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/gi)];
  for (const m of inline) {
    const body = m[1].trim();
    if (body.length > 0) bad.push(`${name}: inline script body (${body.length} chars)`);
  }
}

if (bad.length) {
  console.error("dist/ must not ship inline script bodies:\n" + bad.join("\n"));
  process.exit(1);
}
console.log("dist/: no inline script bodies");