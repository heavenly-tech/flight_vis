import { createFlightViewer } from "../viz/viewer";
import { fixesToFlight } from "../live/fixesToFlight";
import { createShare, listShares, revokeShare } from "./flightSharePanel";
import { startClubPage } from "./i18nPage";
import { formatDate, t } from "../i18n/index";

const params = new URLSearchParams(location.search);
const id = params.get("id");
const title = document.querySelector<HTMLHeadingElement>("#title")!;
const sharePanel = document.querySelector<HTMLElement>("#share-panel")!;
const shareCreate = document.querySelector<HTMLButtonElement>("#btn-share-create")!;
const shareCopy = document.querySelector<HTMLButtonElement>("#btn-share-copy")!;
const shareList = document.querySelector<HTMLUListElement>("#share-list")!;
const shareQr = document.querySelector<HTMLDivElement>("#share-qr")!;

let lastShareUrl = "";

async function refreshShares(flightId: string): Promise<void> {
  const shares = await listShares(flightId);
  shareList.replaceChildren(
    ...shares
      .filter((s) => !s.revoked)
      .map((s) => {
        const li = document.createElement("li");
        const btn = document.createElement("button");
        btn.type = "button";
        btn.textContent = t("flight.shareRevoke");
        btn.addEventListener("click", async () => {
          await revokeShare(flightId, s.id);
          await refreshShares(flightId);
        });
        li.append(
          document.createTextNode(`${t("flight.shareLinkAt", { at: formatDate(s.createdAt, true) })} `),
          btn,
        );
        return li;
      }),
  );
}

shareCreate.addEventListener("click", async () => {
  if (!id) return;
  const created = await createShare(id);
  lastShareUrl = created.url;
  shareCopy.hidden = false;
  shareQr.hidden = false;
  shareQr.innerHTML = created.qrSvg;
  await refreshShares(id);
});

shareCopy.addEventListener("click", async () => {
  if (!lastShareUrl) return;
  await navigator.clipboard.writeText(lastShareUrl);
  shareCopy.textContent = t("common.copied");
});

async function run(): Promise<void> {
  await startClubPage({ persistUser: true });
  if (!id) {
    title.textContent = t("flight.missingId");
    return;
  }
  title.textContent = t("flight.liveTitle");
  sharePanel.hidden = false;
  shareCreate.textContent = t("flight.shareCreate");
  shareCopy.textContent = t("flight.shareCopy");
  await refreshShares(id);
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