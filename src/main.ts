import "cesium/Build/Cesium/Widgets/widgets.css";
import "./style.css";
import { formatClock, formatDuration } from "./track/geo";
import { parseFlight, sampleAt } from "./track/parse";
import type { ColorMode, Flight } from "./track/types";
import { cssForUnit } from "./viz/colors";
import { createFlightViewer } from "./viz/viewer";

const SAMPLE_URL = `${import.meta.env.BASE_URL}samples/vuelo5.gpx`;

const fileInput = document.querySelector<HTMLInputElement>("#file-input")!;
const playButton = document.querySelector<HTMLButtonElement>("#btn-play")!;
const followButton = document.querySelector<HTMLButtonElement>("#btn-follow")!;
const overviewButton = document.querySelector<HTMLButtonElement>("#btn-overview")!;
const speedSelect = document.querySelector<HTMLSelectElement>("#play-speed")!;
const profile = document.querySelector<HTMLCanvasElement>("#profile")!;
const statusEl = document.querySelector<HTMLParagraphElement>("#status")!;
const hud = document.querySelector<HTMLElement>("#hud")!;
const nameEl = document.querySelector<HTMLParagraphElement>("#flight-name")!;
const summaryEl = document.querySelector<HTMLParagraphElement>("#summary")!;

const ctx = profile.getContext("2d")!;

let flight: Flight | null = null;
let playing = false;
let follow = true;
let colorMode: ColorMode = "altitude";
let cursorMs = 0;
let lastFrame = 0;
let playSpeed = Number(speedSelect.value);

function setStatus(text: string | null): void {
  if (!text) {
    statusEl.hidden = true;
    statusEl.textContent = "";
    return;
  }
  statusEl.hidden = false;
  statusEl.textContent = text;
}

function resizeProfile(): void {
  const ratio = window.devicePixelRatio || 1;
  const width = profile.clientWidth;
  const height = 88;
  profile.width = Math.max(1, Math.floor(width * ratio));
  profile.height = Math.floor(height * ratio);
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  drawProfile();
}

function drawProfile(): void {
  const width = profile.clientWidth;
  const height = 88;
  ctx.clearRect(0, 0, width, height);
  if (!flight) return;

  const pts = flight.points;
  const pad = 8;
  const span = Math.max(1, flight.maxAlt - flight.minAlt);
  ctx.beginPath();
  pts.forEach((p, i) => {
    const x = pad + ((p.time - pts[0].time) / flight!.durationMs) * (width - pad * 2);
    const y = height - pad - ((p.alt - flight!.minAlt) / span) * (height - pad * 2);
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  });
  ctx.lineTo(width - pad, height - pad);
  ctx.lineTo(pad, height - pad);
  ctx.closePath();
  ctx.fillStyle = "rgba(103, 232, 249, 0.12)";
  ctx.fill();

  ctx.beginPath();
  pts.forEach((p, i) => {
    const x = pad + ((p.time - pts[0].time) / flight!.durationMs) * (width - pad * 2);
    const y = height - pad - ((p.alt - flight!.minAlt) / span) * (height - pad * 2);
    if (i === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  });
  ctx.strokeStyle = cssForUnit(0.55);
  ctx.lineWidth = 2;
  ctx.stroke();

  const u = (cursorMs - pts[0].time) / flight.durationMs;
  const x = pad + u * (width - pad * 2);
  ctx.fillStyle = "#fff";
  ctx.fillRect(x - 1, pad / 2, 2, height - pad);
}

function renderHud(): void {
  if (!flight) return;
  const sample = sampleAt(flight, cursorMs);
  const p = sample.point;
  document.querySelector("#stat-alt")!.textContent = `${Math.round(p.alt)} m`;
  document.querySelector("#stat-spd")!.textContent = `${(p.speedMps * 3.6).toFixed(0)} km/h`;
  document.querySelector("#stat-vario")!.textContent = `${p.varioMps >= 0 ? "+" : ""}${p.varioMps.toFixed(1)} m/s`;
  document.querySelector("#stat-dist")!.textContent = `${(p.distanceM / 1000).toFixed(1)} km`;
  document.querySelector("#stat-time")!.textContent = formatDuration(p.time - flight.points[0].time);
  document.querySelector("#stat-utc")!.textContent = formatClock(p.time);
  hud.hidden = false;
  drawProfile();
}

async function boot(): Promise<void> {
  setStatus("Preparing 3D globe…");
  const viewer = await createFlightViewer(document.getElementById("cesiumContainer")!);

  async function loadText(text: string, fileName: string): Promise<void> {
    setStatus("Parsing track…");
    const next = parseFlight(text, fileName);
    flight = next;
    cursorMs = next.points[0].time;
    playing = false;
    playButton.textContent = "Play";
    nameEl.textContent = next.name;
    summaryEl.textContent = `${(next.distanceM / 1000).toFixed(1)} km · ${formatDuration(next.durationMs)} · ${Math.round(next.maxAlt)} m max · ${next.points.length} pts`;
    await viewer.setFlight(next, colorMode);
    renderHud();
    setStatus(null);
    viewer.setSample(sampleAt(next, cursorMs), false);
  }

  async function loadUrl(url: string, fileName: string): Promise<void> {
    const res = await fetch(url);
    if (!res.ok) throw new Error(`Could not load ${fileName}`);
    await loadText(await res.text(), fileName);
  }

  function tick(now: number): void {
    if (playing && flight) {
      const dt = lastFrame ? now - lastFrame : 16;
      cursorMs = Math.min(flight.points[flight.points.length - 1].time, cursorMs + dt * playSpeed);
      if (cursorMs >= flight.points[flight.points.length - 1].time) {
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
    if (!flight) return;
    if (cursorMs >= flight.points[flight.points.length - 1].time) {
      cursorMs = flight.points[0].time;
    }
    playing = !playing;
    playButton.textContent = playing ? "Pause" : "Play";
  });

  followButton.addEventListener("click", () => {
    follow = !follow;
    followButton.setAttribute("aria-pressed", String(follow));
    if (!follow) viewer.flyOverview();
  });

  overviewButton.addEventListener("click", () => {
    follow = false;
    followButton.setAttribute("aria-pressed", "false");
    viewer.flyOverview();
  });

  speedSelect.addEventListener("change", () => {
    playSpeed = Number(speedSelect.value);
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
    try {
      await loadText(await file.text(), file.name);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Could not parse track");
    }
  });

  window.addEventListener("dragover", (event) => event.preventDefault());
  window.addEventListener("drop", async (event) => {
    event.preventDefault();
    const file = event.dataTransfer?.files[0];
    if (!file) return;
    try {
      await loadText(await file.text(), file.name);
    } catch (error) {
      setStatus(error instanceof Error ? error.message : "Could not parse track");
    }
  });

  profile.addEventListener("pointerdown", (event) => {
    if (!flight) return;
    const rect = profile.getBoundingClientRect();
    const u = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
    cursorMs = flight.points[0].time + u * flight.durationMs;
    viewer.setSample(sampleAt(flight, cursorMs), follow);
    renderHud();
  });

  window.addEventListener("keydown", (event) => {
    if (event.code === "Space") {
      event.preventDefault();
      playButton.click();
    }
  });

  window.addEventListener("resize", resizeProfile);
  resizeProfile();
  requestAnimationFrame(tick);

  try {
    await loadUrl(SAMPLE_URL, "vuelo5.gpx");
  } catch (error) {
    setStatus(error instanceof Error ? error.message : "Could not load Vuelo5");
  }
}

void boot();
