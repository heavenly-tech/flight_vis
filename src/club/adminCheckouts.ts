import { apiErrorMessage, t } from "../i18n/index";
import { ensureClubAdminOrRedirect } from "./adminGate";
import {
  api,
  closeDialog,
  extractApiError,
  isClubAdminRedirect,
  mountAdminNav,
  openDialog,
  requireAdmin,
} from "./adminCommon";
import { startClubPage } from "./i18nPage";
import { renderTextWithLink } from "./inlineLink";
import { formatSantiagoDate, parseSantiagoDateToExpiryMs } from "./santiagoDate";

interface PilotItem {
  id: string;
  name: string;
  email: string;
  status?: string;
}

interface AircraftTypeItem {
  id: string;
  code: string;
}

interface AircraftItem {
  id: string;
  tail: string;
  type_code: string;
  active: boolean;
}

interface CheckoutItem {
  id: string;
  pilot_id: string;
  pilot_name: string;
  pilot_email: string;
  type_code: string;
  tail: string | null;
  expires_at: number | null;
}

export interface AdminCheckoutsController {
  refresh: () => Promise<void>;
}

export function mountAdminCheckouts(): AdminCheckoutsController {
  const list = document.querySelector<HTMLUListElement>("#list")!;
  const form = document.querySelector<HTMLFormElement>("#form")!;
  const pilotSelect = document.querySelector<HTMLSelectElement>("#pilot")!;
  const typeSelect = document.querySelector<HTMLSelectElement>("#type")!;
  const tailSelect = document.querySelector<HTMLSelectElement>("#tail")!;
  const expiresInput = document.querySelector<HTMLInputElement>("#expires_at")!;
  const pendingWarningEl = document.querySelector<HTMLParagraphElement>("#pending-pilot-warning");
  const statusEl = document.querySelector<HTMLParagraphElement>("#checkouts-status")!;
  const revokeDialog = document.querySelector<HTMLDialogElement>("#revoke-dialog")!;
  const revokeBody = document.querySelector<HTMLParagraphElement>("#revoke-dialog-body")!;
  const revokeCancel = document.querySelector<HTMLButtonElement>("#revoke-cancel")!;
  const revokeConfirm = document.querySelector<HTMLButtonElement>("#revoke-confirm")!;

  let allPilots: PilotItem[] = [];
  let allAircraft: AircraftItem[] = [];
  let pendingRevoke: CheckoutItem | null = null;
  const initialQueryPilot =
    typeof window !== "undefined"
      ? new URLSearchParams(window.location.search).get("pilot")?.trim() ?? ""
      : "";
  let appliedQueryPilot = false;

  function clearStatus(): void {
    statusEl.hidden = true;
    statusEl.textContent = "";
    statusEl.classList.remove("status-error", "status-ok");
    statusEl.removeAttribute("role");
    statusEl.setAttribute("aria-live", "polite");
  }

  function showError(message: string): void {
    statusEl.hidden = false;
    statusEl.textContent = message;
    statusEl.classList.remove("status-ok");
    statusEl.classList.add("status-error");
    statusEl.setAttribute("role", "alert");
    statusEl.setAttribute("aria-live", "assertive");
  }

  function updatePendingPilotWarning(): void {
    if (!pendingWarningEl) return;
    const selected = allPilots.find((p) => p.id === pilotSelect.value);
    const isPending = Boolean(selected && (selected.status ?? "").toLowerCase() === "pending");
    if (isPending) {
      renderTextWithLink(
        pendingWarningEl,
        t("admin.checkouts.pendingPilotWarning"),
        "/club-admin-requests.html",
        t("nav.requests"),
      );
      pendingWarningEl.hidden = false;
    } else {
      pendingWarningEl.hidden = true;
      pendingWarningEl.replaceChildren();
    }
  }

  function renderTailOptions(selectedType: string, preserveTail: string): void {
    const filtered = selectedType
      ? allAircraft.filter((a) => a.type_code === selectedType)
      : allAircraft;
    const allOpt = document.createElement("option");
    allOpt.value = "";
    allOpt.textContent = t("admin.checkouts.allTails");

    const options = filtered.map((a) => {
      const opt = document.createElement("option");
      opt.value = a.tail;
      const suffix = selectedType ? "" : ` (${a.type_code})`;
      const inactive = a.active ? "" : ` · ${t("admin.checkouts.inactiveTailTag")}`;
      opt.textContent = `${a.tail}${suffix}${inactive}`;
      return opt;
    });
    tailSelect.replaceChildren(allOpt, ...options);
    if (preserveTail && filtered.some((a) => a.tail === preserveTail)) {
      tailSelect.value = preserveTail;
    } else {
      tailSelect.value = "";
    }
  }

  pilotSelect.addEventListener("change", () => {
    updatePendingPilotWarning();
  });

  typeSelect.addEventListener("change", () => {
    renderTailOptions(typeSelect.value, tailSelect.value);
  });

  tailSelect.addEventListener("change", () => {
    const picked = allAircraft.find((a) => a.tail === tailSelect.value);
    if (picked && !typeSelect.value) {
      typeSelect.value = picked.type_code;
      renderTailOptions(picked.type_code, picked.tail);
    }
  });

  function openRevokeDialog(c: CheckoutItem): void {
    pendingRevoke = c;
    const tailText = c.tail ?? t("admin.checkouts.allTails");
    revokeBody.textContent = t("admin.checkouts.revokeBody", {
      pilot: c.pilot_name,
      type: c.type_code,
      tail: tailText,
    });
    openDialog(revokeDialog);
  }

  revokeCancel.addEventListener("click", () => {
    pendingRevoke = null;
    closeDialog(revokeDialog);
  });

  revokeDialog.addEventListener("cancel", () => {
    pendingRevoke = null;
  });

  revokeConfirm.addEventListener("click", async () => {
    const target = pendingRevoke;
    pendingRevoke = null;
    closeDialog(revokeDialog);
    if (!target) return;
    clearStatus();
    try {
      const res = await fetch(`/api/admin/checkouts/${encodeURIComponent(target.id)}`, {
        method: "DELETE",
        credentials: "same-origin",
      });
      if (!res.ok) {
        const raw = await extractApiError(res);
        showError(raw ? apiErrorMessage(raw) : t("admin.checkouts.revokeFailed"));
        return;
      }
      await refresh();
    } catch {
      showError(t("admin.checkouts.revokeFailed"));
    }
  });

  async function refresh(): Promise<void> {
    const prevPilot = pilotSelect.value;
    const prevType = typeSelect.value;
    const prevTail = tailSelect.value;

    const [roster, typesData, aircraftData, checkoutsData] = await Promise.all([
      api<{ pilots: PilotItem[] }>("/api/admin/pilots"),
      api<{ types: AircraftTypeItem[] }>("/api/admin/aircraft-types"),
      api<{ aircraft: AircraftItem[] }>("/api/admin/aircraft"),
      api<{ checkouts: CheckoutItem[] }>("/api/admin/checkouts"),
    ]);

    allPilots = roster.pilots;

    const pilotPlaceholder = document.createElement("option");
    pilotPlaceholder.value = "";
    pilotPlaceholder.textContent = t("admin.checkouts.pilotPlaceholder");
    pilotSelect.replaceChildren(
      pilotPlaceholder,
      ...roster.pilots.map((p) => {
        const opt = document.createElement("option");
        opt.value = p.id;
        const isPending = (p.status ?? "").toLowerCase() === "pending";
        const pendingSuffix = isPending ? ` · ${t("admin.checkouts.pendingPilotTag")}` : "";
        opt.textContent = `${p.name} (${p.email})${pendingSuffix}`;
        return opt;
      }),
    );
    if (prevPilot && roster.pilots.some((p) => p.id === prevPilot)) {
      pilotSelect.value = prevPilot;
    } else if (!appliedQueryPilot && initialQueryPilot) {
      const matched = roster.pilots.find((p) => p.id === initialQueryPilot);
      if (matched) {
        pilotSelect.value = matched.id;
      }
    }
    appliedQueryPilot = true;
    updatePendingPilotWarning();

    const typePlaceholder = document.createElement("option");
    typePlaceholder.value = "";
    typePlaceholder.textContent = t("admin.checkouts.typePlaceholder");
    typeSelect.replaceChildren(
      typePlaceholder,
      ...typesData.types.map((tp) => {
        const opt = document.createElement("option");
        opt.value = tp.code;
        opt.textContent = tp.code;
        return opt;
      }),
    );
    if (prevType && typesData.types.some((tp) => tp.code === prevType)) {
      typeSelect.value = prevType;
    }

    allAircraft = aircraftData.aircraft;
    renderTailOptions(typeSelect.value, prevTail);

    if (checkoutsData.checkouts.length === 0) {
      const empty = document.createElement("li");
      empty.className = "muted";
      empty.textContent = t("admin.checkouts.empty");
      list.replaceChildren(empty);
      return;
    }

    list.replaceChildren(
      ...checkoutsData.checkouts.map((c) => {
        const li = document.createElement("li");
        li.className = "checkout-row";
        li.dataset.checkoutId = c.id;
        const exp =
          c.expires_at != null
            ? formatSantiagoDate(c.expires_at)
            : t("admin.checkouts.noExpiry");
        const tailText = c.tail ?? t("admin.checkouts.allTails");

        const span = document.createElement("span");
        span.className = "checkout-text";
        span.textContent = `${c.pilot_name} · ${c.type_code} · ${tailText} · ${exp}`;

        const revokeBtn = document.createElement("button");
        revokeBtn.type = "button";
        revokeBtn.className = "btn-revoke";
        revokeBtn.textContent = t("admin.checkouts.revoke");
        revokeBtn.addEventListener("click", () => openRevokeDialog(c));

        li.append(span, revokeBtn);
        return li;
      }),
    );
  }

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    clearStatus();
    const expiresRaw = expiresInput.value.trim();
    const expires_at = expiresRaw ? parseSantiagoDateToExpiryMs(expiresRaw) : undefined;
    try {
      const res = await fetch("/api/admin/checkouts", {
        method: "POST",
        credentials: "same-origin",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          pilot_id: pilotSelect.value,
          type: typeSelect.value.trim(),
          tail: tailSelect.value.trim() || undefined,
          expires_at,
        }),
      });
      if (!res.ok) {
        const raw = await extractApiError(res);
        showError(raw ? apiErrorMessage(raw) : t("admin.checkouts.saveFailed"));
        return;
      }
      tailSelect.value = "";
      expiresInput.value = "";
      await refresh();
    } catch {
      showError(t("admin.checkouts.saveFailed"));
    }
  });

  return { refresh };
}

if (typeof document !== "undefined" && document.querySelector('[data-page="admin-checkouts"]')) {
  void (async () => {
    if (!(await ensureClubAdminOrRedirect())) return;
    await startClubPage({ switcher: "#lang-slot" });
    mountAdminNav();
    const controller = mountAdminCheckouts();
    try {
      await requireAdmin();
      await controller.refresh();
    } catch (e) {
      if (!isClubAdminRedirect(e)) {
        const list = document.querySelector<HTMLUListElement>("#list");
        if (list) {
          const li = document.createElement("li");
          li.textContent = t("admin.adminRequired");
          list.replaceChildren(li);
        }
      }
    }
  })();
}
