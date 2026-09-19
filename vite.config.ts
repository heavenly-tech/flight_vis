import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type ProxyOptions } from "vite";
import { viteStaticCopy } from "vite-plugin-static-copy";

const root = path.dirname(fileURLToPath(import.meta.url));
const cesiumSource = "node_modules/cesium/Build/Cesium";
const cesiumBaseUrl = "cesiumStatic";
const api = "http://127.0.0.1:8080";

const authProxy: Record<string, string | ProxyOptions> = {
  "/api": api,
  "/logout": api,
  "/login": {
    target: api,
    bypass(req) {
      if (req.method === "GET" || req.method === "HEAD") {
        const query = req.url?.includes("?") ? req.url.slice(req.url.indexOf("?")) : "";
        return `/login.html${query}`;
      }
    },
  },
};

export default defineConfig({
  base: "/",
  define: {
    CESIUM_BASE_URL: JSON.stringify(`/${cesiumBaseUrl}/`),
  },
  plugins: [
    viteStaticCopy({
      targets: [
        { src: `${cesiumSource}/ThirdParty`, dest: cesiumBaseUrl },
        { src: `${cesiumSource}/Workers`, dest: cesiumBaseUrl },
        { src: `${cesiumSource}/Assets`, dest: cesiumBaseUrl },
        { src: `${cesiumSource}/Widgets`, dest: cesiumBaseUrl },
      ],
    }),
  ],
  server: {
    host: true,
    port: 5173,
    proxy: authProxy,
  },
  preview: {
    host: true,
    port: 4173,
    proxy: authProxy,
  },
  build: {
    chunkSizeWarningLimit: 4000,
    rollupOptions: {
      input: {
        main: path.resolve(root, "index.html"),
        login: path.resolve(root, "login.html"),
      },
    },
  },
});
