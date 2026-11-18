#!/usr/bin/env node
/**
 * Serves the production build on PORT (default 9090) with the prod HTML CSP
 * header on every response, and proxies API/auth routes to the Rust API.
 */
import http from "node:http";
import { createReadStream, existsSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.resolve(root, "..", "dist");
const apiOrigin = process.env.FLIGHT_VIS_API_ORIGIN ?? "http://127.0.0.1:18080";
const port = Number(process.env.E2E_CSP_PORT ?? "19090");

const PROD_HTML_CSP =
  "default-src 'self'; " +
  "script-src 'self' 'wasm-unsafe-eval'; " +
  "worker-src 'self' blob:; " +
  "style-src 'self' 'unsafe-inline'; " +
  "img-src 'self' https://services.arcgisonline.com; " +
  "connect-src 'self' https://services.arcgisonline.com https://elevation3d.arcgis.com; " +
  "font-src 'self'; " +
  "frame-ancestors 'none'; " +
  "base-uri 'none'; " +
  "form-action 'self'";

function mime(ext) {
  switch (ext) {
    case "css":
      return "text/css; charset=utf-8";
    case "html":
      return "text/html; charset=utf-8";
    case "js":
      return "text/javascript; charset=utf-8";
    case "json":
      return "application/json";
    case "wasm":
      return "application/wasm";
    case "png":
      return "image/png";
    case "svg":
      return "image/svg+xml";
    case "woff":
      return "font/woff";
    case "woff2":
      return "font/woff2";
    default:
      return "application/octet-stream";
  }
}

function safeJoin(urlPath) {
  const trimmed = urlPath.split("?")[0].replace(/^\//, "");
  if (!trimmed) return path.join(distDir, "index.html");
  const joined = path.join(distDir, ...trimmed.split("/").filter(Boolean));
  if (!joined.startsWith(distDir)) return null;
  return joined;
}

function proxyToApi(req, res, body) {
  const target = new URL(req.url ?? "/", apiOrigin);
  const headers = { ...req.headers, host: target.host };
  const proxyReq = http.request(
    {
      hostname: target.hostname,
      port: target.port,
      path: target.pathname + target.search,
      method: req.method,
      headers,
    },
    (proxyRes) => {
      res.writeHead(proxyRes.statusCode ?? 502, {
        ...proxyRes.headers,
        "content-security-policy": PROD_HTML_CSP,
      });
      proxyRes.pipe(res);
    },
  );
  proxyReq.on("error", () => {
    res.writeHead(502, { "content-type": "text/plain" });
    res.end("bad gateway");
  });
  if (body?.length) proxyReq.write(body);
  proxyReq.end();
}

function shouldProxy(urlPath) {
  return (
    urlPath.startsWith("/api") ||
    urlPath.startsWith("/login") ||
    urlPath.startsWith("/logout") ||
    urlPath.startsWith("/.well-known") ||
    urlPath.startsWith("/c/") ||
    urlPath.startsWith("/s/")
  );
}

const server = http.createServer((req, res) => {
  const chunks = [];
  req.on("data", (c) => chunks.push(c));
  req.on("end", () => {
    const body = Buffer.concat(chunks);
    const urlPath = req.url ?? "/";
    if (shouldProxy(urlPath)) {
      proxyToApi(req, res, body);
      return;
    }
    let filePath = safeJoin(urlPath);
    if (!filePath || !existsSync(filePath) || !statSync(filePath).isFile()) {
      filePath = path.join(distDir, "index.html");
    }
    const ext = path.extname(filePath).slice(1);
    res.writeHead(200, {
      "content-type": mime(ext),
      "content-security-policy": PROD_HTML_CSP,
      "cache-control": "no-cache",
    });
    if (ext === "html") {
      let html = readFileSync(filePath, "utf8");
      let hook = '<script src="/e2e-csp-hooks.js"></script>';
      const pathOnly = urlPath.split("?")[0];
      if (/^\/live\/[^/]+/.test(pathOnly)) {
        hook += '<script src="/e2e-live-csp-sse.js"></script>';
      }
      if (!html.includes("e2e-csp-hooks.js")) {
        html = html.replace("<head>", `<head>${hook}`);
      }
      res.end(html);
      return;
    }
    createReadStream(filePath).pipe(res);
  });
});

server.listen(port, "127.0.0.1", () => {
  process.stdout.write(`e2e-csp-gateway listening on http://127.0.0.1:${port}\n`);
});