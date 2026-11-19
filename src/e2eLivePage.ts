import { connectLiveStream } from "./live/sseClient";

const params = new URLSearchParams(location.search);
const sessionId = params.get("id") ?? "demo-session";
const statusEl = document.getElementById("live-status");
const countEl = document.getElementById("live-count");
if (!statusEl || !countEl) throw new Error("live-status elements missing");
let fixCount = 0;
statusEl.textContent = "reconnecting";
connectLiveStream(sessionId, {
  onFix: () => {
    fixCount += 1;
    countEl.textContent = String(fixCount);
    statusEl.textContent = `fixes:${fixCount}`;
  },
  onEnd: () => {
    statusEl.textContent = "ended";
  },
  onError: () => {
    statusEl.textContent = "error";
  },
});