import { createFlightViewer } from "../viz/viewer";
import { fixesToFlight } from "../live/fixesToFlight";

function tokenFromPath(): string | null {
  const parts = location.pathname.split("/").filter(Boolean);
  if (parts[0] === "s" && parts[1]) {
    return parts[1];
  }
  return null;
}

const token = tokenFromPath();
const title = document.querySelector<HTMLHeadingElement>("#title")!;
const statusEl = document.querySelector<HTMLParagraphElement>("#status")!;

async function run(): Promise<void> {
  if (!token) {
    title.textContent = "Enlace no válido";
    return;
  }
  const metaRes = await fetch(`/api/share/${encodeURIComponent(token)}`, {
    credentials: "same-origin",
  });
  if (!metaRes.ok) {
    title.textContent = "Vuelo no disponible";
    return;
  }
  const meta = (await metaRes.json()) as { tail: string; status: string };
  title.textContent = meta.tail;
  statusEl.textContent = meta.status === "active" ? "En vivo" : "Grabación";
  const viewer = await createFlightViewer(document.getElementById("cesiumContainer")!);
  const fixesRes = await fetch(`/api/share/${encodeURIComponent(token)}/fixes`);
  if (fixesRes.ok) {
    const data = (await fixesRes.json()) as { fixes: import("../live/types").LiveFix[]; tail: string };
    if (data.fixes?.length >= 2) {
      const flight = await fixesToFlight(data.fixes, { name: data.tail });
      await viewer.setFlight(flight, "altitude", undefined, true);
    }
  }
  const stream = new EventSource(`/api/share/${encodeURIComponent(token)}/stream`);
  stream.addEventListener("message", (evt) => {
    void (async () => {
      try {
        const payload = JSON.parse(evt.data) as { fixes?: import("../live/types").LiveFix[] };
        const chunk = payload.fixes ?? (Array.isArray(payload) ? payload : []);
        if (chunk.length < 2) return;
        const flight = await fixesToFlight(chunk, { name: meta.tail });
        await viewer.setFlight(flight, "altitude", undefined, true);
      } catch {
        /* keepalive */
      }
    })();
  });
}

void run();