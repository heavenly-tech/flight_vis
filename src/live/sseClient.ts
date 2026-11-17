import type { LiveFix } from "./types";

export type LiveStreamErrorState = "auth" | "notfound" | "network";

export type LiveStreamCallbacks = {
  onFix: (fix: LiveFix) => void;
  onEnd: (sessionId: string) => void;
  onError: (state: LiveStreamErrorState) => void;
};

export function parseFixEventData(data: string): LiveFix | null {
  try {
    const parsed = JSON.parse(data) as LiveFix;
    if (typeof parsed.t !== "number" || !Number.isFinite(parsed.lat) || !Number.isFinite(parsed.lon)) {
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

export function parseEndEventData(data: string): string | null {
  try {
    const parsed = JSON.parse(data) as { id?: string };
    return typeof parsed.id === "string" ? parsed.id : null;
  } catch {
    return null;
  }
}

/** Returns fixes not yet seen; mutates `seen` with new timestamps. */
export function dedupeFixes(fixes: LiveFix[], seen: Set<number>): LiveFix[] {
  const out: LiveFix[] = [];
  for (const fix of fixes) {
    if (seen.has(fix.t)) continue;
    seen.add(fix.t);
    out.push(fix);
  }
  return out;
}

export function applyFixDedupe(fix: LiveFix, seen: Set<number>): boolean {
  if (seen.has(fix.t)) return false;
  seen.add(fix.t);
  return true;
}

export type LiveStreamHandle = {
  close: () => void;
};

export type OpenLiveStreamHandlers = {
  onOpen?: () => void;
  onReconnect?: () => void;
  onError?: (status: number) => void | Promise<void>;
  onFix: (fix: LiveFix) => void;
  onEnd: () => void;
};

/** Integration entry point used by `runtime.ts`. */
export function openLiveStream(sessionId: string, handlers: OpenLiveStreamHandlers): LiveStreamHandle {
  let opened = false;
  return connectLiveStream(
    sessionId,
    {
      onFix: handlers.onFix,
      onEnd: handlers.onEnd,
      onError: (state) => {
        if (state === "auth") {
          void handlers.onError?.(401);
          return;
        }
        if (state === "notfound") {
          void handlers.onError?.(404);
          return;
        }
        if (opened) handlers.onReconnect?.();
        void handlers.onError?.(0);
      },
    },
    (url) => {
      const source = defaultEventSource(url);
      source.addEventListener("open", () => {
        if (opened) handlers.onReconnect?.();
        else {
          opened = true;
          handlers.onOpen?.();
        }
      });
      return source;
    },
  );
}

export function connectLiveStream(
  sessionId: string,
  callbacks: LiveStreamCallbacks,
  eventSourceFactory: (url: string) => EventSource = defaultEventSource,
): LiveStreamHandle {
  const seen = new Set<number>();
  const url = `/api/live/${encodeURIComponent(sessionId)}/stream`;
  let source: EventSource | null = null;
  let closed = false;
  let classifyAbort: AbortController | null = null;

  const open = (): void => {
    if (closed) return;
    source?.close();
    source = eventSourceFactory(url);

    source.addEventListener("fix", (ev) => {
      const fix = parseFixEventData((ev as MessageEvent).data);
      if (!fix || !applyFixDedupe(fix, seen)) return;
      callbacks.onFix(fix);
    });

    source.addEventListener("end", (ev) => {
      const id = parseEndEventData((ev as MessageEvent).data) ?? sessionId;
      callbacks.onEnd(id);
      source?.close();
    });

    source.onerror = () => {
      classifyAbort?.abort();
      classifyAbort = new AbortController();
      const signal = classifyAbort.signal;
      void classifyStreamError(url, signal).then((state) => {
        if (closed || signal.aborted) return;
        if (state === "auth" || state === "notfound") {
          closed = true;
          source?.close();
          callbacks.onError(state);
          return;
        }
        callbacks.onError("network");
      });
    };
  };

  open();

  return {
    close: () => {
      closed = true;
      classifyAbort?.abort();
      classifyAbort = null;
      source?.close();
      source = null;
    },
  };
}

function defaultEventSource(url: string): EventSource {
  return new EventSource(url, { withCredentials: true });
}

export async function classifyStreamError(
  url: string,
  signal?: AbortSignal,
): Promise<LiveStreamErrorState> {
  try {
    const res = await fetch(url, {
      credentials: "same-origin",
      headers: { Accept: "text/event-stream" },
      signal,
    });
    if (res.status === 401 || res.status === 403) return "auth";
    if (res.status === 404) return "notfound";
    return "network";
  } catch {
    return "network";
  }
}

/** Replay fixes as if from SSE (for tests and reconnect dedup). */
export function replayFixes(
  fixes: LiveFix[],
  seen: Set<number>,
  onFix: (fix: LiveFix) => void,
): number {
  const fresh = dedupeFixes(fixes, seen);
  for (const fix of fresh) onFix(fix);
  return fresh.length;
}