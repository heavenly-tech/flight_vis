#!/usr/bin/env node
/**
 * Serves the production build with prod HTML CSP headers, proxies API routes to the Rust API,
 * and injects e2e-only scripts that are not shipped in dist/.
 */
import http from "node:http";
import { createReadStream, existsSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { cspForPath, policyKindForPath } from "./read-html-csp.mjs";

const root = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.resolve(root, "..", "dist");
const e2eGatewayDir = path.join(root, "e2e-gateway");
const apiOrigin = process.env.FLIGHT_VIS_API_ORIGIN ?? "http://127.0.0.1:8080";
const port = Number(process.env.E2E_CSP_PORT ?? "19090");

const E2E_ONLY = new Set(["e2e-csp-hooks.js", "e2e-live-csp-sse.js"]);

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
      res.writeHead(proxyRes.statusCode ?? 502, proxyRes.headers);
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
  const pathOnly = urlPath.split("?")[0];
  return (
    pathOnly === "/signup" ||
    pathOnly === "/signup.html" ||
    pathOnly === "/club-login.html" ||
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

    const baseName = urlPath.split("?")[0].replace(/^\//, "");
    if (E2E_ONLY.has(baseName)) {
      const e2ePath = path.join(e2eGatewayDir, baseName);
      if (existsSync(e2ePath)) {
        res.writeHead(200, {
          "content-type": "text/javascript; charset=utf-8",
          "cache-control": "no-cache",
        });
        createReadStream(e2ePath).pipe(res);
        return;
      }
    }

    let filePath = safeJoin(urlPath);
    const pathOnly = urlPath.split("?")[0];
    const spaFallback =
      !filePath || !existsSync(filePath) || !statSync(filePath).isFile();
    if (spaFallback) {
      filePath = path.join(distDir, "index.html");
    }
    const ext = path.extname(filePath).slice(1);
    const headers = {
      "content-type": mime(ext),
      "cache-control": "no-cache",
    };
    if (ext === "html") {
      let policyPath = pathOnly;
      if (spaFallback && policyKindForPath(pathOnly) === undefined) {
        policyPath = "/__spa_index__";
      }
      headers["content-security-policy"] = cspForPath(policyPath);
    }
    res.writeHead(200, headers);
    if (ext === "html") {
      let html = readFileSync(filePath, "utf8");
      let hook = '<script src="/e2e-csp-hooks.js"></script>';
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