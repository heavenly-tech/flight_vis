import { CHECKIN_STEP_SEC } from "./checkinStep";
import { applyLayoutToEl, type KioskQrLayout } from "./kioskQrLayout";

export type ClubDisplayOpts = {
  statusEl: HTMLParagraphElement;
  qr: HTMLImageElement;
  qrWrap: HTMLDivElement;
  mapLayer: HTMLElement;
  map: HTMLUListElement;
  viewport: HTMLDivElement;
  activatePanel: HTMLDivElement;
  activateToken: HTMLInputElement;
  activateBtn: HTMLButtonElement;
  /** When set, a local hide/show toggle is shown (not persisted). */
  localToggleBtn?: HTMLButtonElement;
  qrEnabled?: () => boolean;
};

export function mountClubDisplayScreen(opts: ClubDisplayOpts): void {
  const {
    statusEl,
    qr,
    qrWrap,
    mapLayer,
    map,
    viewport,
    activatePanel,
    activateToken,
    activateBtn,
    localToggleBtn,
    qrEnabled,
  } = opts;

  let layout: KioskQrLayout = { x: 0.76, y: 0.76, size: 0.22 };
  let clubQrVisible = true;
  let localQrHidden = false;

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
    if (!showQr()) return;
    qr.src = `/api/club/screen/qr.svg?t=${Date.now()}`;
  }

  function showQr(): boolean {
    const enabled = qrEnabled?.() ?? true;
    return enabled && clubQrVisible && !localQrHidden;
  }

  function applyMapLayout(): void {
    mapLayer.classList.toggle("map-expanded", !showQr());
  }

  function paintQr(): void {
    const visible = showQr();
    qrWrap.hidden = !visible;
    applyMapLayout();
    if (visible) {
      applyLayoutToEl(qrWrap, layout, viewport);
    }
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

  async function fetchLayout(): Promise<void> {
    const res = await fetch("/api/display/kiosk-layout", { credentials: "same-origin" });
    if (!res.ok) return;
    layout = (await res.json()) as KioskQrLayout;
    if (typeof layout.checkinQrVisible === "boolean") {
      clubQrVisible = layout.checkinQrVisible;
    }
    paintQr();
    if (showQr() && !qr.src) refreshQrImage();
  }

  function watchLayoutStream(): void {
    const source = new EventSource("/api/display/kiosk-layout/stream", {
      withCredentials: true,
    });
    source.addEventListener("layout", (ev) => {
      try {
        layout = JSON.parse((ev as MessageEvent).data) as KioskQrLayout;
        if (typeof layout.checkinQrVisible === "boolean") {
          clubQrVisible = layout.checkinQrVisible;
        }
        paintQr();
        if (showQr()) refreshQrImage();
      } catch {
        /* ignore */
      }
    });
    source.onerror = () => {
      source.close();
      window.setTimeout(watchLayoutStream, 3000);
    };
  }

  window.addEventListener("resize", () => paintQr());

  activateBtn.addEventListener("click", () => {
    void (async () => {
      if (await activateDisplay()) {
        refreshQrImage();
        await fetchLayout();
        await refreshMap();
        watchLayoutStream();
        window.setInterval(() => void fetchLayout(), 8000);
      }
    })();
  });

  localToggleBtn?.addEventListener("click", () => {
    localQrHidden = !localQrHidden;
    localToggleBtn.textContent = localQrHidden
      ? localToggleBtn.dataset.labelShow ?? "Show QR"
      : localToggleBtn.dataset.labelHide ?? "Hide QR";
    paintQr();
    if (showQr()) refreshQrImage();
  });

  void (async () => {
    if (!(await displaySessionActive())) {
      activatePanel.hidden = false;
      statusEl.textContent = "Paste the display activation token from the admin map page.";
      return;
    }
    activatePanel.hidden = true;
    if (localToggleBtn) {
      localToggleBtn.hidden = false;
      localToggleBtn.textContent =
        localToggleBtn.dataset.labelHide ?? localToggleBtn.textContent ?? "Hide QR";
    }
    await fetchLayout();
    statusEl.textContent = `QR rotates every ${CHECKIN_STEP_SEC}s.`;
    await refreshMap();
    watchLayoutStream();
    window.setInterval(() => void fetchLayout(), 8000);
    window.setInterval(() => {
      if (showQr()) refreshQrImage();
    }, CHECKIN_STEP_SEC * 1000);
    window.setInterval(() => void refreshMap(), 5000);
  })();
}