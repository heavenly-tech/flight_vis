const who = document.querySelector<HTMLParagraphElement>("#who")!;
const tokenReveal = document.querySelector<HTMLDivElement>("#token-reveal")!;
const tokenText = document.querySelector<HTMLDivElement>("#token-text")!;
const qr = document.querySelector<HTMLImageElement>("#qr")!;
const devices = document.querySelector<HTMLUListElement>("#devices")!;
const flights = document.querySelector<HTMLUListElement>("#flights")!;
const labelInput = document.querySelector<HTMLInputElement>("#dev-label")!;
const newDeviceBtn = document.querySelector<HTMLButtonElement>("#btn-new-device")!;

async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(path, { credentials: "same-origin", ...init });
  if (!res.ok) throw new Error(`${res.status}`);
  return (await res.json()) as T;
}

function qrUrl(text: string): string {
  return `https://api.qrserver.com/v1/create-qr-code/?size=180x180&data=${encodeURIComponent(text)}`;
}

async function boot(): Promise<void> {
  const me = await api<{ pilot: { name: string; status: string; role: string } | null }>("/api/club/me");
  if (!me.pilot) {
    who.textContent = "Not signed in. Use /api/club/login from the club login page.";
    return;
  }
  who.textContent = `${me.pilot.name} · ${me.pilot.status} · ${me.pilot.role}`;
  const devs = await api<{ devices: { id: string; label: string }[] }>("/api/me/devices");
  devices.replaceChildren(
    ...devs.devices.map((d) => {
      const li = document.createElement("li");
      li.textContent = d.label || d.id;
      return li;
    }),
  );
  const list = await api<{ flights: { id: string; tail: string; status: string }[] }>("/api/me/flights");
  flights.replaceChildren(
    ...list.flights.map((f) => {
      const li = document.createElement("li");
      const a = document.createElement("a");
      a.href = `/club-flight.html?id=${encodeURIComponent(f.id)}`;
      a.textContent = `${f.tail} · ${f.status}`;
      li.append(a, document.createTextNode(" "));
      const gpx = document.createElement("a");
      gpx.href = `/api/flights/${f.id}/gpx`;
      gpx.textContent = "GPX";
      li.append(gpx);
      return li;
    }),
  );
}

newDeviceBtn.addEventListener("click", async () => {
  const body = await api<{ token: string }>("/api/me/devices", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ label: labelInput.value.trim() }),
  });
  tokenText.textContent = body.token;
  qr.src = qrUrl(body.token);
  tokenReveal.hidden = false;
  await boot();
});

void boot();