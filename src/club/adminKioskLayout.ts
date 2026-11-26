import { ensureClubAdminOrRedirect } from "./adminGate";
import { api, isClubAdminRedirect, mountAdminNav, requireAdmin } from "./adminCommon";
import { startClubPage } from "./i18nPage";
import { mountOpenKioskLink } from "./openKioskLink";
import { t } from "../i18n/index";
import {
  applyLayoutToEl,
  clampKioskQrLayout,
  defaultKioskQrLayout,
  DEFAULT_PREVIEW_ASPECT,
  type KioskQrLayout,
} from "./kioskQrLayout";

const frame = document.querySelector<HTMLDivElement>("#preview-frame")!;
const qr = document.querySelector<HTMLDivElement>("#preview-qr")!;
const handle = document.querySelector<HTMLSpanElement>("#resize-handle")!;
const aspectSel = document.querySelector<HTMLSelectElement>("#aspect")!;
const saveBtn = document.querySelector<HTMLButtonElement>("#btn-save")!;
const resetBtn = document.querySelector<HTMLButtonElement>("#btn-reset")!;
const saveStatus = document.querySelector<HTMLSpanElement>("#save-status")!;
const qrVisible = document.querySelector<HTMLInputElement>("#checkin-qr-visible");
const qrXInput = document.querySelector<HTMLInputElement>("#qr-x");
const qrYInput = document.querySelector<HTMLInputElement>("#qr-y");
const qrSizeInput = document.querySelector<HTMLInputElement>("#qr-size");
const openKioskSlot = document.querySelector<HTMLDivElement>("#open-kiosk-slot");

let layout: KioskQrLayout = defaultKioskQrLayout(DEFAULT_PREVIEW_ASPECT);
let savedVisible = true;

function aspect(): number {
  return Number(aspectSel.value);
}

function resizeFrame(): void {
  const a = aspect();
  frame.style.aspectRatio = `${a}`;
}

function syncCoordInputs(): void {
  if (qrXInput && document.activeElement !== qrXInput) {
    qrXInput.value = String(Math.round(layout.x * 100));
  }
  if (qrYInput && document.activeElement !== qrYInput) {
    qrYInput.value = String(Math.round(layout.y * 100));
  }
  if (qrSizeInput && document.activeElement !== qrSizeInput) {
    qrSizeInput.value = String(Math.round(layout.size * 100));
  }
}

function paint(): void {
  resizeFrame();
  applyLayoutToEl(qr, layout, frame);
  const show = layout.checkinQrVisible !== false;
  qr.hidden = !show;
  if (qrVisible) qrVisible.checked = show;
  syncCoordInputs();
}

function pointerLayoutFromRect(left: number, top: number, side: number): KioskQrLayout {
  const rect = frame.getBoundingClientRect();
  const short = Math.min(rect.width, rect.height);
  const size = side / short;
  const x = (left - rect.left) / rect.width;
  const y = (top - rect.top) / rect.height;
  return {
    ...clampKioskQrLayout(x, y, size, aspect()),
    checkinQrVisible: layout.checkinQrVisible,
  };
}

function bindDrag(): void {
  let mode: "move" | "resize" | null = null;
  let startX = 0;
  let startY = 0;
  let startLayout: KioskQrLayout = layout;
  let startSide = 0;

  const onPointerDown = (ev: PointerEvent) => {
    if (ev.target === handle) {
      mode = "resize";
      startSide = qr.offsetWidth;
    } else {
      mode = "move";
    }
    startLayout = { ...layout };
    startX = ev.clientX;
    startY = ev.clientY;
    const captureTarget = ev.target === handle ? handle : qr;
    captureTarget.setPointerCapture(ev.pointerId);
    ev.stopPropagation();
    ev.preventDefault();
  };

  const onPointerMove = (ev: PointerEvent) => {
    if (!mode) return;
    const dx = ev.clientX - startX;
    const dy = ev.clientY - startY;
    const rect = frame.getBoundingClientRect();
    const short = Math.min(rect.width, rect.height);
    if (mode === "move") {
      const left = rect.left + startLayout.x * rect.width + dx;
      const top = rect.top + startLayout.y * rect.height + dy;
      const side = startLayout.size * short;
      layout = pointerLayoutFromRect(left, top, side);
    } else {
      const side = Math.max(short * 0.18, startSide + Math.max(dx, dy));
      const left = rect.left + startLayout.x * rect.width;
      const top = rect.top + startLayout.y * rect.height;
      layout = pointerLayoutFromRect(left, top, side);
    }
    paint();
  };

  const onPointerUp = () => {
    mode = null;
  };

  qr.addEventListener("pointerdown", onPointerDown);
  handle.addEventListener("pointerdown", onPointerDown);
  window.addEventListener("pointermove", onPointerMove);
  window.addEventListener("pointerup", onPointerUp);
}

function applyCoordInputs(): void {
  const xPct = Number(qrXInput?.value);
  const yPct = Number(qrYInput?.value);
  const sizePct = Number(qrSizeInput?.value);
  const x = Number.isFinite(xPct) ? xPct / 100 : layout.x;
  const y = Number.isFinite(yPct) ? yPct / 100 : layout.y;
  const size = Number.isFinite(sizePct) ? sizePct / 100 : layout.size;
  layout = {
    ...clampKioskQrLayout(x, y, size, aspect()),
    checkinQrVisible: layout.checkinQrVisible,
  };
  paint();
}

for (const input of [qrXInput, qrYInput, qrSizeInput]) {
  input?.addEventListener("change", applyCoordInputs);
}

qrVisible?.addEventListener("change", () => {
  layout = { ...layout, checkinQrVisible: qrVisible.checked };
  paint();
});

async function loadLayout(): Promise<void> {
  layout = await api<KioskQrLayout>("/api/admin/kiosk-layout");
  savedVisible = layout.checkinQrVisible !== false;
  paint();
}

saveBtn.addEventListener("click", async () => {
  saveStatus.setAttribute("role", "status");
  saveStatus.textContent = t("admin.saving");
  saveBtn.disabled = true;
  try {
    const desiredVisible = qrVisible ? qrVisible.checked : layout.checkinQrVisible !== false;
    let updated = await api<KioskQrLayout>("/api/admin/kiosk-layout", {
      method: "PATCH" as const,
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ x: layout.x, y: layout.y, size: layout.size, aspect: aspect() }),
    });
    if (desiredVisible !== savedVisible) {
      updated = await api<KioskQrLayout>("/api/admin/club/kiosk-checkin-qr-visible", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ visible: desiredVisible }),
      });
    }
    layout = updated;
    savedVisible = layout.checkinQrVisible !== false;
    paint();
    saveStatus.textContent = t("admin.saved");
  } catch (e) {
    console.error("[adminKioskLayout] save failed:", e);
    saveStatus.setAttribute("role", "alert");
    saveStatus.textContent = t("api.genericError");
  } finally {
    saveBtn.disabled = false;
  }
});

resetBtn.addEventListener("click", async () => {
  saveStatus.setAttribute("role", "status");
  saveStatus.textContent = t("admin.saving");
  resetBtn.disabled = true;
  try {
    layout = await api<KioskQrLayout>("/api/admin/kiosk-layout/reset", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ aspect: aspect() }),
    });
    savedVisible = layout.checkinQrVisible !== false;
    paint();
    saveStatus.textContent = t("admin.resetDone");
  } catch (e) {
    console.error("[adminKioskLayout] reset failed:", e);
    saveStatus.setAttribute("role", "alert");
    saveStatus.textContent = t("api.genericError");
  } finally {
    resetBtn.disabled = false;
  }
});

aspectSel.addEventListener("change", () => {
  layout = {
    ...clampKioskQrLayout(layout.x, layout.y, layout.size, aspect()),
    checkinQrVisible: layout.checkinQrVisible,
  };
  paint();
});

paint();

void (async () => {
  await startClubPage({ switcher: "#lang-slot" });
  if (!(await ensureClubAdminOrRedirect())) return;
  mountAdminNav();
  try {
    await requireAdmin();
    bindDrag();
    window.addEventListener("resize", () => paint());
    await Promise.all([loadLayout(), mountOpenKioskLink(openKioskSlot)]);
  } catch (e) {
    console.error("[adminKioskLayout] boot failed:", e);
    if (!isClubAdminRedirect(e)) {
      saveStatus.setAttribute("role", "alert");
      saveStatus.textContent = t("admin.adminRequired");
    }
  }
})();
