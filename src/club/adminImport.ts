import { ensureClubAdminOrRedirect } from "./adminGate";
import {
  api,
  isClubAdminRedirect,
  mountAdminLangSwitcher,
  mountAdminNav,
  requireAdmin,
} from "./adminCommon";
import { t } from "../i18n/index";

const kindSel = document.querySelector<HTMLSelectElement>("#kind")!;
const fileInp = document.querySelector<HTMLInputElement>("#file")!;
const previewBox = document.querySelector<HTMLPreElement>("#preview")!;
const commitBtn = document.querySelector<HTMLButtonElement>("#commit")!;
const tplLink = document.querySelector<HTMLAnchorElement>("#template")!;

let batchId: string | null = null;

function syncTemplate(): void {
  const kind = kindSel.value;
  tplLink.href = `/api/admin/import/templates/${kind}`;
  tplLink.textContent = t("admin.roster.downloadTemplate");
}

kindSel.addEventListener("change", () => {
  batchId = null;
  commitBtn.disabled = true;
  previewBox.textContent = "";
  syncTemplate();
});

fileInp.addEventListener("change", async () => {
  const file = fileInp.files?.[0];
  if (!file) return;
  const text = await file.text();
  const kind = kindSel.value;
  const res = await fetch(`/api/admin/import/${kind}/preview`, {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "text/csv" },
    body: text,
  });
  if (!res.ok) {
    previewBox.textContent = `${t("admin.roster.previewFailed")} (${res.status})`;
    return;
  }
  const data = (await res.json()) as {
    batch_id: string;
    can_commit: boolean;
    rows: { line: number; action: string; errors: string[]; summary?: string }[];
  };
  batchId = data.batch_id;
  commitBtn.disabled = !data.can_commit;
  const lines = data.rows.map(
    (r) =>
      `L${r.line} [${r.action}] ${r.summary ?? ""} ${r.errors.length ? "ERR: " + r.errors.join("; ") : ""}`,
  );
  previewBox.textContent = lines.join("\n");
});

commitBtn.addEventListener("click", async () => {
  if (!batchId) return;
  const kind = kindSel.value;
  const out = await api<{ applied: number }>(`/api/admin/import/${kind}/commit`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ batch_id: batchId }),
  });
  previewBox.textContent = `${t("admin.roster.committed")} (${out.applied})`;
  batchId = null;
  commitBtn.disabled = true;
});

void (async () => {
  if (!(await ensureClubAdminOrRedirect())) return;
  mountAdminNav();
  mountAdminLangSwitcher();
  syncTemplate();
  commitBtn.disabled = true;
  try {
    await requireAdmin();
  } catch (e) {
    if (!isClubAdminRedirect(e)) {
      previewBox.textContent = t("api.adminOnly");
    }
  }
})();