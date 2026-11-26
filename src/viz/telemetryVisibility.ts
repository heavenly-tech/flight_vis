export const TELEMETRY_VISIBLE_STORAGE_KEY = "fv.telemetryVisible";

type StorageReader = Pick<Storage, "getItem">;
type StorageWriter = Pick<Storage, "setItem">;

function defaultStorage(): Storage | null {
  try {
    return typeof globalThis !== "undefined" && "localStorage" in globalThis
      ? globalThis.localStorage
      : null;
  } catch {
    return null;
  }
}

export function loadTelemetryVisible(storage?: StorageReader | null): boolean {
  const store = storage === undefined ? defaultStorage() : storage;
  if (!store) return true;
  try {
    const raw = store.getItem(TELEMETRY_VISIBLE_STORAGE_KEY);
    if (raw === "false" || raw === "0") return false;
    return true;
  } catch {
    return true;
  }
}

export function saveTelemetryVisible(visible: boolean, storage?: StorageWriter | null): void {
  const store = storage === undefined ? defaultStorage() : storage;
  if (!store) return;
  try {
    store.setItem(TELEMETRY_VISIBLE_STORAGE_KEY, visible ? "true" : "false");
  } catch {
    /* ignore storage quota / privacy mode errors */
  }
}

export type ShortcutEventLike = {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
  repeat?: boolean;
  defaultPrevented?: boolean;
  target: EventTarget | null;
};

export function isEditableTarget(target: EventTarget | null): boolean {
  if (!target || typeof target !== "object") return false;
  let el: Element | null = null;
  if ("tagName" in target && typeof (target as Element).tagName === "string") {
    el = target as Element;
  } else if ("parentElement" in target) {
    el = (target as Node).parentElement;
  }
  if (!el) return false;
  const tag = el.tagName.toUpperCase();
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  if ((el as HTMLElement).isContentEditable === true) return true;
  let cur: Element | null = el;
  while (cur) {
    const attr = typeof cur.getAttribute === "function" ? cur.getAttribute("contenteditable") : null;
    if (attr !== null && attr !== undefined) {
      const normalized = attr.toLowerCase();
      if (normalized === "" || normalized === "true" || normalized === "plaintext-only") {
        return true;
      }
      if (normalized === "false") {
        return false;
      }
    }
    cur = cur.parentElement ?? null;
  }
  return false;
}

export function shouldHandleShortcut(event: ShortcutEventLike): boolean {
  if (event.defaultPrevented || event.repeat) return false;
  if (event.ctrlKey || event.metaKey || event.altKey) return false;
  if (event.key !== "h" && event.key !== "H") return false;
  if (isEditableTarget(event.target)) return false;
  return true;
}

export function applyTelemetryVisibility(opts: {
  hud: HTMLElement;
  toggleButton?: HTMLElement | null;
  body?: HTMLElement | null;
  visible: boolean;
  hasFlight: boolean;
}): boolean {
  const { hud, toggleButton, body, visible, hasFlight } = opts;
  toggleButton?.setAttribute("aria-pressed", String(visible));
  const showHud = visible && hasFlight;
  hud.hidden = !showHud;
  body?.classList.toggle("telemetry-hidden", !visible);
  return showHud;
}
