export {};

const statusEl = document.querySelector<HTMLParagraphElement>("#status")!;
const qr = document.querySelector<HTMLImageElement>("#qr")!;
const tokenText = document.querySelector<HTMLParagraphElement>("#token-text")!;
const map = document.querySelector<HTMLUListElement>("#map")!;

function qrUrl(text: string): string {
  return `https://api.qrserver.com/v1/create-qr-code/?size=280x280&data=${encodeURIComponent(text)}`;
}

async function activateFromQuery(): Promise<boolean> {
  const params = new URLSearchParams(location.search);
  const token = params.get("token")?.trim();
  if (!token) return false;
  const res = await fetch("/api/display/activate", {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ token }),
  });
  if (!res.ok) {
    statusEl.textContent = "Invalid display token.";
    return false;
  }
  history.replaceState({}, "", "/club/screen");
  return true;
}

async function refreshQr(): Promise<void> {
  const res = await fetch("/api/display/checkin-qr", { credentials: "same-origin" });
  if (!res.ok) {
    statusEl.textContent = "Display session required. Ask an admin to open this screen.";
    return;
  }
  const data = (await res.json()) as { token: string; stepSeconds: number };
  tokenText.textContent = data.token;
  qr.src = qrUrl(data.token);
  statusEl.textContent = `QR rotates every ${data.stepSeconds}s.`;
}

async function refreshMap(): Promise<void> {
  const res = await fetch("/api/display/map", { credentials: "same-origin" });
  if (!res.ok) return;
  const data = (await res.json()) as {
    aircraft: { tail: string; pilotName?: string }[];
  };
  map.replaceChildren(
    ...(data.aircraft ?? []).map((row) => {
      const li = document.createElement("li");
      li.textContent = `${row.tail} · ${row.pilotName ?? "—"}`;
      return li;
    }),
  );
}

void (async () => {
  await activateFromQuery();
  await refreshQr();
  await refreshMap();
  setInterval(() => void refreshQr(), 10_000);
  setInterval(() => void refreshMap(), 5000);
})();