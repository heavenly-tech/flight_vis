/**
 * Cross-window signal bus between admin pages and club-screen.html on the same PC.
 * Transports ONLY lightweight signals ("refetch" | "logout") — never flight, map, QR, or club data.
 * Uses BroadcastChannel with a localStorage ping fallback (timestamp + nonce).
 */
export const KIOSK_SYNC_CHANNEL = "fv-kiosk";
export const KIOSK_SYNC_STORAGE_KEY = "fv-kiosk-sync";

export type KioskSyncSignal = "refetch" | "logout";

export interface KioskSyncMessage {
  type: KioskSyncSignal;
}

function isSignal(value: unknown): value is KioskSyncSignal {
  return value === "refetch" || value === "logout";
}

function parseChannelMessage(data: unknown): KioskSyncSignal | null {
  if (!data || typeof data !== "object") return null;
  const raw = (data as { type?: unknown }).type;
  return isSignal(raw) ? raw : null;
}

function parseStoragePing(raw: string | null): KioskSyncSignal | null {
  if (typeof raw !== "string") return null;
  const idx = raw.indexOf(":");
  const head = idx >= 0 ? raw.slice(0, idx) : raw;
  return isSignal(head) ? head : null;
}

let nonceSeq = 0;

function makePingValue(type: KioskSyncSignal): string {
  nonceSeq = (nonceSeq + 1) & 0x7fffffff;
  const rand = Math.random().toString(36).slice(2, 8);
  return `${type}:${Date.now()}:${nonceSeq}-${rand}`;
}

export function sendKioskSignal(type: KioskSyncSignal): void {
  if (typeof BroadcastChannel !== "undefined") {
    try {
      const ch = new BroadcastChannel(KIOSK_SYNC_CHANNEL);
      const msg: KioskSyncMessage = { type };
      ch.postMessage(msg);
      ch.close();
    } catch {
      /* ignore BroadcastChannel errors and still emit storage ping */
    }
  }
  if (typeof localStorage !== "undefined") {
    try {
      localStorage.setItem(KIOSK_SYNC_STORAGE_KEY, makePingValue(type));
    } catch {
      /* storage may be unavailable in private browsing */
    }
  }
}

export function notifyKioskRefetch(): void {
  sendKioskSignal("refetch");
}

export function notifyKioskLogout(): void {
  sendKioskSignal("logout");
}

export function subscribeKioskSignals(
  onSignal: (signal: KioskSyncSignal) => void,
): () => void {
  let channel: BroadcastChannel | null = null;
  const onMessage = (ev: MessageEvent) => {
    const sig = parseChannelMessage(ev.data);
    if (sig) onSignal(sig);
  };
  if (typeof BroadcastChannel !== "undefined") {
    try {
      channel = new BroadcastChannel(KIOSK_SYNC_CHANNEL);
      channel.addEventListener("message", onMessage);
    } catch {
      channel = null;
    }
  }

  const onStorage = (ev: StorageEvent) => {
    if (ev.key !== KIOSK_SYNC_STORAGE_KEY) return;
    const sig = parseStoragePing(ev.newValue);
    if (sig) onSignal(sig);
  };
  if (typeof window !== "undefined") {
    window.addEventListener("storage", onStorage);
  }

  return () => {
    if (channel) {
      channel.removeEventListener("message", onMessage);
      try {
        channel.close();
      } catch {
        /* ignore */
      }
      channel = null;
    }
    if (typeof window !== "undefined") {
      window.removeEventListener("storage", onStorage);
    }
  };
}
