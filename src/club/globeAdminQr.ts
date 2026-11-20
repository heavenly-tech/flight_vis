import { t } from "../i18n/index";

// Reuse constants from existing check-in QR flow
export const CHECKIN_STEP_SEC = 30;
export const ADMIN_PHONE_QR_TIMEOUT_MS = 60_000;
const CONSUME_POLL_GRACE_MS = 8_000;

// Minimum and maximum sizes for the overlay
export const MIN_OVERLAY_SIZE = 160;
export const MAX_OVERLAY_SIZE = 1000;

// Storage key for the overlay position/size
const STORAGE_KEY = "fv.globeAdminQr.rect";

// Type for overlay rectangle
export interface OverlayRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

// Clamp overlay rectangle to viewport with minimum size
// Exported for unit testing
export function clampOverlayRect(
  rect: OverlayRect,
  viewport: { width: number; height: number },
  minSize: number = MIN_OVERLAY_SIZE,
): OverlayRect {
  const { width: vw, height: vh } = viewport;
  
  // Clamp width and height to min/max/viewport
  let w = Math.max(minSize, Math.min(rect.width, vw, MAX_OVERLAY_SIZE));
  let h = Math.max(minSize, Math.min(rect.height, vh, MAX_OVERLAY_SIZE));
  
  // Clamp x position so the overlay stays within viewport
  let x = Math.max(0, Math.min(rect.x, vw - w));
  
  // Clamp y position so the overlay stays within viewport
  let y = Math.max(0, Math.min(rect.y, vh - h));
  
  return { x, y, width: w, height: h };
}

// Default position for the overlay (top-right area)
function defaultOverlayRect(): OverlayRect {
  return {
    x: 100,
    y: 100,
    width: 300,
    height: 300,
  };
}

// Load saved rectangle from localStorage
function loadOverlayRect(): OverlayRect {
  const saved = localStorage.getItem(STORAGE_KEY);
  if (saved) {
    try {
      const rect = JSON.parse(saved) as OverlayRect;
      // Validate and clamp to reasonable defaults
      if (
        rect &&
        typeof rect.x === "number" &&
        typeof rect.y === "number" &&
        typeof rect.width === "number" &&
        typeof rect.height === "number"
      ) {
        return rect;
      }
    } catch {
      // Invalid JSON, use default
    }
  }
  return defaultOverlayRect();
}

// Save rectangle to localStorage
function saveOverlayRect(rect: OverlayRect): void {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(rect));
}

// State management
type OverlayState = "closed" | "open" | "minimized";

let state: OverlayState = "closed";
let shownAtMs = 0;
let stepTimer: number | undefined;
let hideTimer: number | undefined;
let consumeGraceTimer: number | undefined;
let consumePollTimer: number | undefined;

// DOM elements (lazy initialized)
let btnAdminQr: HTMLButtonElement | null = null;
let overlay: HTMLElement | null = null;
let qrImg: HTMLImageElement | null = null;
let statusEl: HTMLElement | null = null;
let minimizeBtn: HTMLButtonElement | null = null;
let resizeHandle: HTMLElement | null = null;
let pill: HTMLElement | null = null;

// Current position and size
let currentRect: OverlayRect = defaultOverlayRect();

// Track if we're dragging or resizing
let isDragging = false;
let isResizing = false;
let dragStart: { x: number; y: number; rect: OverlayRect; pointerId: number } | null = null;
let resizeStart: { x: number; y: number; rect: OverlayRect; pointerId: number } | null = null;

// Initialize the overlay
export function initGlobeAdminQr(
  options: {
    btnAdminQrSelector: string;
    overlaySelector: string;
    qrImgSelector: string;
    statusSelector: string;
    minimizeBtnSelector: string;
    resizeHandleSelector: string;
    pillSelector: string;
    cesiumContainerSelector: string;
  },
): void {
  // Get DOM elements
  btnAdminQr = document.querySelector<HTMLButtonElement>(options.btnAdminQrSelector);
  overlay = document.querySelector<HTMLElement>(options.overlaySelector);
  qrImg = document.querySelector<HTMLImageElement>(options.qrImgSelector);
  statusEl = document.querySelector<HTMLElement>(options.statusSelector);
  minimizeBtn = document.querySelector<HTMLButtonElement>(options.minimizeBtnSelector);
  resizeHandle = document.querySelector<HTMLElement>(options.resizeHandleSelector);
  pill = document.querySelector<HTMLElement>(options.pillSelector);
  
  if (!btnAdminQr || !overlay || !qrImg || !statusEl || !minimizeBtn || !resizeHandle || !pill) {
    console.error("globeAdminQr: missing required DOM elements");
    return;
  }

  // Load saved position/size
  currentRect = loadOverlayRect();
  
  // Initialize overlay position and size
  updateOverlayStyle();
  
  // Set up button click handler
  btnAdminQr.addEventListener("click", () => {
    if (state === "closed") {
      openOverlay();
    } else if (state === "open") {
      closeOverlay();
    } else if (state === "minimized") {
      restoreOverlay();
    }
  });

  // Set up minimize button
  minimizeBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    minimizeOverlay();
  });

  // Set up pill click handler
  pill.addEventListener("click", () => {
    if (state === "minimized") {
      restoreOverlay();
    }
  });

  // Set up drag on overlay header
  const header = overlay.querySelector<HTMLElement>(".admin-qr-header");
  if (header) {
    header.style.touchAction = "none";
    header.addEventListener("pointerdown", (e: PointerEvent) => {
      if (e.button !== 0 && e.button !== undefined) return; // Only left button for mouse
      startDrag(e);
    });
  }

  // Set up resize handle
  resizeHandle.style.touchAction = "none";
  resizeHandle.addEventListener("pointerdown", (e: PointerEvent) => {
    if (e.button !== 0 && e.button !== undefined) return; // Only left button for mouse
    startResize(e);
  });

  // Set up window resize handler
  window.addEventListener("resize", () => {
    if (state !== "closed") {
      const viewport = getViewport();
      currentRect = clampOverlayRect(currentRect, viewport);
      updateOverlayStyle();
    }
  });

  // Set up pointer move and up handlers
  window.addEventListener("pointermove", (e: PointerEvent) => {
    if (isDragging && dragStart) {
      doDrag(e);
    } else if (isResizing && resizeStart) {
      doResize(e);
    }
  });

  window.addEventListener("pointerup", () => {
    if (isDragging) {
      isDragging = false;
      if (overlay && dragStart) {
        overlay.releasePointerCapture?.(dragStart.pointerId);
      }
      dragStart = null;
      saveOverlayRect(currentRect);
    }
    if (isResizing) {
      isResizing = false;
      if (resizeHandle && resizeStart) {
        resizeHandle.releasePointerCapture?.(resizeStart.pointerId);
      }
      resizeStart = null;
      saveOverlayRect(currentRect);
    }
  });

  // Set up image error handler for 401/403
  qrImg.addEventListener("error", async () => {
    // Check if it's an auth error
    if (!qrImg?.src) return;
    try {
      const res = await fetch(qrImg.src, { credentials: "same-origin" });
      if (res.status === 401 || res.status === 403) {
        showError(t("globe.adminQrError"));
        closeOverlay();
      }
    } catch {
      // Network error, ignore
    }
  });

  // Initialize with closed state
  setState("closed");
}

// Open the overlay
function openOverlay(): void {
  if (state !== "closed") return;
  
  const viewport = getViewport();
  currentRect = clampOverlayRect(currentRect, viewport);
  
  setState("open");
  updateOverlayStyle();
  
  // Start QR refresh
  refreshQr();
  shownAtMs = Date.now();
  
  // Set up timers
  stepTimer = window.setInterval(() => refreshQr(), CHECKIN_STEP_SEC * 1000);
  hideTimer = window.setTimeout(() => closeOverlay(), ADMIN_PHONE_QR_TIMEOUT_MS);
  
  // Set up consume polling
  consumeGraceTimer = window.setTimeout(() => {
    void watchConsumed();
    consumePollTimer = window.setInterval(() => void watchConsumed(), 2000);
  }, CONSUME_POLL_GRACE_MS);
  
  // Update status
  if (statusEl) {
    statusEl.textContent = t("globe.adminQrStatus");
  }
}

// Close the overlay
function closeOverlay(): void {
  if (state === "closed") return;
  
  // Clear all timers
  window.clearInterval(stepTimer);
  window.clearTimeout(hideTimer);
  window.clearTimeout(consumeGraceTimer);
  window.clearInterval(consumePollTimer);
  
  stepTimer = undefined;
  hideTimer = undefined;
  consumeGraceTimer = undefined;
  consumePollTimer = undefined;
  
  // Clear image source
  if (qrImg) {
    qrImg.removeAttribute("src");
  }
  
  shownAtMs = 0;
  setState("closed");
}

// Minimize the overlay
function minimizeOverlay(): void {
  if (state !== "open") return;
  
  // Clear timers but keep the QR shownAt for when we restore
  window.clearInterval(stepTimer);
  window.clearTimeout(hideTimer);
  window.clearTimeout(consumeGraceTimer);
  window.clearInterval(consumePollTimer);
  
  stepTimer = undefined;
  hideTimer = undefined;
  consumeGraceTimer = undefined;
  consumePollTimer = undefined;
  
  // Clear image source
  if (qrImg) {
    qrImg.removeAttribute("src");
  }
  
  setState("minimized");
}

// Restore from minimized state
function restoreOverlay(): void {
  if (state !== "minimized") return;
  
  setState("open");
  updateOverlayStyle();
  
  // Fresh QR on restore
  refreshQr();
  shownAtMs = Date.now();
  
  // Restart timers
  stepTimer = window.setInterval(() => refreshQr(), CHECKIN_STEP_SEC * 1000);
  hideTimer = window.setTimeout(() => closeOverlay(), ADMIN_PHONE_QR_TIMEOUT_MS);
  
  // Restart consume polling
  consumeGraceTimer = window.setTimeout(() => {
    void watchConsumed();
    consumePollTimer = window.setInterval(() => void watchConsumed(), 2000);
  }, CONSUME_POLL_GRACE_MS);
  
  // Update status
  if (statusEl) {
    statusEl.textContent = t("globe.adminQrStatus");
  }
}

// Refresh the QR image
function refreshQr(): void {
  if (state !== "open" || !qrImg) return;
  const url = `/api/admin/checkin-qr.svg?t=${Date.now()}`;
  qrImg.src = url;
}

// Watch for consumed QR
async function watchConsumed(): Promise<void> {
  if (state !== "open" || shownAtMs <= 0) return;
  if (Date.now() - shownAtMs < CONSUME_POLL_GRACE_MS) return;
  
  try {
    const res = await fetch(
      `/api/admin/checkin-qr/consumed?sinceMs=${encodeURIComponent(String(shownAtMs))}`,
      { credentials: "same-origin" },
    );
    if (!res.ok) return;
    const data = (await res.json()) as { consumed?: boolean };
    if (data.consumed) {
      // QR was consumed, rotate by closing and reopening
      closeOverlay();
      // Small delay before reopening to ensure fresh token
      window.setTimeout(() => {
        if (state === "closed") {
          openOverlay();
        }
      }, 100);
    }
  } catch {
    // Ignore network errors
  }
}

// Show error message
function showError(message: string): void {
  if (statusEl) {
    statusEl.textContent = message;
  }
}

// Update overlay style based on currentRect
function updateOverlayStyle(): void {
  if (!overlay) return;
  
  overlay.style.left = `${currentRect.x}px`;
  overlay.style.top = `${currentRect.y}px`;
  overlay.style.width = `${currentRect.width}px`;
  overlay.style.height = `${currentRect.height}px`;
}

// Get viewport dimensions
function getViewport(): { width: number; height: number } {
  return {
    width: window.innerWidth,
    height: window.innerHeight,
  };
}

// Set the overlay state
function setState(newState: OverlayState): void {
  state = newState;
  
  if (!btnAdminQr || !overlay || !pill) return;
  
  btnAdminQr.setAttribute("aria-pressed", String(state === "open"));
  
  switch (state) {
    case "closed":
      overlay.hidden = true;
      pill.hidden = true;
      btnAdminQr.setAttribute("aria-expanded", "false");
      break;
    case "open":
      overlay.hidden = false;
      pill.hidden = true;
      btnAdminQr.setAttribute("aria-expanded", "true");
      break;
    case "minimized":
      overlay.hidden = true;
      pill.hidden = false;
      btnAdminQr.setAttribute("aria-expanded", "false");
      break;
  }
}

// Start drag operation
function startDrag(e: PointerEvent): void {
  if (!overlay || isResizing) return;
  
  isDragging = true;
  dragStart = {
    x: e.clientX,
    y: e.clientY,
    rect: { ...currentRect },
    pointerId: e.pointerId,
  };
  
  overlay.setPointerCapture(e.pointerId);
  e.preventDefault();
}

// Perform drag
function doDrag(e: PointerEvent): void {
  if (!dragStart || !overlay) return;
  
  const dx = e.clientX - dragStart.x;
  const dy = e.clientY - dragStart.y;
  
  const viewport = getViewport();
  currentRect = {
    ...dragStart.rect,
    x: dragStart.rect.x + dx,
    y: dragStart.rect.y + dy,
  };
  
  currentRect = clampOverlayRect(currentRect, viewport);
  updateOverlayStyle();
  e.preventDefault();
}

// Start resize operation
function startResize(e: PointerEvent): void {
  if (!resizeHandle || isDragging) return;
  
  isResizing = true;
  resizeStart = {
    x: e.clientX,
    y: e.clientY,
    rect: { ...currentRect },
    pointerId: e.pointerId,
  };
  
  resizeHandle.setPointerCapture(e.pointerId);
  e.preventDefault();
}

// Perform resize (maintain square-ish aspect)
function doResize(e: PointerEvent): void {
  if (!resizeStart || !overlay) return;
  
  const dx = e.clientX - resizeStart.x;
  const dy = e.clientY - resizeStart.y;
  
  const viewport = getViewport();
  
  // Calculate new dimensions maintaining aspect ratio
  let newWidth = resizeStart.rect.width + dx;
  let newHeight = resizeStart.rect.height + dy;
  
  // Maintain square aspect (QR codes are square)
  const size = Math.max(newWidth, newHeight);
  newWidth = size;
  newHeight = size;
  
  currentRect = {
    ...resizeStart.rect,
    width: newWidth,
    height: newHeight,
  };
  
  currentRect = clampOverlayRect(currentRect, viewport);
  updateOverlayStyle();
  e.preventDefault();
}

// Public API
export function showAdminQrOverlay(): void {
  if (state === "closed") {
    openOverlay();
  }
}

export function hideAdminQrOverlay(): void {
  closeOverlay();
}

export function isAdminQrOverlayVisible(): boolean {
  return state === "open";
}

export function isAdminQrOverlayMinimized(): boolean {
  return state === "minimized";
}
