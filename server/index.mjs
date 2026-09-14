import { createReadStream, existsSync, statSync } from "node:fs";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { serve } from "@hono/node-server";
import { Hono } from "hono";

const root = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.resolve(root, "../dist");
const dataDir = path.resolve(process.env.FLIGHT_DATA || path.resolve(root, "../data"));
const flightsDir = path.join(dataDir, "flights");
const port = Number(process.env.PORT || 8080);

const RESERVED = new Set(["api", "assets", "cesiumstatic", "samples", "static", "health"]);
const MIME = {
  ".css": "text/css; charset=utf-8",
  ".gpx": "application/gpx+xml",
  ".html": "text/html; charset=utf-8",
  ".ico": "image/x-icon",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".png": "image/png",
  ".svg": "image/svg+xml",
  ".wasm": "application/wasm",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
};

function normalizeSlug(input) {
  return String(input || "")
    .trim()
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

function isValidSlug(slug) {
  return /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(slug) && !RESERVED.has(slug);
}

function flightDir(slug) {
  return path.join(flightsDir, slug);
}

async function readMeta(slug) {
  const dir = flightDir(slug);
  const metaPath = path.join(dir, "meta.json");
  const trackPath = path.join(dir, "track");
  if (!existsSync(metaPath) || !existsSync(trackPath)) return null;
  const meta = JSON.parse(await readFile(metaPath, "utf8"));
  const track = await readFile(trackPath, "utf8");
  return { ...meta, slug, track };
}

async function writeFlight(slug, body) {
  const dir = flightDir(slug);
  await mkdir(dir, { recursive: true });
  const existing = existsSync(path.join(dir, "meta.json"))
    ? JSON.parse(await readFile(path.join(dir, "meta.json"), "utf8"))
    : {};
  const track = body.track ?? (existsSync(path.join(dir, "track")) ? await readFile(path.join(dir, "track"), "utf8") : null);
  if (!track) {
    const error = new Error("Track file is required");
    error.status = 400;
    throw error;
  }
  const meta = {
    slug,
    name: body.name || existing.name || slug,
    fileName: body.fileName || existing.fileName || "track",
    trimStartMs: body.trimStartMs ?? existing.trimStartMs ?? null,
    trimEndMs: body.trimEndMs ?? existing.trimEndMs ?? null,
    tags: Array.isArray(body.tags) ? body.tags : existing.tags || [],
    updatedAt: new Date().toISOString(),
    createdAt: existing.createdAt || new Date().toISOString(),
  };
  await writeFile(path.join(dir, "track"), track);
  await writeFile(path.join(dir, "meta.json"), JSON.stringify(meta, null, 2));
  return { ...meta, track };
}

async function seed() {
  await mkdir(flightsDir, { recursive: true });
  if (existsSync(path.join(flightDir("vuelo-5"), "meta.json"))) return;
  const candidates = [
    path.resolve(root, "../samples/vuelo5.gpx"),
    path.resolve(root, "../public/samples/vuelo5.gpx"),
    path.join(distDir, "samples/vuelo5.gpx"),
  ];
  const sample = candidates.find((file) => existsSync(file));
  if (!sample) return;
  await writeFlight("vuelo-5", {
    name: "Vuelo5",
    fileName: "vuelo5.gpx",
    trimStartMs: null,
    trimEndMs: null,
    tags: [],
    track: await readFile(sample, "utf8"),
  });
}

const app = new Hono();

app.get("/api/health", (c) => c.json({ ok: true }));

app.get("/api/flights", async (c) => {
  await mkdir(flightsDir, { recursive: true });
  const slugs = (await readdir(flightsDir, { withFileTypes: true }))
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .filter(isValidSlug);
  const items = [];
  for (const slug of slugs) {
    const record = await readMeta(slug);
    if (!record) continue;
    items.push({ slug: record.slug, name: record.name, updatedAt: record.updatedAt });
  }
  items.sort((a, b) => b.updatedAt.localeCompare(a.updatedAt));
  return c.json(items);
});

app.get("/api/flights/:slug", async (c) => {
  const slug = normalizeSlug(c.req.param("slug"));
  if (!isValidSlug(slug)) return c.json({ error: "Invalid slug" }, 400);
  const record = await readMeta(slug);
  if (!record) return c.json({ error: "Not found" }, 404);
  return c.json(record);
});

app.put("/api/flights/:slug", async (c) => {
  const slug = normalizeSlug(c.req.param("slug"));
  if (!isValidSlug(slug)) return c.json({ error: "Invalid slug" }, 400);
  const body = await c.req.json();
  try {
    return c.json(await writeFlight(slug, body));
  } catch (error) {
    return c.json({ error: error.message }, error.status || 500);
  }
});

app.get("*", async (c) => {
  const urlPath = decodeURIComponent(c.req.path);
  const relative = path.normalize(urlPath).replace(/^(\.\.(\/|\\|$))+/, "").replace(/^\/+/, "");
  const file = path.resolve(distDir, relative);
  if (relative && file.startsWith(distDir + path.sep) && existsSync(file) && statSync(file).isFile()) {
    const type = MIME[path.extname(file).toLowerCase()] || "application/octet-stream";
    return c.body(createReadStream(file), 200, { "Content-Type": type });
  }
  const index = path.join(distDir, "index.html");
  if (!existsSync(index)) return c.text("Build the frontend first", 500);
  return c.html(await readFile(index, "utf8"));
});

await seed();

serve({ fetch: app.fetch, port }, (info) => {
  console.log(`flight_vis listening on ${info.port}`);
});
