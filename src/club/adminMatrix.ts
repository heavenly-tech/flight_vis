import { api, mountAdminLangSwitcher, mountAdminNav, requireAdmin } from "./adminCommon";
import { t } from "../i18n/index";

const root = document.querySelector<HTMLDivElement>("#matrix")!;
const bulkBar = document.querySelector<HTMLDivElement>("#bulk")!;
const typeSel = document.querySelector<HTMLSelectElement>("#bulk-type")!;
const tailInp = document.querySelector<HTMLInputElement>("#bulk-tail")!;
const selectedPilots = new Set<string>();

type Matrix = {
  pilots: { id: string; name: string; email: string }[];
  types: { id: string; code: string }[];
  aircraft: { id: string; tail: string; type_id: string; type_code: string; active: boolean }[];
  type_checkouts: Record<string, { id: string; expires_at: number | null }>;
  tail_checkouts: Record<string, { id: string; expires_at: number | null }>;
};

async function load(): Promise<void> {
  const m = await api<Matrix>("/api/admin/matrix");
  typeSel.replaceChildren(
    ...m.types.map((ty) => {
      const o = document.createElement("option");
      o.value = ty.code;
      o.textContent = ty.code;
      return o;
    }),
  );
  const table = document.createElement("table");
  table.style.width = "100%";
  table.style.borderCollapse = "collapse";
  table.style.fontSize = "0.85rem";
  const head = document.createElement("tr");
  head.appendChild(document.createElement("th"));
  for (const ty of m.types) {
    const th = document.createElement("th");
    th.textContent = ty.code;
    head.appendChild(th);
  }
  table.appendChild(head);
  for (const p of m.pilots) {
    const tr = document.createElement("tr");
    const label = document.createElement("td");
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.addEventListener("change", () => {
      if (cb.checked) selectedPilots.add(p.id);
      else selectedPilots.delete(p.id);
    });
    label.append(cb, ` ${p.name}`);
    tr.appendChild(label);
    for (const ty of m.types) {
      const td = document.createElement("td");
      const key = `${p.id}:${ty.id}`;
      const on = Boolean(m.type_checkouts[key]);
      const btn = document.createElement("button");
      btn.type = "button";
      btn.textContent = on ? "✓" : "·";
      btn.title = t("admin.roster.toggleAuth");
      btn.addEventListener("click", async () => {
        await api("/api/admin/checkouts/toggle", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            pilot_id: p.id,
            type: ty.code,
            enable: !on,
          }),
        });
        await load();
      });
      td.appendChild(btn);
      tr.appendChild(td);
    }
    table.appendChild(tr);
  }
  root.replaceChildren(table);
}

bulkBar.querySelector("#bulk-assign")?.addEventListener("click", async () => {
  if (selectedPilots.size === 0) return;
  await api("/api/admin/checkouts/bulk", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      pilot_ids: [...selectedPilots],
      type: typeSel.value,
      tail: tailInp.value.trim() || undefined,
      assign: true,
    }),
  });
  await load();
});

bulkBar.querySelector("#bulk-unassign")?.addEventListener("click", async () => {
  if (selectedPilots.size === 0) return;
  await api("/api/admin/checkouts/bulk", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      pilot_ids: [...selectedPilots],
      type: typeSel.value,
      tail: tailInp.value.trim() || undefined,
      assign: false,
    }),
  });
  await load();
});

void (async () => {
  mountAdminNav();
  mountAdminLangSwitcher();
  try {
    await requireAdmin();
    await load();
  } catch {
    root.textContent = t("api.adminOnly");
  }
})();