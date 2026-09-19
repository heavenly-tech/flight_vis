import "cesium/Build/Cesium/Widgets/widgets.css";
import "./style.css";
import { getAuth, getFlight, loginUrl, logout, saveFlight, shareUrl, type AuthStatus } from "./api";
import { formatClock, formatDuration } from "./track/geo";
import { parseFlight, sampleAt } from "./track/parse";
import { isValidSlug, normalizeSlug, slugFromPath } from "./track/slug";
import type { ColorMode, Flight, FlightTag, TrimRange } from "./track/types";
import { clampTrim, fullRange, newTagId, rangeStats, suggestTrim } from "./track/trim";
import { cssForUnit } from "./viz/colors";
import { createFlightViewer } from "./viz/viewer";

const SAMPLE_URL = `${import.meta.env.BASE_URL}samples/vuelo5.gpx`;
const HANDLE = 10;

const fileInput = document.querySelector<HTMLInputElement>("#file-input")!;
const playButton = document.querySelector<HTMLButtonElement>("#btn-play")!;
const followButton = document.querySelector<HTMLButtonElement>("#btn-follow")!;
const overviewButton = document.querySelector<HTMLButtonElement>("#btn-overview")!;
const shareButton = document.querySelector<HTMLButtonElement>("#btn-share")!;
const slugInput = document.querySelector<HTMLInputElement>("#slug-input")!;
const skipButton = document.querySelector<HTMLButtonElement>("#btn-skip-ground")!;
const startHereButton = document.querySelector<HTMLButtonElement>("#btn-start-here")!;
const endHereButton = document.querySelector<HTMLButtonElement>("#btn-end-here")!;
const tagButton = document.querySelector<HTMLButtonElement>("#btn-tag")!;
const speedSelect = document.querySelector<HTMLSelectElement>("#play-speed")!;
const profile = document.querySelector<HTMLCanvasElement>("#profile")!;
const statusEl = document.querySelector<HTMLParagraphElement>("#status")!;
const hud = document.querySelector<HTMLElement>("#hud")!;
const hudToggle = document.querySelector<HTMLButtonElement>("#btn-hud")!;
const transport = document.querySelector<HTMLElement>(".transport")!;
const nameEl = document.querySelector<HTMLParagraphElement>("#flight-name")!;
const summaryEl = document.querySelector<HTMLParagraphElement>("#summary")!;
const tagList = document.querySelector<HTMLDivElement>("#tag-list")!;
const tagDialog = document.querySelector<HTMLDialogElement>("#tag-dialog")!;
const tagForm = document.querySelector<HTMLFormElement>("#tag-form")!;
const tagLabel = document.querySelector<HTMLInputElement>("#tag-label")!;
const fileButton = document.querySelector<HTMLButtonElement>("#btn-file")!;
const fileMenu = document.querySelector<HTMLDivElement>("#file-menu")!;
const filePanel = document.querySelector<HTMLDivElement>("#file-panel")!;
const tagsButton = document.querySelector<HTMLButtonElement>("#btn-tags")!;
const tagsPanel = document.querySelector<HTMLDivElement>("#tags-panel")!;
const moreButton = document.querySelector<HTMLButtonElement>("#btn-more")!;
const morePanel = document.querySelector<HTMLDivElement>("#more-panel")!;
const shareButtonLabel = shareButton.textContent ?? "Save & copy link";
const authHint = document.querySelector<HTMLParagraphElement>("#auth-hint")!;
const loginLink = document.querySelector<HTMLAnchorElement>("#btn-login")!;
const logoutButton = document.querySelector<HTMLButtonElement>("#btn-logout")!;

let auth: AuthStatus = { authenticated: false, configured: true };

function syncAuthUi(): void {
  loginLink.href = loginUrl();
  loginLink.hidden = auth.authenticated;
  logoutButton.hidden = !auth.authenticated;
  if (!auth.configured) {
    authHint.hidden = false;
    authHint.textContent = "Upload is locked until FLIGHT_VIS_PASSWORD is set.";
    shareButton.textContent = "Save locked";
    return;
  }
  if (auth.authenticated) {
    authHint.hidden = true;
    shareButton.textContent = shareButtonLabel;
    return;
  }
  authHint.hidden = false;
  authHint.textContent = "Log in to upload and save a shareable slug.";
  shareButton.textContent = "Log in to save";
}

const ctx = profile.getContext("2d")!;

let flight: Flight | null = null;
let trackText = "";
let trim: TrimRange | null = null;
let tags: FlightTag[] = [];
let playing = false;
let follow = true;
let colorMode: ColorMode = "altitude";
let cursorMs = 0;
let lastFrame = 0;
let playSpeed = Number(speedSelect.value);
let drag: "start" | "end" | "seek" | null = null;
let applyTrim: ((fly?: boolean) => Promise<void>) | null = null;
let seekPlayhead: ((timeMs: number) => void) | null = null;

function pathSlug(): string | null {
  return slugFromPath(window.location.pathname);
}

function setStatus(text: string | null): void {
  if (!text) {
    statusEl.hidden = true;
    statusEl.textContent = "";
    return;
  }
  statusEl.hidden = false;
  statusEl.textContent = text;
}

function xForTime(timeMs: number, width: number, pad: number): number {
  if (!flight) return pad;
  return pad + ((timeMs - flight.points[0].time) / flight.durationMs) * (width - pad * 2);
}

function timeForX(x: number, width: number, pad: number): number {
  if (!flight) return 0;
  const u = Math.min(1, Math.max(0, (x - pad) / (width - pad * 2)));
  return flight.points[0].time + u * flight.durationMs;
}

function resizeProfile(): void {
  const ratio = window.devicePixelRatio || 1;
  const width = profile.clientWidth;
  const height = Math.max(48, profile.clientHeight || 100);
  profile.width = Math.max(1, Math.floor(width * ratio));
  profile.height = Math.floor(height * ratio);
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  drawProfile();
}

function drawProfile(): void {
  const width = profile.clientWidth;
  const height = Math.max(48, profile.clientHeight || 100);
  ctx.clearRect(0, 0, width, height);
  if (!flight || !trim) return;

  const pts = flight.points;
  const pad = 10;
  const span = Math.max(1, flight.maxAlt - flight.minAlt);
  const xOf = (t: number) => xForTime(t, width, pad);
  const yOf = (alt: number) => height - pad - ((alt - flight!.minAlt) / span) * (height - pad * 2);

  ctx.beginPath();
  pts.forEach((p, i) => {
    if (i === 0) ctx.moveTo(xOf(p.time), yOf(p.alt));
    else ctx.lineTo(xOf(p.time), yOf(p.alt));
  });
  ctx.lineTo(width - pad, height - pad);
  ctx.lineTo(pad, height - pad);
  ctx.closePath();
  ctx.fillStyle = "rgba(103, 232, 249, 0.08)";
  ctx.fill();

  ctx.beginPath();
  pts.forEach((p, i) => {
    if (i === 0) ctx.moveTo(xOf(p.time), yOf(p.alt));
    else ctx.lineTo(xOf(p.time), yOf(p.alt));
  });
  ctx.strokeStyle = "rgba(167, 243, 208, 0.35)";
  ctx.lineWidth = 1.5;
  ctx.stroke();

  const startX = xOf(trim.startMs);
  const endX = xOf(trim.endMs);
  ctx.fillStyle = "rgba(7, 9, 13, 0.55)";
  ctx.fillRect(pad, pad / 2, Math.max(0, startX - pad), height - pad);
  ctx.fillRect(endX, pad / 2, Math.max(0, width - pad - endX), height - pad);

  const window = trim;
  ctx.beginPath();
  const inner = pts.filter((p) => p.time >= window.startMs && p.time <= window.endMs);
  inner.forEach((p, i) => {
    if (i === 0) ctx.moveTo(xOf(p.time), yOf(p.alt));
    else ctx.lineTo(xOf(p.time), yOf(p.alt));
  });
  ctx.strokeStyle = cssForUnit(0.55);
  ctx.lineWidth = 2.4;
  ctx.stroke();

  for (const tag of tags) {
    const x = xOf(tag.timeMs);
    ctx.fillStyle = "#fbbf24";
    ctx.beginPath();
    ctx.moveTo(x, 6);
    ctx.lineTo(x + 5, 16);
    ctx.lineTo(x - 5, 16);
    ctx.closePath();
    ctx.fill();
  }

  ctx.fillStyle = "#ecebff";
  ctx.fillRect(startX - 3, 8, 6, height - 16);
  ctx.fillRect(endX - 3, 8, 6, height - 16);

  const playX = xOf(cursorMs);
  ctx.fillStyle = "#fff";
  ctx.fillRect(playX - 1, pad / 2, 2, height - pad);
}

function renderTags(): void {
  tagList.innerHTML = "";
  if (tags.length === 0) {
    tagList.hidden = true;
    return;
  }
  tagList.hidden = false;
  for (const tag of tags) {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "tag-chip";
    chip.textContent = tag.label;
    chip.addEventListener("click", () => seekPlayhead?.(tag.timeMs));
    const remove = document.createElement("span");
    remove.textContent = "×";
    remove.title = "Remove tag";
    remove.addEventListener("click", (event) => {
      event.stopPropagation();
      tags = tags.filter((t) => t.id !== tag.id);
      renderTags();
      drawProfile();
      window.dispatchEvent(new CustomEvent("flight-tags"));
    });
    chip.append(remove);
    tagList.append(chip);
  }
}

function renderHud(): void {
  if (!flight || !trim) return;
  const sample = sampleAt(flight, cursorMs);
  const p = sample.point;
  const stats = rangeStats(flight, trim);
  document.querySelector("#stat-alt")!.textContent = `${Math.round(p.alt)} m`;
  document.querySelector("#stat-spd")!.textContent = `${(p.speedMps * 3.6).toFixed(0)} km/h`;
  document.querySelector("#stat-vario")!.textContent = `${p.varioMps >= 0 ? "+" : ""}${p.varioMps.toFixed(1)} m/s`;
  document.querySelector("#stat-dist")!.textContent = `${(p.distanceM / 1000).toFixed(1)} km`;
  document.querySelector("#stat-time")!.textContent = formatDuration(p.time - trim.startMs);
  document.querySelector("#stat-utc")!.textContent = formatClock(p.time);
  summaryEl.textContent = `${(stats.distanceM / 1000).toFixed(1)} km · ${formatDuration(stats.durationMs)} · ${Math.round(stats.maxAlt)} m max`;
  hud.hidden = false;
  layoutHud();
  drawProfile();
}

function layoutHud(): void {
  if (hud.hidden) {
    hudToggle.hidden = true;
    document.body.classList.remove("hud-clipped", "hud-rows-1", "hud-rows-2", "hud-open");
    hudToggle.setAttribute("aria-expanded", "false");
    return;
  }

  const expanded = document.body.classList.contains("hud-open");
  document.body.classList.remove("hud-clipped", "hud-rows-1", "hud-rows-2");

  const cell = hud.querySelector(".hud-stat");
  if (!cell) return;

  const row = cell.getBoundingClientRect().height;
  const gap = Number.parseFloat(getComputedStyle(hud).rowGap) || 8;
  const available = transport.getBoundingClientRect().top - hud.getBoundingClientRect().top - 44;
  const rowsFit = Math.max(0, Math.floor((available + gap) / (row + gap)));

  if (rowsFit >= 3) {
    hudToggle.hidden = true;
    document.body.classList.remove("hud-open");
    hudToggle.setAttribute("aria-expanded", "false");
    return;
  }

  hudToggle.hidden = false;
  document.body.classList.add("hud-clipped");
  document.body.classList.add(rowsFit <= 1 ? "hud-rows-1" : "hud-rows-2");
  if (expanded) document.body.classList.add("hud-open");
  hudToggle.setAttribute("aria-expanded", String(expanded));
}

function hitHandle(x: number, width: number): "start" | "end" | null {
  if (!flight || !trim) return null;
  const pad = 10;
  const startX = xForTime(trim.startMs, width, pad);
  const endX = xForTime(trim.endMs, width, pad);
  if (Math.abs(x - startX) <= HANDLE) return "start";
  if (Math.abs(x - endX) <= HANDLE) return "end";
  return null;
}

async function boot(): Promise<void> {
  setStatus("Preparing 3D globe…");
  const viewer = await createFlightViewer(document.getElementById("cesiumContainer")!);

  applyTrim = async (fly = true) => {
    if (!flight || !trim) return;
    trim = clampTrim(flight, trim);
    cursorMs = Math.min(Math.max(cursorMs, trim.startMs), trim.endMs);
    await viewer.setTrim(trim, fly);
    viewer.setSample(sampleAt(flight, cursorMs), false);
    renderHud();
  };

  seekPlayhead = (timeMs: number) => {
    if (!flight || !trim) return;
    cursorMs = Math.min(Math.max(timeMs, trim.startMs), trim.endMs);
    playing = false;
    playButton.textContent = "Play";
    viewer.seekSample(sampleAt(flight, cursorMs), follow);
    renderHud();
  };

  viewer.setOnTagPick((timeMs) => seekPlayhead?.(timeMs));

  window.addEventListener("flight-tags", () => viewer.setTags(tags));

  async function loadParsed(next: Flight, text: string, opts?: { trim?: TrimRange; tags?: FlightTag[]; slug?: string }): Promise<void> {
    flight = next;
    trackText = text;
    if (opts?.slug) slugInput.value = opts.slug;
    tags = opts?.tags ?? [];
    trim = clampTrim(next, opts?.trim ?? fullRange(next));
    cursorMs = trim.startMs;
    playing = false;
    playButton.textContent = "Play";
    nameEl.textContent = next.name;
    await viewer.setFlight(next, colorMode, trim);
    viewer.setTags(tags);
    renderTags();
    renderHud();
    setStatus(null);
    viewer.setSample(sampleAt(next, cursorMs), false);
  }

  async function loadText(text: string, fileName: string, opts?: { trim?: TrimRange; tags?: FlightTag[]; slug?: string }): Promise<void> {
    setStatus("Parsing track…");
    await loadParsed(parseFlight(text, fileName), text, opts);
  }

  function tick(now: number): void {
    if (playing && flight && trim) {
      const dt = lastFrame ? now - lastFrame : 16;
      cursorMs = Math.min(trim.endMs, cursorMs + dt * playSpeed);
      if (cursorMs >= trim.endMs) {
        playing = false;
        playButton.textContent = "Play";
        viewer.flyOverview();
      } else {
        viewer.setSample(sampleAt(flight, cursorMs), follow);
      }
      renderHud();
    }
    lastFrame = now;
    requestAnimationFrame(tick);
  }

  playButton.addEventListener("click", () => {
    if (!flight || !trim) return;
    if (cursorMs >= trim.endMs || cursorMs < trim.startMs) cursorMs = trim.startMs;
    playing = !playing;
    playButton.textContent = playing ? "Pause" : "Play";
  });

  followButton.addEventListener("click", () => {
    follow = !follow;
    followButton.setAttribute("aria-pressed", String(follow));
    if (!flight || !trim) return;
    const sample = sampleAt(flight, cursorMs);
    if (follow) {
      viewer.captureFollowFromCamera(sample);
      viewer.setSample(sample, true);
    } else {
      viewer.unlockFollow();
    }
  });

  function setFileOpen(open: boolean): void {
    filePanel.hidden = !open;
    fileButton.setAttribute("aria-expanded", String(open));
    document.body.classList.toggle("file-open", open);
  }

  function setTagsOpen(open: boolean): void {
    tagsPanel.hidden = !open;
    tagsButton.setAttribute("aria-expanded", String(open));
    document.body.classList.toggle("tags-open", open);
    layoutHud();
  }

  function setMoreOpen(open: boolean): void {
    morePanel.hidden = !open;
    moreButton.setAttribute("aria-expanded", String(open));
    document.body.classList.toggle("more-open", open);
    if (open) resizeProfile();
    layoutHud();
  }

  fileButton.addEventListener("click", () => {
    const open = filePanel.hidden;
    setFileOpen(open);
    if (open) {
      setMoreOpen(false);
      setTagsOpen(false);
    }
  });

  tagsButton.addEventListener("click", () => {
    const open = tagsPanel.hidden;
    setTagsOpen(open);
    if (open) {
      setFileOpen(false);
      setMoreOpen(false);
    }
  });

  moreButton.addEventListener("click", () => {
    const open = morePanel.hidden;
    setMoreOpen(open);
    if (open) {
      setFileOpen(false);
      setTagsOpen(false);
    }
  });

  hudToggle.addEventListener("click", () => {
    const open = !document.body.classList.contains("hud-open");
    document.body.classList.toggle("hud-open", open);
    hudToggle.setAttribute("aria-expanded", String(open));
  });

  document.addEventListener("pointerdown", (event) => {
    if (!filePanel.hidden && !fileMenu.contains(event.target as Node)) setFileOpen(false);
  });

  overviewButton.addEventListener("click", () => {
    follow = false;
    followButton.setAttribute("aria-pressed", "false");
    viewer.unlockFollow();
    viewer.flyOverview();
  });

  document.querySelectorAll<HTMLButtonElement>("[data-follow-mode]").forEach((button) => {
    button.addEventListener("click", () => {
      const mode = button.dataset.followMode as "fixed" | "relative";
      viewer.setFollowMode(mode);
      document.querySelectorAll("[data-follow-mode]").forEach((el) => el.classList.toggle("active", el === button));
    });
  });

  speedSelect.addEventListener("change", () => {
    playSpeed = Number(speedSelect.value);
  });

  skipButton.addEventListener("click", async () => {
    if (!flight) return;
    trim = suggestTrim(flight);
    await applyTrim?.(true);
  });

  startHereButton.addEventListener("click", async () => {
    if (!flight || !trim) return;
    trim = clampTrim(flight, { startMs: cursorMs, endMs: trim.endMs });
    await applyTrim?.(false);
  });

  endHereButton.addEventListener("click", async () => {
    if (!flight || !trim) return;
    trim = clampTrim(flight, { startMs: trim.startMs, endMs: cursorMs });
    await applyTrim?.(false);
  });

  tagButton.addEventListener("click", () => {
    setTagsOpen(true);
    tagLabel.value = "";
    tagDialog.showModal();
    tagLabel.focus();
  });

  tagForm.addEventListener("submit", (event) => {
    const submitter = (event as SubmitEvent).submitter as HTMLButtonElement | null;
    if (submitter?.value === "cancel" || !flight) return;
    const label = tagLabel.value.trim();
    if (!label) return;
    tags = [...tags, { id: newTagId(), timeMs: cursorMs, label }].sort((a, b) => a.timeMs - b.timeMs);
    renderTags();
    drawProfile();
    viewer.setTags(tags);
  });

  logoutButton.addEventListener("click", async () => {
    await logout();
    auth = { authenticated: false, configured: auth.configured };
    syncAuthUi();
  });

  shareButton.addEventListener("click", async () => {
    if (!flight || !trim) return;
    if (!auth.authenticated) {
      window.location.href = loginUrl();
      return;
    }
    const slug = normalizeSlug(slugInput.value);
    if (!isValidSlug(slug)) {
      setStatus("Slug must be lowercase letters, numbers, and dashes.");
      window.setTimeout(() => setStatus(null), 2400);
      return;
    }
    try {
      shareButton.disabled = true;
      const saved = await saveFlight({
        slug,
        name: flight.name,
        fileName: flight.fileName,
        trimStartMs: trim.startMs,
        trimEndMs: trim.endMs,
        tags,
        track: trackText,
      });
      slugInput.value = saved.slug;
      const url = shareUrl(saved.slug);
      history.replaceState({ slug: saved.slug }, "", `/${saved.slug}`);
      await navigator.clipboard.writeText(url);
      setStatus(`Copied ${url}`);
      window.setTimeout(() => setStatus(null), 2400);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Could not save");
    } finally {
      shareButton.disabled = false;
    }
  });

  document.querySelectorAll<HTMLButtonElement>("[data-mode]").forEach((button) => {
    button.addEventListener("click", async () => {
      colorMode = button.dataset.mode as ColorMode;
      document.querySelectorAll("[data-mode]").forEach((el) => el.classList.toggle("active", el === button));
      if (flight) await viewer.setColorMode(colorMode);
    });
  });

  fileInput.addEventListener("change", async () => {
    const file = fileInput.files?.[0];
    if (!file) return;
    history.replaceState(null, "", "/");
    try {
      await loadText(await file.text(), file.name);
      setFileOpen(false);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Could not parse track");
    }
  });

  window.addEventListener("dragover", (event) => event.preventDefault());
  window.addEventListener("drop", async (event) => {
    event.preventDefault();
    const file = event.dataTransfer?.files[0];
    if (!file) return;
    history.replaceState(null, "", "/");
    try {
      await loadText(await file.text(), file.name);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Could not parse track");
    }
  });

  profile.addEventListener("pointerdown", (event) => {
    if (!flight || !trim) return;
    const rect = profile.getBoundingClientRect();
    const x = event.clientX - rect.left;
    drag = hitHandle(x, rect.width) ?? "seek";
    profile.setPointerCapture(event.pointerId);
    if (drag === "seek") {
      cursorMs = timeForX(x, rect.width, 10);
      viewer.setSample(sampleAt(flight, cursorMs), follow);
      renderHud();
    }
  });

  profile.addEventListener("pointermove", (event) => {
    if (!drag || !flight || !trim) return;
    const rect = profile.getBoundingClientRect();
    const t = timeForX(event.clientX - rect.left, rect.width, 10);
    if (drag === "start") trim = clampTrim(flight, { startMs: t, endMs: trim.endMs });
    if (drag === "end") trim = clampTrim(flight, { startMs: trim.startMs, endMs: t });
    if (drag === "seek") {
      cursorMs = t;
      viewer.setSample(sampleAt(flight, cursorMs), follow);
    }
    renderHud();
  });

  profile.addEventListener("pointerup", async () => {
    const was = drag;
    drag = null;
    if (was === "start" || was === "end") await applyTrim?.(true);
  });

  window.addEventListener("keydown", (event) => {
    if (event.target instanceof HTMLInputElement) return;
    if (event.code === "Space") {
      event.preventDefault();
      playButton.click();
    }
    if (event.key === "t" || event.key === "T") tagButton.click();
  });

  window.addEventListener("popstate", () => {
    const slug = pathSlug();
    if (slug) void loadFromSlug(slug);
  });

  window.addEventListener("resize", () => {
    resizeProfile();
    layoutHud();
  });
  resizeProfile();
  requestAnimationFrame(tick);

  async function loadFromSlug(slug: string): Promise<boolean> {
    try {
      const stored = await getFlight(slug);
      if (!stored) return false;
      history.replaceState({ slug: stored.slug }, "", `/${stored.slug}`);
      const storedTrim =
        stored.trimStartMs != null && stored.trimEndMs != null
          ? { startMs: stored.trimStartMs, endMs: stored.trimEndMs }
          : undefined;
      await loadText(stored.track, stored.fileName, {
        slug: stored.slug,
        trim: storedTrim,
        tags: stored.tags ?? [],
      });
      nameEl.textContent = stored.name;
      return true;
    } catch {
      return false;
    }
  }

  try {
    auth = await getAuth();
  } catch {
    auth = { authenticated: false, configured: true };
  }
  syncAuthUi();

  try {
    const slug = pathSlug();
    if (slug && (await loadFromSlug(slug))) return;
    if (await loadFromSlug("vuelo-5")) return;
    const res = await fetch(SAMPLE_URL);
    if (!res.ok) throw new Error("Could not load Vuelo5");
    await loadText(await res.text(), "vuelo5.gpx", { slug: "vuelo-5" });
  } catch (error) {
    setStatus(error instanceof Error ? error.message : "Could not load flight");
  }
}

void boot();
