#!/usr/bin/env node
/**
 * Repo scan for Argentine voseo (CI / local). Logic mirrored in src/i18n/voseoGuard.ts.
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const r = spawnSync(
  "npx",
  ["vitest", "run", "src/i18n/voseoGuard.test.ts"],
  { cwd: root, stdio: "inherit", shell: false },
);
process.exit(r.status ?? 1);