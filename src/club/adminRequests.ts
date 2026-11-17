import { api, mountAdminNav, requireAdmin } from "./adminCommon";

const list = document.querySelector<HTMLDivElement>("#list")!;
const badge = document.querySelector<HTMLSpanElement>("#badge")!;

async function refresh(): Promise<void> {
  const counts = await api<{ pilots: number; accessRequests: number }>("/api/admin/pending-counts");
  const total = counts.pilots + counts.accessRequests;
  badge.hidden = total === 0;
  badge.textContent = String(total);
  const pending = await api<{
    requests: {
      id: string;
      pilotName: string;
      pilotEmail: string;
      requestedTail: string;
      typeCode: string | null;
      newTypeName: string | null;
      status: string;
    }[];
  }>("/api/admin/access-requests?status=pending");
  list.replaceChildren(
    ...pending.requests.map((r) => {
      const art = document.createElement("article");
      const type = r.typeCode ?? r.newTypeName ?? "?";
      art.innerHTML = `<p><strong>${r.pilotName}</strong> (${r.pilotEmail})<br/>
        Tail ${r.requestedTail} · type ${type}</p>`;
      const approve = document.createElement("button");
      approve.textContent = "Approve";
      approve.addEventListener("click", async () => {
        await api(`/api/admin/access-requests/${encodeURIComponent(r.id)}/approve`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: "{}",
        });
        await refresh();
      });
      const reject = document.createElement("button");
      reject.textContent = "Reject";
      reject.addEventListener("click", async () => {
        const reason = prompt("Reason for pilot")?.trim() || "rejected";
        await api(`/api/admin/access-requests/${encodeURIComponent(r.id)}/reject`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ reason }),
        });
        await refresh();
      });
      art.append(approve, reject);
      return art;
    }),
  );
}

void (async () => {
  mountAdminNav();
  try {
    await requireAdmin();
    await refresh();
  } catch {
    list.textContent = "Admin login required.";
  }
})();