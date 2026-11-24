import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { applyI18nDocument, initI18n, t } from "../i18n/index";
import { mountAdminNav, resetAdminNavStateForTests } from "./adminCommon";
import { mountAdminPilots } from "./adminPilots";
import { mountAdminRequests } from "./adminRequests";
import { mountAdminTails } from "./adminTails";
import { mountAdminTypes } from "./adminTypes";

describe("admin consoles (requests, pilots, tails, types)", () => {
  beforeEach(() => {
    initI18n({ stored: "es" });
    resetAdminNavStateForTests();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
  });

  it("matches admin nav badge to Solicitudes pending access requests count and rejects via in-page dialog", async () => {
    document.body.innerHTML = `
      <main>
        <h1><span data-i18n="admin.requests.title">Solicitudes de acceso</span> <span id="badge" class="badge" hidden>0</span></h1>
        <div id="admin-nav"></div>
        <p id="requests-status" aria-live="polite" hidden></p>
        <div id="list"></div>
        <dialog id="reject-dialog">
          <form id="reject-form">
            <h2 id="reject-dialog-title" data-i18n="admin.requests.rejectTitle">Rechazar solicitud</h2>
            <p id="reject-dialog-summary"></p>
            <textarea id="reject-reason" name="reason"></textarea>
            <button type="button" id="reject-cancel" data-i18n="common.cancel">Cancelar</button>
            <button type="submit" id="reject-confirm" data-i18n="admin.reject">Rechazar</button>
          </form>
        </dialog>
      </main>
    `;
    applyI18nDocument();

    let requests = [
      {
        id: "req-1",
        pilotName: "Diego Rojas",
        pilotEmail: "diego@club.cl",
        requestedTail: "CC-AAA",
        typeCode: "C152",
        newTypeName: null,
        status: "pending",
      },
      {
        id: "req-2",
        pilotName: "Sofía Díaz",
        pilotEmail: "sofia@club.cl",
        requestedTail: "CC-BBB",
        typeCode: "DA20",
        newTypeName: null,
        status: "pending",
      },
    ];
    const rejected: Array<{ id: string; reason: string }> = [];
    const promptSpy = vi.fn();
    vi.stubGlobal("prompt", promptSpy);

    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === "/api/admin/pending-counts") {
        // Pending pilots (4) MUST NOT inflate the Solicitudes badge!
        return new Response(
          JSON.stringify({ pilots: 4, accessRequests: requests.length }),
          { status: 200 },
        );
      }
      if (url === "/api/admin/access-requests?status=pending") {
        return new Response(JSON.stringify({ requests }), { status: 200 });
      }
      if (url.endsWith("/reject") && init?.method === "POST") {
        const id = decodeURIComponent(url.split("/")[4] ?? "");
        const body = JSON.parse(String(init.body ?? "{}")) as { reason: string };
        rejected.push({ id, reason: body.reason });
        requests = requests.filter((r) => r.id !== id);
        return new Response(JSON.stringify({ ok: true }), { status: 200 });
      }
      return new Response("{}", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    mountAdminNav();
    const ctrl = mountAdminRequests();
    await ctrl.refresh();

    const pageBadge = document.querySelector<HTMLSpanElement>("#badge")!;
    const navBadge = document.querySelector<HTMLSpanElement>("#admin-nav-requests-badge")!;
    expect(document.querySelectorAll("#list article")).toHaveLength(2);
    expect(pageBadge.hidden).toBe(false);
    expect(pageBadge.textContent).toBe("2");
    expect(navBadge.hidden).toBe(false);
    expect(navBadge.textContent).toBe("2");
    expect(document.querySelector("#list")?.textContent).toContain("Matrícula CC-AAA · tipo C152");

    // Open reject dialog -> cancel does not reject or call window.prompt
    const dialog = document.querySelector<HTMLDialogElement>("#reject-dialog")!;
    const rejectBtn = document.querySelector<HTMLButtonElement>('article[data-request-id="req-1"] .btn-reject')!;
    rejectBtn.click();
    expect(promptSpy).not.toHaveBeenCalled();
    expect(dialog.hasAttribute("open")).toBe(true);
    document.querySelector<HTMLButtonElement>("#reject-cancel")!.click();
    expect(dialog.hasAttribute("open")).toBe(false);
    expect(rejected).toEqual([]);

    // Open again, enter reason, submit -> POSTs reason and updates both badges to 1
    rejectBtn.click();
    document.querySelector<HTMLTextAreaElement>("#reject-reason")!.value = "Falta chequeo anual";
    document
      .querySelector<HTMLFormElement>("#reject-form")!
      .dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));

    await vi.waitFor(() =>
      expect(rejected).toEqual([{ id: "req-1", reason: "Falta chequeo anual" }]),
    );
    await vi.waitFor(() => expect(document.querySelectorAll("#list article")).toHaveLength(1));
    expect(pageBadge.textContent).toBe("1");
    expect(navBadge.textContent).toBe("1");
  });

  it("shows invite errors as role=alert and requires dialog confirmation for Quitar acceso", async () => {
    document.body.innerHTML = `
      <main>
        <div id="admin-nav"></div>
        <form id="invite-form">
          <input id="invite-email" type="email" />
          <button type="submit">Agregar administrador</button>
        </form>
        <p id="invite-status" role="status" aria-live="polite"></p>
        <ul id="pending"></ul>
        <p id="memberships-status" aria-live="polite" hidden></p>
        <div id="memberships"></div>
        <dialog id="remove-dialog">
          <h2 id="remove-dialog-title">Quitar acceso</h2>
          <p id="remove-dialog-body"></p>
          <button type="button" id="remove-cancel">Cancelar</button>
          <button type="button" id="remove-confirm">Quitar acceso</button>
        </dialog>
      </main>
    `;

    let memberships = [
      {
        id: "mem-1",
        name: "Carlos Soto",
        email: "carlos@club.cl",
        status: "active",
        role: "pilot",
      },
      {
        id: "mem-2",
        name: "Laura Vidal",
        email: "laura@club.cl",
        status: "suspended",
        role: "admin",
      },
    ];
    const removedIds: string[] = [];

    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === "/api/admin/pilots/pending") {
        return new Response(JSON.stringify({ pilots: [] }), { status: 200 });
      }
      if (url === "/api/admin/memberships") {
        return new Response(JSON.stringify({ memberships }), { status: 200 });
      }
      if (url === "/api/admin/memberships/invite" && init?.method === "POST") {
        return new Response(JSON.stringify({ error: "db error" }), { status: 500 });
      }
      if (url.endsWith("/remove") && init?.method === "POST") {
        const id = decodeURIComponent(url.split("/")[4] ?? "");
        removedIds.push(id);
        memberships = memberships.filter((m) => m.id !== id);
        return new Response(JSON.stringify({ ok: true }), { status: 200 });
      }
      return new Response("{}", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const ctrl = mountAdminPilots();
    await ctrl.refresh();

    // Statuses are localized in es-CL (Activo / Suspendido, never raw English).
    const rows = document.querySelectorAll<HTMLDivElement>(".mem-row");
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("Piloto · Activo");
    expect(rows[0].textContent).not.toContain("active");
    expect(rows[1].textContent).toContain("Administrador · Suspendido");

    // Invite error is shown with role="alert" and error class, not success copy.
    const emailInput = document.querySelector<HTMLInputElement>("#invite-email")!;
    const inviteStatus = document.querySelector<HTMLParagraphElement>("#invite-status")!;
    emailInput.value = "nuevo@club.cl";
    document
      .querySelector<HTMLFormElement>("#invite-form")!
      .dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));
    await vi.waitFor(() => expect(inviteStatus.getAttribute("role")).toBe("alert"));
    expect(inviteStatus.classList.contains("status-error")).toBe(true);
    expect(inviteStatus.textContent).not.toBe(t("club.admin.inviteSent"));
    expect(inviteStatus.textContent).toBe(t("api.dbError"));
    expect(emailInput.value).toBe("nuevo@club.cl");

    // Quitar acceso opens confirmation dialog -> cancel keeps membership -> confirm removes it.
    const removeDialog = document.querySelector<HTMLDialogElement>("#remove-dialog")!;
    const removeBtn = document.querySelector<HTMLButtonElement>(
      '.mem-row[data-membership-id="mem-1"] .btn-remove-access',
    )!;
    removeBtn.click();
    expect(removeDialog.hasAttribute("open")).toBe(true);
    expect(removedIds).toEqual([]);
    expect(document.querySelector("#remove-dialog-body")?.textContent).toContain("Carlos Soto");

    document.querySelector<HTMLButtonElement>("#remove-cancel")!.click();
    expect(removeDialog.hasAttribute("open")).toBe(false);
    expect(removedIds).toEqual([]);

    removeBtn.click();
    document.querySelector<HTMLButtonElement>("#remove-confirm")!.click();
    await vi.waitFor(() => expect(removedIds).toEqual(["mem-1"]));
    await vi.waitFor(() => expect(document.querySelectorAll(".mem-row")).toHaveLength(1));
  });

  it("renders tails and types in es-CL with localized active/inactive labels", async () => {
    document.body.innerHTML = `
      <form id="form">
        <input name="tail" />
        <input name="type" />
        <button type="submit">Guardar matrícula</button>
      </form>
      <p id="tails-status" hidden></p>
      <ul id="list"></ul>
    `;

    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        if (String(input) === "/api/admin/aircraft") {
          return new Response(
            JSON.stringify({
              aircraft: [
                { id: "a-1", tail: "CC-AAA", type_code: "C152", active: true },
                { id: "a-2", tail: "CC-BBB", type_code: "DA20", active: false },
              ],
            }),
            { status: 200 },
          );
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const tailsCtrl = mountAdminTails();
    await tailsCtrl.refresh();
    const tailItems = document.querySelectorAll("#list li");
    expect(tailItems).toHaveLength(2);
    expect(tailItems[0].textContent).toContain("CC-AAA · C152 · Activa");
    expect(tailItems[0].querySelector("button")?.textContent).toBe("Desactivar");
    expect(tailItems[1].textContent).toContain("CC-BBB · DA20 · Inactiva");
    expect(tailItems[1].querySelector("button")?.textContent).toBe("Activar");

    document.body.innerHTML = `
      <form id="form">
        <input id="code" />
        <button type="submit">Guardar tipo</button>
      </form>
      <p id="types-status" hidden></p>
      <ul id="list"></ul>
    `;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        if (String(input) === "/api/admin/aircraft-types") {
          return new Response(JSON.stringify({ types: [] }), { status: 200 });
        }
        return new Response("{}", { status: 404 });
      }),
    );
    const typesCtrl = mountAdminTypes();
    await typesCtrl.refresh();
    expect(document.querySelector("#list")?.textContent).toBe(t("admin.types.empty"));
  });
});
