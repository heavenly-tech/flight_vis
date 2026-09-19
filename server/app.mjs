import { createReadStream, existsSync, statSync } from "node:fs";
import { mkdir, readFile, readdir, rename, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Hono } from "hono";
import {
  clearCookieHeader,
  defaultAuth,
  isSecureRequest,
  parseCookie,
  safeNext,
  setCookieHeader,
  wantsJson,
} from "./auth.mjs";

const root = path.dirname(fileURLToPath(import.meta.url));

export const RESERVED = new Set([
  "api",
  "assets",
  "cesiumstatic",
  "favicon.ico",
  "health",
  "login",
  "logout",
  "samples",
  "static",
]);

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

export function normalizeSlug(input) {
  return String(input || "")
    .trim()
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

export function isValidSlug(slug) {
  return /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(slug) && !RESERVED.has(slug);
}

export function slugFromFileName(fileName) {
  const base = String(fileName || "").replace(/\.[^.]+$/, "");
  const slug = normalizeSlug(base);
  return isValidSlug(slug) ? slug : "track";
}

export function nameFromFileName(fileName) {
  return String(fileName || "track").replace(/\.[^.]+$/, "") || "track";
}

export function allocateSlug(desired, taken) {
  const base = isValidSlug(desired) ? desired : "track";
  if (!taken.has(base)) return base;
  for (let n = 2; n < 10000; n++) {
    const suffix = `-${n}`;
    const candidate = normalizeSlug(`${base.slice(0, 64 - suffix.length)}${suffix}`);
    const slug = isValidSlug(candidate) ? candidate : `track${suffix}`;
    if (!taken.has(slug)) return slug;
  }
  const error = new Error("Could not allocate slug");
  error.status = 409;
  throw error;
}

export function createStore(dataDir) {
  const flightsDir = path.join(dataDir, "flights");

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
    const track =
      body.track ?? (existsSync(path.join(dir, "track")) ? await readFile(path.join(dir, "track"), "utf8") : null);
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

  function exists(slug) {
    const dir = flightDir(slug);
    return existsSync(path.join(dir, "meta.json")) && existsSync(path.join(dir, "track"));
  }

  function summarize(slug, meta) {
    const size = statSync(path.join(flightDir(slug), "track")).size;
    return {
      slug,
      name: meta.name || slug,
      fileName: meta.fileName || "track",
      updatedAt: meta.updatedAt || null,
      createdAt: meta.createdAt || null,
      size,
    };
  }

  async function listFlights() {
    await mkdir(flightsDir, { recursive: true });
    const slugs = (await readdir(flightsDir, { withFileTypes: true }))
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name)
      .filter(isValidSlug);
    const items = [];
    for (const slug of slugs) {
      const metaPath = path.join(flightDir(slug), "meta.json");
      const trackPath = path.join(flightDir(slug), "track");
      if (!existsSync(metaPath) || !existsSync(trackPath)) continue;
      const meta = JSON.parse(await readFile(metaPath, "utf8"));
      items.push(summarize(slug, meta));
    }
    items.sort((a, b) => String(b.updatedAt || "").localeCompare(String(a.updatedAt || "")));
    return items;
  }

  async function deleteFlight(slug) {
    if (!exists(slug)) {
      const error = new Error("Not found");
      error.status = 404;
      throw error;
    }
    await rm(flightDir(slug), { recursive: true, force: true });
  }

  async function renameFlight(fromSlug, body = {}) {
    if (!exists(fromSlug)) {
      const error = new Error("Not found");
      error.status = 404;
      throw error;
    }

    const nextName = body.name != null ? String(body.name).trim() : "";
    const requested = body.slug != null ? normalizeSlug(body.slug) : fromSlug;
    if (body.slug != null && !isValidSlug(requested)) {
      const error = new Error("Invalid slug");
      error.status = 400;
      throw error;
    }

    const nextSlug = requested;
    if (nextSlug !== fromSlug) {
      if (exists(nextSlug) && !body.overwrite) {
        const error = new Error("exists");
        error.status = 409;
        error.slug = nextSlug;
        throw error;
      }
      if (exists(nextSlug) && body.overwrite) {
        await rm(flightDir(nextSlug), { recursive: true, force: true });
      }
      await rename(flightDir(fromSlug), flightDir(nextSlug));
    }

    const metaPath = path.join(flightDir(nextSlug), "meta.json");
    const meta = JSON.parse(await readFile(metaPath, "utf8"));
    if (nextName) meta.name = nextName;
    meta.slug = nextSlug;
    meta.updatedAt = new Date().toISOString();
    await writeFile(metaPath, JSON.stringify(meta, null, 2));
    return summarize(nextSlug, meta);
  }

  async function uploadFlights(files, conflict = "suffix") {
    if (!Array.isArray(files) || files.length === 0) {
      const error = new Error("Files are required");
      error.status = 400;
      throw error;
    }
    if (files.length > 50) {
      const error = new Error("Too many files");
      error.status = 400;
      throw error;
    }

    const mode = conflict === "overwrite" || conflict === "error" ? conflict : "suffix";
    const taken = new Set((await listFlights()).map((item) => item.slug));
    const existing = [];
    const prepared = [];

    for (const file of files) {
      const fileName = String(file?.fileName || "track");
      const track = file?.track;
      if (!track || typeof track !== "string") {
        const error = new Error("Track file is required");
        error.status = 400;
        throw error;
      }
      const desired = file.slug != null ? normalizeSlug(file.slug) : slugFromFileName(fileName);
      const base = isValidSlug(desired) ? desired : slugFromFileName(fileName);
      if (taken.has(base) && mode === "error") {
        existing.push(base);
        continue;
      }
      const slug = taken.has(base) && mode !== "overwrite" ? allocateSlug(base, taken) : base;
      const name = String(file.name || "").trim() || nameFromFileName(fileName);
      prepared.push({ slug, name, fileName, track });
      taken.add(slug);
    }

    if (mode === "error" && existing.length) {
      const error = new Error("exists");
      error.status = 409;
      error.slugs = [...new Set(existing)];
      throw error;
    }

    const items = [];
    for (const item of prepared) {
      const saved = await writeFlight(item.slug, {
        name: item.name,
        fileName: item.fileName,
        track: item.track,
        tags: [],
        trimStartMs: null,
        trimEndMs: null,
      });
      items.push(summarize(item.slug, saved));
    }
    return { items };
  }

  return {
    flightsDir,
    flightDir,
    readMeta,
    writeFlight,
    listFlights,
    deleteFlight,
    renameFlight,
    uploadFlights,
    exists,
  };
}

export async function seed(dataDir, options = {}) {
  const store = createStore(dataDir);
  await mkdir(store.flightsDir, { recursive: true });
  if (existsSync(path.join(store.flightDir("vuelo-5"), "meta.json"))) return;
  const candidates = options.samples || [
    path.resolve(root, "../samples/vuelo5.gpx"),
    path.resolve(root, "../public/samples/vuelo5.gpx"),
    path.resolve(root, "../dist/samples/vuelo5.gpx"),
  ];
  const sample = candidates.find((file) => existsSync(file));
  if (!sample) return;
  await store.writeFlight("vuelo-5", {
    name: "Vuelo5",
    fileName: "vuelo5.gpx",
    trimStartMs: null,
    trimEndMs: null,
    tags: [],
    track: await readFile(sample, "utf8"),
  });
}

function loginPageFallback(configured) {
  const lock = configured
    ? ""
    : `<p class="lock">Upload is locked. Set <code>FLIGHT_VIS_PASSWORD</code> on the host.</p>`;
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>flight_vis · login</title>
    <style>
      :root { color-scheme: dark; --bg:#07090d; --panel:#10151d; --stroke:rgba(255,255,255,.12); --text:#f4f7fb; --muted:#9aa6b5; --cyan:#67e8f9; --bad:#f87171; }
      body { margin:0; min-height:100vh; display:flex; align-items:center; justify-content:center; background:var(--bg); color:var(--text); font:14px/1.5 Outfit,system-ui,sans-serif; }
      .sheet { width:min(400px,92vw); padding:28px 24px; border:1px solid var(--stroke); border-radius:16px; background:var(--panel); }
      h1 { margin:0; letter-spacing:.18em; text-transform:uppercase; font-size:11px; color:var(--cyan); }
      p { color:var(--muted); }
      label { display:block; font-size:12px; color:var(--muted); margin-bottom:6px; }
      input { width:100%; box-sizing:border-box; background:#0b0f16; color:var(--text); border:1px solid var(--stroke); border-radius:10px; padding:10px 12px; }
      button { margin-top:14px; width:100%; cursor:pointer; background:#ecebff; color:#15132a; border:0; border-radius:10px; padding:10px 12px; font-weight:650; }
      .err { color:var(--bad); min-height:1.2em; }
    </style>
  </head>
  <body>
    <form class="sheet" method="post" action="/login">
      <h1>flight_vis</h1>
      <p>heavenly.cl · password to save and upload tracks</p>
      ${lock || `<label for="password">Password</label>
      <input id="password" name="password" type="password" required autofocus />
      <input type="hidden" name="next" value="/" />
      <button type="submit">Log in</button>
      <p class="err"></p>`}
    </form>
  </body>
</html>`;
}

async function readLoginPage(distDir, configured) {
  const candidates = [
    path.join(distDir, "login.html"),
    path.resolve(root, "../dist/login.html"),
    path.resolve(root, "../login.html"),
  ];
  const file = candidates.find((candidate) => existsSync(candidate));
  if (!file) return loginPageFallback(configured);
  return readFile(file, "utf8");
}

async function readLoginFields(c) {
  const type = c.req.header("content-type") || "";
  if (type.includes("application/json")) {
    const body = await c.req.json().catch(() => ({}));
    return {
      password: String(body.password || ""),
      next: safeNext(body.next),
    };
  }
  const body = await c.req.parseBody();
  return {
    password: String(body.password || ""),
    next: safeNext(body.next),
  };
}

export function createApp(options = {}) {
  const dataDir = options.dataDir || path.resolve(root, "../data");
  const distDir = options.distDir || path.resolve(root, "../dist");
  const auth = options.auth || defaultAuth;
  const store = createStore(dataDir);
  const app = new Hono();

  function sessionOk(c) {
    return auth.verify(parseCookie(c.req.header("cookie")));
  }

  async function requireAuth(c, next) {
    if (sessionOk(c)) return next();
    return c.json({ error: auth.configured() ? "unauthorized" : "locked" }, 401);
  }

  function attachSession(c, token) {
    c.header("Set-Cookie", setCookieHeader(token, isSecureRequest(c)));
  }

  function clearSession(c) {
    c.header("Set-Cookie", clearCookieHeader(isSecureRequest(c)));
  }

  app.get("/api/health", (c) => c.json({ ok: true }));

  app.get("/api/auth/me", (c) =>
    c.json({
      authenticated: sessionOk(c),
      configured: auth.configured(),
    }),
  );

  app.get("/login", async (c) => {
    if (sessionOk(c)) return c.redirect(safeNext(c.req.query("next")), 302);
    const html = await readLoginPage(distDir, auth.configured());
    return c.html(html);
  });

  app.get("/login.html", (c) => {
    const next = c.req.query("next");
    return c.redirect(next ? `/login?next=${encodeURIComponent(next)}` : "/login", 302);
  });

  app.post("/login", async (c) => {
    const json = wantsJson(c);
    if (!auth.configured()) {
      if (json) return c.json({ error: "locked" }, 401);
      return c.redirect("/login?locked=1", 302);
    }
    const fields = await readLoginFields(c);
    if (!auth.checkPassword(fields.password)) {
      if (json) return c.json({ error: "unauthorized" }, 401);
      return c.redirect("/login?e=1", 302);
    }
    attachSession(c, auth.mint());
    if (json) return c.json({ ok: true, next: fields.next });
    return c.redirect(fields.next, 302);
  });

  const logout = (c) => {
    clearSession(c);
    if (wantsJson(c)) return c.json({ ok: true });
    return c.redirect("/login", 302);
  };
  app.get("/logout", logout);
  app.post("/logout", logout);

  app.get("/api/flights", requireAuth, async (c) => c.json(await store.listFlights()));

  app.get("/api/flights/:slug", async (c) => {
    const slug = normalizeSlug(c.req.param("slug"));
    if (!isValidSlug(slug)) return c.json({ error: "Invalid slug" }, 400);
    const record = await store.readMeta(slug);
    if (!record) return c.json({ error: "Not found" }, 404);
    return c.json(record);
  });

  app.put("/api/flights/:slug", requireAuth, async (c) => {
    const slug = normalizeSlug(c.req.param("slug"));
    if (!isValidSlug(slug)) return c.json({ error: "Invalid slug" }, 400);
    const body = await c.req.json();
    try {
      return c.json(await store.writeFlight(slug, body));
    } catch (error) {
      return c.json({ error: error.message }, error.status || 500);
    }
  });

  app.post("/api/flights", requireAuth, async (c) => {
    const body = await c.req.json().catch(() => ({}));
    try {
      return c.json(await store.uploadFlights(body.files, body.conflict));
    } catch (error) {
      const payload = { error: error.message };
      if (error.slugs) payload.slugs = error.slugs;
      return c.json(payload, error.status || 500);
    }
  });

  app.patch("/api/flights/:slug", requireAuth, async (c) => {
    const slug = normalizeSlug(c.req.param("slug"));
    if (!isValidSlug(slug)) return c.json({ error: "Invalid slug" }, 400);
    const body = await c.req.json().catch(() => ({}));
    try {
      return c.json(await store.renameFlight(slug, body));
    } catch (error) {
      const payload = { error: error.message };
      if (error.slug) payload.slug = error.slug;
      return c.json(payload, error.status || 500);
    }
  });

  app.delete("/api/flights/:slug", requireAuth, async (c) => {
    const slug = normalizeSlug(c.req.param("slug"));
    if (!isValidSlug(slug)) return c.json({ error: "Invalid slug" }, 400);
    try {
      await store.deleteFlight(slug);
      return c.json({ ok: true });
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

  return app;
}
