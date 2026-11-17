import type { AuthStatus } from "../api";
import { fullRange } from "../track/trim";
import type { ColorMode, Flight, TrimRange } from "../track/types";
import { sampleAt } from "../track/parse";
import type { FlightViewer } from "../viz/viewer";
import { fetchLiveFixes, listLiveSessions, loginUrl } from "./api";
import { fixesToFlight } from "./fixesToFlight";
import { flightToGpx } from "./flightToGpx";
import { dedupeFixes } from "./sseClient";
import { getLocalFlight, putLocalFlight } from "./localFlights";
import { liveFixesFromGpx } from "./gpxLive";
import { detectLang, msg, setLiveLang, type LiveLang } from "./messages";
import { openLiveStream, type LiveStreamHandle } from "./sseClient";
import type { LiveFix } from "./types";

const LIVE_PATH = /^\/live(?:\/([A-Za-z0-9._-]+))?\/?$/i;
const LIVE_LAG_MS = 2000;
const ACC_DIM_M = 50;

export function parseLivePath(pathname: string): { listOnly: boolean; sessionId: string | null } {
  const match = pathname.match(LIVE_PATH);
  if (!match) return { listOnly: false, sessionId: null };
  return { listOnly: !match[1], sessionId: match[1] ?? null };
}

export type LiveHost = {
  viewer: FlightViewer;
  getAuth: () => AuthStatus;
  getFlight: () => Flight | null;
  setFlight: (flight: Flight | null) => void;
  getTrim: () => TrimRange | null;
  setTrim: (trim: TrimRange | null) => void;
  getCursor: () => number;
  setCursor: (ms: number) => void;
  isFollow: () => boolean;
  getFollowingLive: () => boolean;
  setFollowingLive: (v: boolean) => void;
  setPlaying: (v: boolean) => void;
  syncPlayLabel: () => void;
  applyTrim: (fly?: boolean) => Promise<void>;
  afterSeek: () => void;
  refreshHud: () => void;
  refreshProfile: () => void;
  setStatus: (text: string | null) => void;
  setFlightName: (text: string) => void;
  enableFollow: () => void;
  refreshLibrary: () => Promise<void>;
  getColorMode: () => ColorMode;
  loadRecording: (
    track: string,
    fileName: string,
    trim: TrimRange,
    name: string,
    localId?: string,
  ) => Promise<void>;
};

export type LiveRuntime = {
  isActive: () => boolean;
  edgeMs: () => number;
  capTime: (ms: number) => number;
  playCap: () => number | null;
  onScrub: () => void;
  tickFollow: () => void;
  startList: () => Promise<void>;
  startSession: (sessionId: string) => Promise<void>;
  stop: () => void;
  lang: () => LiveLang;
  toggleLang: () => void;
};

export function createLiveRuntime(host: LiveHost): LiveRuntime {
  let lang = detectLang();
  let active = false;
  let sessionId: string | null = null;
  let stream: LiveStreamHandle | null = null;
  let followingLive = true;
  let pendingSave: { flight: Flight; trim: TrimRange; localId: string } | null = null;
  let recordingId: string | null = null;
  const allFixes: LiveFix[] = [];
  const seenFixTimes = new Set<number>();

  const liveBtn = document.querySelector<HTMLButtonElement>("#btn-live")!;
  const recordBtn = document.querySelector<HTMLButtonElement>("#btn-record")!;
  const sessionsBtn = document.querySelector<HTMLButtonElement>("#btn-live-sessions")!;
  const liveStatus = document.querySelector<HTMLParagraphElement>("#live-status")!;
  const langBtn = document.querySelector<HTMLButtonElement>("#btn-lang")!;
  const trimDialog = document.querySelector<HTMLDialogElement>("#live-trim-dialog")!;
  const trimForm = document.querySelector<HTMLFormElement>("#live-trim-form")!;
  const trimTitle = document.querySelector<HTMLParagraphElement>("#live-trim-title")!;
  const trimStart = document.querySelector<HTMLInputElement>("#live-trim-start")!;
  const trimEnd = document.querySelector<HTMLInputElement>("#live-trim-end")!;
  const trimTz = document.querySelector<HTMLParagraphElement>("#live-trim-tz")!;
  const timelineEdge = document.querySelector<HTMLElement>("#timeline-live-edge")!;

  function setLiveStatus(text: string | null): void {
    if (!text) {
      liveStatus.hidden = true;
      liveStatus.textContent = "";
      return;
    }
    liveStatus.hidden = false;
    liveStatus.textContent = text;
  }

  function applyLangUi(): void {
    langBtn.textContent = lang.toUpperCase();
    sessionsBtn.textContent = msg("liveSessions", lang);
    trimTitle.textContent = msg("trimTitle", lang);
    liveBtn.title = msg("followLive", lang);
  }

  function recordingLocalId(): string | null {
    return sessionId ? `local:${sessionId}` : null;
  }

  function syncLiveButton(): void {
    if (!active) {
      liveBtn.hidden = true;
      return;
    }
    const edge = edgeMs();
    const lag = edge - host.getCursor();
    if (lag <= LIVE_LAG_MS) {
      liveBtn.hidden = true;
      liveBtn.classList.add("is-live-edge");
      liveBtn.textContent = msg("followLive", lang);
      return;
    }
    liveBtn.hidden = false;
    liveBtn.classList.remove("is-live-edge");
    const secs = Math.ceil(lag / 1000);
    const mm = String(Math.floor(secs / 60)).padStart(2, "0");
    const ss = String(secs % 60).padStart(2, "0");
    liveBtn.textContent = `−${mm}:${ss}`;
  }

  function edgeMs(): number {
    const f = host.getFlight();
    if (!f || f.points.length === 0) return 0;
    return f.points[f.points.length - 1].time;
  }

  function capTime(ms: number): number {
    if (!active) return ms;
    const flight = host.getFlight();
    const trim = host.getTrim();
    if (!flight || !trim) return ms;
    return Math.min(Math.max(ms, trim.startMs), edgeMs());
  }

  async function rebuildFromAllFixes(): Promise<void> {
    if (allFixes.length < 2 || !sessionId) return;
    const prevLen = host.getFlight()?.points.length ?? 0;
    const next = fixesToFlight(allFixes, { name: `Live ${sessionId.slice(0, 8)}`, fileName: `${sessionId}.gpx` });
    host.setFlight(next);
    const range = host.getTrim() ?? fullRange(next);
    host.setTrim({ startMs: range.startMs, endMs: edgeMs() });

    host.setFlightName(next.name);
    host.setStatus(null);
    if (prevLen === 0) {
      await host.viewer.setFlight(next, host.getColorMode(), host.getTrim() ?? fullRange(next), false);
      host.viewer.setAccuracyDim(ACC_DIM_M);
      host.setCursor(edgeMs());
      host.afterSeek();
    } else if (next.points.length > prevLen) {
      await host.viewer.appendLivePoints(next.points.slice(prevLen));
    } else {
      await host.viewer.setFlight(next, host.getColorMode(), host.getTrim() ?? fullRange(next), false);
    }

    if (followingLive) {
      host.setCursor(edgeMs());
      host.viewer.setSample(sampleAt(next, host.getCursor()), host.isFollow());
      host.refreshHud();
    }
    syncLiveButton();
    host.refreshProfile();
    await flushRecording();
  }

  async function ingestFixes(fixes: LiveFix[]): Promise<void> {
    const fresh = dedupeFixes(fixes, seenFixTimes);
    if (fresh.length === 0) return;
    allFixes.push(...fresh);
    allFixes.sort((a, b) => a.t - b.t);
    await rebuildFromAllFixes();
  }

  async function seedBufferedFixes(serverFixes: LiveFix[] | null): Promise<void> {
    const localId = recordingLocalId();
    const local = localId ? await getLocalFlight(localId) : null;
    const localFixes = local ? liveFixesFromGpx(local.track) : [];
    const combined = [...localFixes, ...(serverFixes ?? [])].sort((a, b) => a.t - b.t);
    allFixes.length = 0;
    seenFixTimes.clear();
    const merged: LiveFix[] = [];
    for (const fix of combined) {
      if (seenFixTimes.has(fix.t)) continue;
      seenFixTimes.add(fix.t);
      merged.push(fix);
    }
    allFixes.push(...merged);
    await rebuildFromAllFixes();
  }

  async function flushRecording(): Promise<void> {
    const flight = host.getFlight();
    const trim = host.getTrim();
    const id = recordingLocalId();
    if (!flight || !trim || !id) return;
    const track = flightToGpx(flight);
    recordingId = id;
    await putLocalFlight({
      id,
      name: flight.name,
      fileName: flight.fileName,
      track,
      trim,
      tags: [],
    });
    await host.refreshLibrary();
  }

  function openTrimDialog(flight: Flight, localId: string): void {
    pendingSave = { flight, trim: fullRange(flight), localId };
    const end = flight.points[flight.points.length - 1].time;
    const start = flight.points[0].time;
    trimEnd.value = msToLocalInput(end);
    trimStart.value = msToLocalInput(start);
    const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    trimTz.textContent =
      lang === "es"
        ? `Zona horaria del dispositivo: ${zone}`
        : `Device time zone: ${zone}`;
    trimDialog.showModal();
  }

  function msToLocalInput(ms: number): string {
    const d = new Date(ms);
    const pad = (n: number) => String(n).padStart(2, "0");
    return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  }

  function inputToMs(value: string): number | null {
    const ms = Date.parse(value);
    return Number.isFinite(ms) ? ms : null;
  }

  async function saveTrimmed(): Promise<void> {
    if (!pendingSave) return;
    const start = inputToMs(trimStart.value);
    const end = inputToMs(trimEnd.value);
    if (start == null || end == null || end <= start) return;
    const trim: TrimRange = { startMs: start, endMs: end };
    const track = flightToGpx(pendingSave.flight);
    const saved = await putLocalFlight({
      id: pendingSave.localId,
      name: pendingSave.flight.name,
      fileName: pendingSave.flight.fileName,
      track,
      trim,
      tags: [],
    });
    const fileName = pendingSave.flight.fileName;
    const name = pendingSave.flight.name;
    pendingSave = null;
    await host.refreshLibrary();
    host.setStatus(msg("recorded", lang));
    window.setTimeout(() => host.setStatus(null), 2600);
    await host.loadRecording(track, fileName, trim, name, saved.id);
  }

  async function attachStream(id: string): Promise<void> {
    stream?.close();
    setLiveStatus(msg("connecting", lang));
    let initial: LiveFix[] | null;
    try {
      initial = await fetchLiveFixes(id);
    } catch (error) {
      if (error instanceof Error && error.message === "auth") {
        host.setStatus(msg("loginRequired", lang));
        return;
      }
      host.setStatus(msg("notFound", lang));
      return;
    }
    if (initial === null) {
      host.setStatus(msg("notFound", lang));
      setLiveStatus(null);
      return;
    }
    await seedBufferedFixes(initial);

    stream = openLiveStream(id, {
      onOpen: () => setLiveStatus(null),
      onReconnect: () => setLiveStatus(msg("reconnecting", lang)),
      onError: async (status) => {
        if (status === 401) {
          host.setStatus(msg("loginRequired", lang));
          setLiveStatus(null);
          stream?.close();
          stream = null;
          return;
        }
        if (status === 404) {
          host.setStatus(msg("notFound", lang));
          setLiveStatus(null);
          active = false;
          stream?.close();
          stream = null;
          return;
        }
        setLiveStatus(msg("reconnecting", lang));
      },
      onFix: (fix) => {
        void ingestFixes([fix]);
      },
      onEnd: async () => {
        active = false;
        stream = null;
        document.body.classList.remove("live-session");
        liveBtn.hidden = true;
        setLiveStatus(msg("sessionEnded", lang));
        const flight = host.getFlight();
        if (flight) {
          await flushRecording();
          const id = recordingId ?? recordingLocalId();
          if (id) openTrimDialog(flight, id);
        }
      },
    });
  }

  liveBtn.addEventListener("click", () => {
    followingLive = true;
    host.setFollowingLive(true);
    host.enableFollow();
    host.setCursor(edgeMs());
    const flight = host.getFlight();
    if (flight) {
      host.viewer.setSample(sampleAt(flight, host.getCursor()), true);
    }
    host.afterSeek();
    syncLiveButton();
  });

  recordBtn.hidden = true;

  sessionsBtn.addEventListener("click", () => {
    void startList();
  });

  langBtn.addEventListener("click", () => {
    lang = lang === "en" ? "es" : "en";
    setLiveLang(lang);
    applyLangUi();
    syncLiveButton();
  });

  trimForm.addEventListener("submit", (event) => {
    const submitter = (event as SubmitEvent).submitter as HTMLButtonElement | null;
    if (submitter?.value === "cancel") {
      pendingSave = null;
      return;
    }
    event.preventDefault();
    void saveTrimmed().then(() => trimDialog.close());
  });

  document.querySelectorAll<HTMLButtonElement>("[data-tail-min]").forEach((button) => {
    button.addEventListener("click", () => {
      const minutes = Number(button.dataset.tailMin);
      const flight = pendingSave?.flight ?? host.getFlight();
      if (!flight) return;
      const end = flight.points[flight.points.length - 1].time;
      const start = Math.max(flight.points[0].time, end - minutes * 60_000);
      trimStart.value = msToLocalInput(start);
      trimEnd.value = msToLocalInput(end);
      document.querySelectorAll("[data-tail-min]").forEach((el) => el.classList.toggle("active", el === button));
    });
  });

  applyLangUi();

  async function startList(): Promise<void> {
    const auth = host.getAuth();
    if (!auth.authenticated) {
      window.location.href = loginUrl(`/live`);
      return;
    }
    try {
      const sessions = await listLiveSessions();
      const activeSessions = sessions.filter((s) => s.phase === "active");
      if (!activeSessions.length) {
        host.setStatus(msg("noLiveSessions", lang));
        window.setTimeout(() => host.setStatus(null), 2600);
        return;
      }
      const pick = activeSessions[0];
      history.replaceState({ live: pick.id }, "", `/live/${pick.id}`);
      await startSession(pick.id);
    } catch (error) {
      if (error instanceof Error && error.message === "auth") {
        host.setStatus(msg("loginRequired", lang));
        return;
      }
      host.setStatus(msg("noLiveSessions", lang));
    }
  }

  async function startSession(id: string): Promise<void> {
    allFixes.length = 0;
    seenFixTimes.clear();
    recordingId = `local:${id}`;
    host.setStatus(null);
    host.setFlightName(`Live ${id.slice(0, 8)}`);
    const auth = host.getAuth();
    if (!auth.authenticated) {
      host.setStatus(msg("loginRequired", lang));
      window.setTimeout(() => {
        window.location.href = loginUrl(`/live/${id}`);
      }, 900);
      return;
    }
    sessionId = id;
    active = true;
    followingLive = true;
    host.setFollowingLive(true);
    document.body.classList.add("live-session");
    liveBtn.hidden = false;
    sessionsBtn.hidden = false;
    host.viewer.setAccuracyDim(ACC_DIM_M);
    history.replaceState({ live: id }, "", `/live/${id}`);
    await attachStream(id);
    syncLiveButton();
    timelineEdge.hidden = false;
  }

  return {
    isActive: () => active,
    edgeMs,
    capTime,
    playCap: () => (active ? edgeMs() : null),
    onScrub: () => {
      if (!active) return;
      const lag = edgeMs() - host.getCursor();
      followingLive = lag <= LIVE_LAG_MS;
      host.setFollowingLive(followingLive);
      syncLiveButton();
    },
    tickFollow: () => {
      if (!active || !followingLive) return;
      const edge = edgeMs();
      if (host.getCursor() < edge - 250) {
        host.setCursor(edge);
        const flight = host.getFlight();
        if (flight) host.viewer.setSample(sampleAt(flight, edge), host.isFollow());
        host.refreshHud();
      }
      syncLiveButton();
    },
    startList,
    startSession,
    stop: () => {
      stream?.close();
      stream = null;
      active = false;
      document.body.classList.remove("live-session");
    },
    lang: () => lang,
    toggleLang: () => {
      lang = lang === "en" ? "es" : "en";
      setLiveLang(lang);
      applyLangUi();
    },
  };
}

export { ACC_DIM_M };