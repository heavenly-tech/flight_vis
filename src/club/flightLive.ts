import { createFlightViewer } from "../viz/viewer";
import { fixesToFlight } from "../live/fixesToFlight";

const params = new URLSearchParams(location.search);
const id = params.get("id");
const title = document.querySelector<HTMLHeadingElement>("#title")!;

async function run(): Promise<void> {
  if (!id) {
    title.textContent = "Missing flight id";
    return;
  }
  const viewer = await createFlightViewer(document.getElementById("cesiumContainer")!);
  const fixesRes = await fetch(`/api/flights/${id}/fixes`, { credentials: "same-origin" });
  if (fixesRes.ok) {
    const data = (await fixesRes.json()) as { fixes: import("../live/types").LiveFix[] };
    if (data.fixes?.length >= 2) {
      const flight = await fixesToFlight(data.fixes, { name: id });
      await viewer.setFlight(flight, "altitude", undefined, true);
    }
  }
  const stream = new EventSource(`/api/flights/${id}/stream`, { withCredentials: true });
  stream.addEventListener("message", (evt) => {
    void (async () => {
      try {
        const payload = JSON.parse(evt.data) as { fixes?: import("../live/types").LiveFix[] };
        const chunk = payload.fixes ?? [];
        if (chunk.length < 2) return;
        const flight = await fixesToFlight(chunk, { name: id });
        await viewer.setFlight(flight, "altitude", undefined, true);
      } catch {
        /* keepalive */
      }
    })();
  });
}

void run();