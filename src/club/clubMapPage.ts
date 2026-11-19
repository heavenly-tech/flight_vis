import { startClubPage } from "./i18nPage";
import { t } from "../i18n/boot";

await startClubPage();
const list = document.getElementById("list");
if (!list) throw new Error("list missing");
const res = await fetch("/api/club/map", { credentials: "same-origin" });
if (!res.ok) {
  list.innerHTML = `<li>${t("club.map.signInApproved")}</li>`;
} else {
  const data = (await res.json()) as { aircraft?: { tail: string; pilotName?: string }[] };
  for (const row of data.aircraft ?? []) {
    const li = document.createElement("li");
    li.textContent = `${row.tail} · ${row.pilotName ?? "—"}`;
    list.appendChild(li);
  }
}