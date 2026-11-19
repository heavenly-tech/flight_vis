import { startClubPage } from "./i18nPage";

await startClubPage({ persistUser: false });
const list = document.getElementById("list");
if (!list) throw new Error("list missing");
const club = new URLSearchParams(location.search).get("club");
const mapUrl = club
  ? `/api/club/map/public?club=${encodeURIComponent(club)}`
  : "/api/club/map/public";
const res = await fetch(mapUrl);
const data = (await res.json()) as { aircraft?: { tail: string; pilotName?: string }[] };
for (const row of data.aircraft ?? []) {
  const li = document.createElement("li");
  const name = row.pilotName ? ` · ${row.pilotName}` : "";
  li.textContent = `${row.tail}${name}`;
  list.appendChild(li);
}