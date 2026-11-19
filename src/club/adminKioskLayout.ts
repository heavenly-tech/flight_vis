import { ensureClubAdminOrRedirect } from "./adminGate";
import { api, isClubAdminRedirect, mountAdminNav, requireAdmin } from "./adminCommon";
import { startClubPage } from "./i18nPage";
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

let layout: KioskQrLayout = defaultKioskQrLayout(DEFAULT_PREVIEW_ASPECT);

function aspect(): number {
  return Number(aspectSel.value);
}

function resizeFrame(): void {
  const a = aspect();
  frame.style.aspectRatio = `${a}`;
}

function paint(): void {
  resizeFrame();
  applyLayoutToEl(qr, layout, frame);
}

function pointerLayoutFromRect(left: number, top: number, side: number): KioskQrLayout {
  const rect = frame.getBoundingClientRect();
  const short = Math.min(rect.width, rect.height);
  const size = side / short;
  const x = (left - rect.left) / rect.width;
  const y = (top - rect.top) / rect.height;
  return clampKioskQrLayout(x, y, size, aspect());
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
    (ev.target as HTMLElement).setPointerCapture(ev.pointerId);
    ev.preventDefault();
  };

  const onPointerMove = (ev: PointerEvent) => {
    if (!mode) return;
    const dx = ev.clientX - startX;
    const dy = ev.clientY - startY;
  const rect = frame.getBoundingClientRect();
    const short = Math.min(rect.width, rect.height);
    if (mode === "move") {
      const left = startLayout.x * rect.width + dx;
      const top = startLayout.y * rect.height + dy;
      const side = startLayout.size * short;
      layout = pointerLayoutFromRect(left, top, side);
    } else {
      const side = Math.max(short * 0.18, startSide + Math.max(dx, dy));
      const left = startLayout.x * rect.width;
      const top = startLayout.y * rect.height;
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

async function loadLayout(): Promise<void> {
  layout = await api<KioskQrLayout>("/api/admin/kiosk-layout");
  paint();
}

saveBtn.addEventListener("click", async () => {
  saveStatus.textContent = "Guardando…";
  layout = await api<KioskQrLayout>("/api/admin/kiosk-layout", {
    method: "PATCH" as const,
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ ...layout, aspect: aspect() }),
  });
  saveStatus.textContent = `Guardado (rev ${layout.rev ?? "?"})`;
});

resetBtn.addEventListener("click", async () => {
  layout = await api<KioskQrLayout>("/api/admin/kiosk-layout/reset", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ aspect: aspect() }),
  });
  paint();
  saveStatus.textContent = "Restablecido";
});

aspectSel.addEventListener("change", () => {
  layout = clampKioskQrLayout(layout.x, layout.y, layout.size, aspect());
  paint();
});

paint();

void (async () => {
  void startClubPage({ switcher: "#lang-slot" });
  if (!(await ensureClubAdminOrRedirect())) return;
  mountAdminNav();
  try {
    await requireAdmin();
    bindDrag();
    window.addEventListener("resize", () => paint());
    await loadLayout();
  } catch (e) {
    if (!isClubAdminRedirect(e)) saveStatus.textContent = "Se requiere admin.";
  }
})();