import { createFlightViewer } from "../viz/viewer";
import { fixesToFlight } from "../live/fixesToFlight";
import { startClubPage } from "./i18nPage";
import { t } from "../i18n/index";

const params = new URLSearchParams(location.search);
const token = params.get("token") ?? location.pathname.split("/").pop();
const title = document.querySelector<HTMLHeadingElement>("#title")!;
const statusEl = document.querySelector<HTMLParagraphElement>("#status")!;

async function run(): Promise<void> {
  await startClubPage({ persistUser: false });
  title.textContent = t("sharePage.title");
  if (!token) {
    title.textContent = t("sharePage.invalidLink");
    return;
  }
  const metaRes = await fetch(`/api/share/${encodeURIComponent(token)}/meta`);
  if (!metaRes.ok) {
    title.textContent = t("sharePage.unavailable");
    return;
  }
  const meta = (await metaRes.json()) as { flightId: string; status: string };
  statusEl.textContent =
    meta.status === "active" ? t("flight.statusLive") : t("flight.statusReplay");
  const viewer = await createFlightViewer(document.getElementById("cesiumContainer")!);
  const fixesRes = await fetch(`/api/share/${encodeURIComponent(token)}/fixes`);
  if (fixesRes.ok) {
    const data = (await fixesRes.json()) as { fixes: import("../live/types").LiveFix[] };
    if (data.fixes?.length >= 2) {
      const flight = await fixesToFlight(data.fixes, { name: meta.flightId });
      await viewer.setFlight(flight, "altitude", undefined, true);
    }
  }
}

void run();