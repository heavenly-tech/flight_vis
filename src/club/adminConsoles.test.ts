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

  it("shows pending pilots and access requests in Solicitudes, sums both on the Solicitudes badge, and keeps Pilotos badge hidden", async () => {
    document.body.innerHTML = `
      <main>
        <h1><span data-i18n="admin.requests.title">Solicitudes de acceso</span> <span id="badge" class="badge" hidden>0</span></h1>
        <div id="admin-nav"></div>
        <p id="requests-status" aria-live="polite" hidden></p>
        <section id="pending-pilots-section">
          <h2 data-i18n="admin.requests.pendingPilotsTitle">Pilotos pendientes de aprobación</h2>
          <ul id="pending-pilots" class="admin-list"></ul>
        </section>
        <section id="access-requests-section">
          <h2 data-i18n="admin.requests.accessRequestsTitle">Solicitudes de habilitación</h2>
          <div id="list"></div>
        </section>
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

    let pendingPilots = [
      { id: "pilot-p1", name: "Mateo Silva", email: "mateo@club.cl" },
    ];
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
    const approvedPilots: Array<{ id: string; body: string }> = [];
    const rejected: Array<{ id: string; reason: string }> = [];
    const promptSpy = vi.fn();
    vi.stubGlobal("prompt", promptSpy);

    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === "/api/admin/pending-counts") {
        return new Response(
          JSON.stringify({ pilots: pendingPilots.length, accessRequests: requests.length }),
          { status: 200 },
        );
      }
      if (url === "/api/admin/pilots/pending") {
        return new Response(JSON.stringify({ pilots: pendingPilots }), { status: 200 });
      }
      if (url.startsWith("/api/admin/pilots/") && url.endsWith("/approve") && init?.method === "POST") {
        const id = decodeURIComponent(url.split("/")[4] ?? "");
        approvedPilots.push({ id, body: String(init.body ?? "") });
        pendingPilots = pendingPilots.filter((p) => p.id !== id);
        return new Response(JSON.stringify({ ok: true }), { status: 200 });
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
    const pilotsBadge = document.querySelector<HTMLSpanElement>("#admin-nav-pilots-badge")!;
    expect(document.querySelectorAll("#pending-pilots li.pending-pilot-row")).toHaveLength(1);
    expect(document.querySelectorAll("#list article")).toHaveLength(2);
    // Solicitudes badge sums pending pilots (1) + pending access requests (2) = 3.
    expect(pageBadge.hidden).toBe(false);
    expect(pageBadge.textContent).toBe("3");
    expect(navBadge.hidden).toBe(false);
    expect(navBadge.textContent).toBe("3");
    // Pilotos nav badge stays hidden (pending pilots belong to Solicitudes).
    expect(pilotsBadge.hidden).toBe(true);
    expect(document.querySelector("#list")?.textContent).toContain("Matrícula CC-AAA · tipo C152");

    // Approve pending pilot -> POSTs "{}" and updates counts to 2 + shows empty state.
    document
      .querySelector<HTMLButtonElement>('#pending-pilots li[data-pilot-id="pilot-p1"] .btn-approve')!
      .click();
    await vi.waitFor(() =>
      expect(approvedPilots).toEqual([{ id: "pilot-p1", body: "{}" }]),
    );
    await vi.waitFor(() => expect(pageBadge.textContent).toBe("2"));
    expect(navBadge.textContent).toBe("2");
    expect(document.querySelector("#pending-pilots")?.textContent).toBe(
      t("club.admin.noPendingPilots"),
    );

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

  it("invites with role (pilot/admin), renders active rating chips + Habilitar links, excludes pending rows with notice, and confirms Quitar acceso", async () => {
    document.body.innerHTML = `
      <main>
        <div id="admin-nav"></div>
        <form id="invite-form">
          <input id="invite-email" type="email" />
          <select id="invite-role" name="role">
            <option value="pilot" selected>Piloto</option>
            <option value="admin">Administrador</option>
          </select>
          <button type="submit">Enviar invitación</button>
        </form>
        <p id="invite-status" role="status" aria-live="polite"></p>
        <p id="pending-pilots-notice" class="admin-notice" hidden></p>
        <p id="memberships-status" aria-live="polite" hidden></p>
        <div id="memberships" class="admin-list"></div>
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
        pilotId: "p-1",
        name: "Carlos Soto",
        email: "carlos@club.cl",
        status: "active",
        role: "pilot",
      },
      {
        id: "mem-2",
        pilotId: "p-2",
        name: "Laura Vidal",
        email: "laura@club.cl",
        status: "suspended",
        role: "admin",
      },
      {
        id: "mem-3",
        pilotId: "p-3",
        name: "Pedro Nuevo",
        email: "pedro@club.cl",
        status: "pending",
        role: "pilot",
      },
    ];
    const inviteBodies: Array<{ email: string; role: string }> = [];
    let failNextInvite = false;
    const removedIds: string[] = [];

    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === "/api/admin/pilots/pending") {
        return new Response(
          JSON.stringify({ pilots: [{ id: "p-3", name: "Pedro Nuevo", email: "pedro@club.cl" }] }),
          { status: 200 },
        );
      }
      if (url === "/api/admin/memberships") {
        return new Response(JSON.stringify({ memberships }), { status: 200 });
      }
      if (url === "/api/admin/checkouts") {
        return new Response(
          JSON.stringify({
            checkouts: [
              {
                id: "co-1",
                pilot_id: "p-1",
                type_code: "C152",
                tail: null,
                expires_at: null,
              },
              {
                id: "co-2",
                pilot_id: "p-1",
                type_code: "DA20",
                tail: "CC-XYZ",
                expires_at: Date.now() + 86_400_000,
              },
              {
                id: "co-expired",
                pilot_id: "p-1",
                type_code: "PA28",
                tail: "CC-OLD",
                expires_at: Date.now() - 86_400_000,
              },
            ],
          }),
          { status: 200 },
        );
      }
      if (url === "/api/admin/memberships/invite" && init?.method === "POST") {
        if (failNextInvite) {
          return new Response(JSON.stringify({ error: "db error" }), { status: 500 });
        }
        const parsed = JSON.parse(String(init.body ?? "{}")) as { email: string; role: string };
        inviteBodies.push(parsed);
        return new Response(JSON.stringify({ ok: true }), { status: 200 });
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

    // Pending membership is excluded from Membresías; notice links to Solicitudes.
    const rows = document.querySelectorAll<HTMLDivElement>(".mem-row");
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("Piloto · Activo");
    expect(rows[0].textContent).not.toContain("active");
    expect(rows[1].textContent).toContain("Administrador · Suspendido");
    expect(document.querySelector("#memberships")?.textContent).not.toContain("Pedro Nuevo");

    const notice = document.querySelector<HTMLParagraphElement>("#pending-pilots-notice")!;
    expect(notice.hidden).toBe(false);
    expect(notice.textContent).toContain("1 piloto espera aprobación en Solicitudes.");
    expect(notice.querySelector("a")?.getAttribute("href")).toBe("/club-admin-requests.html");

    // Ratings chips & Habilitar link on p-1 (expired PA28 excluded); empty ratings state on p-2.
    const p1Chips = [...rows[0].querySelectorAll(".rating-chip")].map((el) => el.textContent);
    expect(p1Chips).toEqual([`C152 · ${t("admin.checkouts.allTails")}`, "DA20 · CC-XYZ"]);
    expect(rows[0].querySelector<HTMLAnchorElement>(".rating-link")?.getAttribute("href")).toBe(
      "/club-admin-checkouts.html?pilot=p-1",
    );
    expect(rows[1].querySelectorAll(".rating-chip")).toHaveLength(0);
    expect(rows[1].textContent).toContain(t("club.admin.noRatings"));
    expect(rows[1].querySelector<HTMLAnchorElement>(".rating-link")?.getAttribute("href")).toBe(
      "/club-admin-checkouts.html?pilot=p-2",
    );

    // Invite pilot (default) -> sends { email, role: "pilot" } and shows pilot success copy.
    const emailInput = document.querySelector<HTMLInputElement>("#invite-email")!;
    const roleSelect = document.querySelector<HTMLSelectElement>("#invite-role")!;
    const inviteStatus = document.querySelector<HTMLParagraphElement>("#invite-status")!;
    emailInput.value = "piloto@club.cl";
    document
      .querySelector<HTMLFormElement>("#invite-form")!
      .dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));
    await vi.waitFor(() => expect(inviteStatus.textContent).toBe(t("club.admin.inviteSentPilot")));
    expect(inviteBodies[0]).toEqual({ email: "piloto@club.cl", role: "pilot" });
    expect(emailInput.value).toBe("");

    // Invite admin -> sends { email, role: "admin" } and shows admin success copy.
    roleSelect.value = "admin";
    emailInput.value = "admin2@club.cl";
    document
      .querySelector<HTMLFormElement>("#invite-form")!
      .dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));
    await vi.waitFor(() => expect(inviteStatus.textContent).toBe(t("club.admin.inviteSentAdmin")));
    expect(inviteBodies[1]).toEqual({ email: "admin2@club.cl", role: "admin" });

    // Invite error is shown with role="alert" and error class, not success copy.
    failNextInvite = true;
    emailInput.value = "nuevo@club.cl";
    document
      .querySelector<HTMLFormElement>("#invite-form")!
      .dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));
    await vi.waitFor(() => expect(inviteStatus.getAttribute("role")).toBe("alert"));
    expect(inviteStatus.classList.contains("status-error")).toBe(true);
    expect(inviteStatus.textContent).not.toBe(t("club.admin.inviteSentAdmin"));
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

  it("still renders Pilotos rows cleanly when /api/admin/checkouts fails", async () => {
    document.body.innerHTML = `
      <main>
        <form id="invite-form">
          <input id="invite-email" type="email" />
          <select id="invite-role"><option value="pilot" selected>Piloto</option></select>
          <button type="submit">Enviar</button>
        </form>
        <p id="invite-status"></p>
        <p id="pending-pilots-notice" hidden></p>
        <p id="memberships-status" hidden></p>
        <div id="memberships" class="admin-list"></div>
        <dialog id="remove-dialog">
          <p id="remove-dialog-body"></p>
          <button type="button" id="remove-cancel"></button>
          <button type="button" id="remove-confirm"></button>
        </dialog>
      </main>
    `;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = String(input);
        if (url === "/api/admin/memberships") {
          return new Response(
            JSON.stringify({
              memberships: [
                {
                  id: "mem-1",
                  pilotId: "p-1",
                  name: "Ana Ruiz",
                  email: "ana@club.cl",
                  status: "active",
                  role: "pilot",
                },
              ],
            }),
            { status: 200 },
          );
        }
        if (url === "/api/admin/pilots/pending") {
          return new Response(JSON.stringify({ pilots: [] }), { status: 200 });
        }
        if (url === "/api/admin/checkouts") {
          return new Response(JSON.stringify({ error: "db error" }), { status: 500 });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const ctrl = mountAdminPilots();
    await ctrl.refresh();

    const rows = document.querySelectorAll(".mem-row");
    expect(rows).toHaveLength(1);
    expect(rows[0].textContent).toContain("Ana Ruiz");
    expect(rows[0].querySelectorAll(".rating-chip")).toHaveLength(0);
    expect(rows[0].querySelector<HTMLAnchorElement>(".rating-link")?.getAttribute("href")).toBe(
      "/club-admin-checkouts.html?pilot=p-1",
    );
  });

  it("populates Matrículas type dropdown from aircraft types, handles empty types, preserves selection, and renders aligned rows", async () => {
    document.body.innerHTML = `
      <form id="form">
        <input id="tail-input" name="tail" />
        <select id="type-input" name="type" required></select>
        <button type="submit">Guardar matrícula</button>
      </form>
      <p id="tails-no-types-hint" hidden></p>
      <p id="tails-status" hidden></p>
      <ul id="list" class="admin-list"></ul>
    `;

    let types: Array<{ id: string; code: string }> = [
      { id: "t-1", code: "C152" },
      { id: "t-2", code: "DA20" },
    ];
    let aircraft = [
      { id: "a-1", tail: "CC-AAA", type_code: "C152", active: true },
      { id: "a-2", tail: "CC-BBB", type_code: "DA20", active: false },
    ];
    const posted: Array<Record<string, unknown>> = [];

    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = String(input);
        if (url === "/api/admin/aircraft-types") {
          return new Response(JSON.stringify({ types }), { status: 200 });
        }
        if (url === "/api/admin/aircraft" && (!init?.method || init.method === "GET")) {
          return new Response(JSON.stringify({ aircraft }), { status: 200 });
        }
        if (url === "/api/admin/aircraft" && init?.method === "POST") {
          const body = JSON.parse(String(init.body ?? "{}")) as Record<string, unknown>;
          posted.push(body);
          aircraft = [
            ...aircraft,
            {
              id: "a-3",
              tail: String(body.tail),
              type_code: String(body.type),
              active: Boolean(body.active),
            },
          ];
          return new Response(JSON.stringify({ id: "a-3" }), { status: 200 });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const tailsCtrl = mountAdminTails();
    await tailsCtrl.refresh();

    const typeSelect = document.querySelector<HTMLSelectElement>("#type-input")!;
    const submitBtn = document.querySelector<HTMLButtonElement>('button[type="submit"]')!;
    const hint = document.querySelector<HTMLParagraphElement>("#tails-no-types-hint")!;
    expect(typeSelect.disabled).toBe(false);
    expect(submitBtn.disabled).toBe(false);
    expect(hint.hidden).toBe(true);
    expect([...typeSelect.options].map((o) => o.value)).toEqual(["", "C152", "DA20"]);
    expect(typeSelect.options[0].textContent).toBe(t("admin.tails.typeSelectPlaceholder"));

    const tailItems = document.querySelectorAll("#list li.admin-row");
    expect(tailItems).toHaveLength(2);
    expect(tailItems[0].textContent).toContain("CC-AAA · C152 · Activa");
    expect(tailItems[0].querySelector("button")?.textContent).toBe("Desactivar");
    expect(tailItems[1].textContent).toContain("CC-BBB · DA20 · Inactiva");
    expect(tailItems[1].querySelector("button")?.textContent).toBe("Activar");

    // Submitting keeps POST body shape and preserves selected type across refresh.
    document.querySelector<HTMLInputElement>("#tail-input")!.value = "CC-CCC";
    typeSelect.value = "DA20";
    document
      .querySelector<HTMLFormElement>("#form")!
      .dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));
    await vi.waitFor(() => expect(posted).toHaveLength(1));
    expect(posted[0]).toEqual({ tail: "CC-CCC", type: "DA20", active: true });
    expect(typeSelect.value).toBe("DA20");

    // Empty types -> disables select + submit and displays hint link to Tipos.
    types = [];
    await tailsCtrl.refresh();
    expect(typeSelect.disabled).toBe(true);
    expect(submitBtn.disabled).toBe(true);
    expect(hint.hidden).toBe(false);
    expect(hint.textContent).toBe("Primero crea un tipo en Tipos.");
    expect(hint.querySelector("a")?.getAttribute("href")).toBe("/club-admin-types.html");

    // Tipos list also uses aligned rows and empty state.
    document.body.innerHTML = `
      <form id="form">
        <input id="code" />
        <button type="submit">Guardar tipo</button>
      </form>
      <p id="types-status" hidden></p>
      <ul id="list" class="admin-list"></ul>
    `;
    let typeList: Array<{ id: string; code: string }> = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        if (String(input) === "/api/admin/aircraft-types") {
          return new Response(JSON.stringify({ types: typeList }), { status: 200 });
        }
        return new Response("{}", { status: 404 });
      }),
    );
    const typesCtrl = mountAdminTypes();
    await typesCtrl.refresh();
    expect(document.querySelector("#list")?.textContent).toBe(t("admin.types.empty"));

    typeList = [{ id: "t-1", code: "C152" }];
    await typesCtrl.refresh();
    expect(document.querySelectorAll("#list li.admin-row")).toHaveLength(1);
    expect(document.querySelector("#list li.admin-row")?.textContent).toBe("C152");
  });

  it("renders updated admin nav labels in es and en", async () => {
    document.body.innerHTML = `<div id="admin-nav"></div>`;
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response(JSON.stringify({ pilots: 0, accessRequests: 0 }), { status: 200 })),
    );

    initI18n({ stored: "es" });
    mountAdminNav();
    expect(
      document.querySelector('a[href="/club-admin-kiosk-layout.html"]')?.textContent,
    ).toBe("Diseño de pantalla");
    expect(
      document.querySelector('a[href="/club-admin-checkin-qr.html"]')?.textContent,
    ).toBe("QR en celular");

    initI18n({ stored: "en" });
    mountAdminNav();
    expect(
      document.querySelector('a[href="/club-admin-kiosk-layout.html"]')?.textContent,
    ).toBe("Screen layout");
    expect(
      document.querySelector('a[href="/club-admin-checkin-qr.html"]')?.textContent,
    ).toBe("Phone QR");
  });
});
