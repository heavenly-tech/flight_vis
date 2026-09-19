import path from "node:path";
import { fileURLToPath } from "node:url";
import { serve } from "@hono/node-server";
import { createApp, seed } from "./app.mjs";

const root = path.dirname(fileURLToPath(import.meta.url));
const distDir = path.resolve(root, "../dist");
const dataDir = path.resolve(process.env.FLIGHT_DATA || path.resolve(root, "../data"));
const port = Number(process.env.PORT || 8080);

await seed(dataDir);

const app = createApp({ dataDir, distDir });

serve({ fetch: app.fetch, port }, (info) => {
  console.log(`flight_vis listening on ${info.port}`);
});
