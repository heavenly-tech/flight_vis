export {};

import { CHECKIN_STEP_SEC } from "./checkinStep";
import { applyLayoutToEl, type KioskQrLayout } from "./kioskQrLayout";

const statusEl = document.querySelector<HTMLParagraphElement>("#status-bar")!;
const qr = document.querySelector<HTMLImageElement>("#qr")!;
const qrWrap = document.querySelector<HTMLDivElement>("#qr-wrap")!;
const map = document.querySelector<HTMLUListElement>("#map")!;
const viewport = document.querySelector<HTMLDivElement>("#viewport")!;
const activatePanel = document.querySelector<HTMLDivElement>("#activate")!;
const activateToken = document.querySelector<HTMLInputElement>("#activate-token")!;
const activateBtn = document.querySelector<HTMLButtonElement>("#btn-activate")!;

let layout: KioskQrLayout = { x: 0.76, y: 0.76, size: 0.22 };

async function displaySessionActive(): Promise<boolean> {
  const res = await fetch("/api/display/map", { credentials: "same-origin" });
  return res.ok;
}

async function activateDisplay(): Promise<boolean> {
  const token = activateToken.value.trim();
  if (!token) return false;
  const res = await fetch("/api/display/activate", {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ token }),
  });
  if (!res.ok) {
    statusEl.textContent = "Invalid display token.";
    return false;
  }
  activatePanel.hidden = true;
  statusEl.textContent = "Display activated.";
  return true;
}

function refreshQrImage(): void {
  qr.src = `/api/club/screen/qr.svg?t=${Date.now()}`;
}

async function refreshMap(): Promise<void> {
  const res = await fetch("/api/display/map", { credentials: "same-origin" });
  if (!res.ok) return;
  const data = (await res.json()) as {
    aircraft: { tail: string; pilotName?: string }[];
  };
  map.replaceChildren(
    ...(data.aircraft ?? []).map((row) => {
      const li = document.createElement("li");
      li.textContent = `${row.tail} · ${row.pilotName ?? "—"}`;
      return li;
    }),
  );
}

function paintLayout(): void {
  qrWrap.hidden = false;
  applyLayoutToEl(qrWrap, layout, viewport);
}

async function fetchLayout(): Promise<void> {
  const res = await fetch("/api/display/kiosk-layout", { credentials: "same-origin" });
  if (!res.ok) return;
  layout = (await res.json()) as KioskQrLayout;
  paintLayout();
}

function watchLayoutStream(): void {
  const source = new EventSource("/api/display/kiosk-layout/stream", {
    withCredentials: true,
  });
  source.addEventListener("layout", (ev) => {
    try {
      layout = JSON.parse((ev as MessageEvent).data) as KioskQrLayout;
      paintLayout();
    } catch {
      /* ignore */
    }
  });
  source.onerror = () => {
    source.close();
    window.setTimeout(watchLayoutStream, 3000);
  };
}

window.addEventListener("resize", () => paintLayout());

activateBtn.addEventListener("click", () => {
  void (async () => {
    if (await activateDisplay()) {
      refreshQrImage();
      await fetchLayout();
      await refreshMap();
      watchLayoutStream();
    }
  })();
});

void (async () => {
  if (!(await displaySessionActive())) {
    activatePanel.hidden = false;
    statusEl.textContent = "Paste the display activation token from the admin map page.";
    return;
  }
  activatePanel.hidden = true;
  refreshQrImage();
  await fetchLayout();
  statusEl.textContent = `QR rotates every ${CHECKIN_STEP_SEC}s.`;
  await refreshMap();
  watchLayoutStream();
  setInterval(() => refreshQrImage(), CHECKIN_STEP_SEC * 1000);
  setInterval(() => void refreshMap(), 5000);
})();