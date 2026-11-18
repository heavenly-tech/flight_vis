import { api, mountAdminLangSwitcher, mountAdminNav, requireAdmin } from "./adminCommon";
import { t } from "../i18n/index";

const list = document.querySelector<HTMLUListElement>("#list")!;
const filter = document.querySelector<HTMLSelectElement>("#filter")!;
const more = document.querySelector<HTMLButtonElement>("#more")!;
let offset = 0;

async function load(reset: boolean): Promise<void> {
  if (reset) offset = 0;
  const et = filter.value || undefined;
  const q = new URLSearchParams({ offset: String(offset), limit: "30" });
  if (et) q.set("entity_type", et);
  const data = await api<{
    entries: {
      action: string;
      entity_type: string;
      entity_id: string;
      created_at: number;
      bulk_batch_id: string | null;
    }[];
    total: number;
  }>(`/api/admin/audit?${q}`);
  const items = data.entries.map((e) => {
    const li = document.createElement("li");
    const when = new Date(e.created_at).toISOString();
    const bulk = e.bulk_batch_id ? ` · batch ${e.bulk_batch_id.slice(0, 8)}` : "";
    li.textContent = `${when} · ${e.action} · ${e.entity_type} · ${e.entity_id}${bulk}`;
    return li;
  });
  if (reset) list.replaceChildren(...items);
  else list.append(...items);
  offset += data.entries.length;
  more.hidden = offset >= data.total;
}

filter.addEventListener("change", () => void load(true));
more.addEventListener("click", () => void load(false));

void (async () => {
  mountAdminNav();
  mountAdminLangSwitcher();
  try {
    await requireAdmin();
    await load(true);
  } catch {
    list.innerHTML = `<li>${t("api.adminOnly")}</li>`;
  }
})();