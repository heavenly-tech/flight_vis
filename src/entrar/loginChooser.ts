import { applyI18nDocument, bootI18n } from "../i18n/boot";
import { mountLangSwitcher } from "../i18n/index";
import { safeNextPath } from "../club/safeNext";

export async function bootLoginChooser(): Promise<void> {
  await bootI18n({ persistUser: false });
  applyI18nDocument(document);
  const lang = document.getElementById("lang-slot");
  if (lang) mountLangSwitcher(lang, { persistUser: false });

  const next = safeNextPath(new URLSearchParams(location.search).get("next") ?? "/");
  const base = new URLSearchParams({ next });
  const club = document.getElementById("btn-club") as HTMLAnchorElement;
  const pilot = document.getElementById("btn-pilot") as HTMLAnchorElement;
  base.set("mode", "club");
  club.href = `/login?${base}`;
  base.set("mode", "pilot");
  pilot.href = `/login?${base}`;
}