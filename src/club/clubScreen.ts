export {};

import { CHECKIN_STEP_SEC } from "./checkinStep";

const statusEl = document.querySelector<HTMLParagraphElement>("#status")!;
const qr = document.querySelector<HTMLImageElement>("#qr")!;
const map = document.querySelector<HTMLUListElement>("#map")!;
const activatePanel = document.querySelector<HTMLDivElement>("#activate")!;
const activateToken = document.querySelector<HTMLInputElement>("#activate-token")!;
const activateBtn = document.querySelector<HTMLButtonElement>("#btn-activate")!;

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

activateBtn.addEventListener("click", () => {
  void (async () => {
    if (await activateDisplay()) {
      refreshQrImage();
      await refreshMap();
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
  statusEl.textContent = `QR rotates every ${CHECKIN_STEP_SEC}s.`;
  await refreshMap();
  setInterval(() => refreshQrImage(), CHECKIN_STEP_SEC * 1000);
  setInterval(() => void refreshMap(), 5000);
})();