import { startClubPage } from "./i18nPage";
import { t } from "../i18n/boot";

void (async () => {
  await startClubPage({ persistUser: true });
  const list = document.querySelector<HTMLUListElement>("#list");
  if (!list) return;
  const res = await fetch("/api/club/map", { credentials: "same-origin" });
  if (!res.ok) {
    list.innerHTML = `<li>${t("club.map.signInApproved")}</li>`;
    return;
  }
  const data = (await res.json()) as { aircraft?: { tail: string; pilotName?: string }[] };
  list.replaceChildren(
    ...(data.aircraft ?? []).map((row) => {
      const li = document.createElement("li");
      li.textContent = `${row.tail} · ${row.pilotName ?? "—"}`;
      return li;
    }),
  );
})();