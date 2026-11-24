import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { applyI18nDocument, initI18n, t } from "../i18n/index";
import { mountAdminCheckouts } from "./adminCheckouts";

function buildCheckoutsDom(): void {
  document.body.innerHTML = `
    <main>
      <h1 data-i18n="nav.checkouts">Habilitaciones</h1>
      <div id="admin-nav"></div>
      <form id="form">
        <label for="pilot"><span data-i18n="admin.checkouts.pilotLabel">Piloto</span>
          <select id="pilot" name="pilot_id" required></select>
        </label>
        <label for="type"><span data-i18n="admin.checkouts.typeLabel">Tipo de aeronave</span>
          <select id="type" name="type" required></select>
        </label>
        <label for="tail"><span data-i18n="admin.checkouts.tailLabel">Matrícula específica (opcional)</span>
          <select id="tail" name="tail"></select>
        </label>
        <label for="expires_at"><span data-i18n="admin.checkouts.expiresLabel">Vence (opcional)</span>
          <input id="expires_at" name="expires_at" type="date" />
        </label>
        <button type="submit" data-i18n="admin.checkouts.add">Agregar habilitación</button>
      </form>
      <p id="checkouts-status" aria-live="polite" hidden></p>
      <ul id="list"></ul>
      <dialog id="revoke-dialog">
        <h2 id="revoke-dialog-title" data-i18n="admin.checkouts.revokeTitle">Revocar habilitación</h2>
        <p id="revoke-dialog-body"></p>
        <button type="button" id="revoke-cancel" data-i18n="common.cancel">Cancelar</button>
        <button type="button" id="revoke-confirm" data-i18n="admin.checkouts.revoke">Revocar</button>
      </dialog>
    </main>
  `;
  applyI18nDocument();
}

describe("adminCheckouts", () => {
  beforeEach(() => {
    initI18n({ stored: "es" });
    buildCheckoutsDom();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
  });

  it("populates type and tail selects, formats Santiago dates, and revokes with confirmation dialog", async () => {
    let checkouts = [
      {
        id: "co-1",
        pilot_id: "p-1",
        pilot_name: "Ana Pérez",
        pilot_email: "ana@club.cl",
        type_code: "C152",
        tail: "CC-AAA",
        // 2026-12-31T01:30:00Z -> 30/12/2026 in America/Santiago
        expires_at: Date.UTC(2026, 11, 31, 1, 30, 0),
      },
      {
        id: "co-2",
        pilot_id: "p-1",
        pilot_name: "Ana Pérez",
        pilot_email: "ana@club.cl",
        type_code: "DA20",
        tail: null,
        expires_at: null,
      },
    ];
    let postedCheckout: Record<string, unknown> | null = null;
    const deletedIds: string[] = [];

    const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      const method = init?.method ?? "GET";
      if (url === "/api/admin/pilots") {
        return new Response(
          JSON.stringify({ pilots: [{ id: "p-1", name: "Ana Pérez", email: "ana@club.cl" }] }),
          { status: 200 },
        );
      }
      if (url === "/api/admin/aircraft-types") {
        return new Response(
          JSON.stringify({
            types: [
              { id: "t-1", code: "C152" },
              { id: "t-2", code: "DA20" },
            ],
          }),
          { status: 200 },
        );
      }
      if (url === "/api/admin/aircraft") {
        return new Response(
          JSON.stringify({
            aircraft: [
              { id: "a-1", tail: "CC-AAA", type_code: "C152", active: true },
              { id: "a-2", tail: "CC-BBB", type_code: "DA20", active: true },
            ],
          }),
          { status: 200 },
        );
      }
      if (url === "/api/admin/checkouts" && method === "GET") {
        return new Response(JSON.stringify({ checkouts }), { status: 200 });
      }
      if (url === "/api/admin/checkouts" && method === "POST") {
        postedCheckout = JSON.parse(String(init?.body ?? "{}")) as Record<string, unknown>;
        return new Response(JSON.stringify({ ok: true }), { status: 200 });
      }
      if (url.startsWith("/api/admin/checkouts/") && method === "DELETE") {
        const id = decodeURIComponent(url.split("/").pop() ?? "");
        deletedIds.push(id);
        checkouts = checkouts.filter((c) => c.id !== id);
        return new Response(JSON.stringify({ ok: true }), { status: 200 });
      }
      return new Response("{}", { status: 404 });
    });
    vi.stubGlobal("fetch", fetchMock);

    const ctrl = mountAdminCheckouts();
    await ctrl.refresh();

    expect(document.querySelector("h1")?.textContent).toBe("Habilitaciones");
    expect(document.querySelector('label[for="expires_at"] span')?.textContent).toBe(
      "Vence (opcional)",
    );

    const typeSelect = document.querySelector<HTMLSelectElement>("#type")!;
    const tailSelect = document.querySelector<HTMLSelectElement>("#tail")!;
    expect([...typeSelect.options].map((o) => o.value)).toEqual(["", "C152", "DA20"]);
    expect([...tailSelect.options].map((o) => o.value)).toEqual(["", "CC-AAA", "CC-BBB"]);

    // Selecting type filters tail options to matching type.
    typeSelect.value = "C152";
    typeSelect.dispatchEvent(new Event("change"));
    expect([...tailSelect.options].map((o) => o.value)).toEqual(["", "CC-AAA"]);

    // Existing rows show Santiago date (30/12/2026) and localized labels, not ISO/English.
    const rows = document.querySelectorAll<HTMLLIElement>("#list li");
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain("30/12/2026");
    expect(rows[0].textContent).not.toContain("2026-12-31T");
    expect(rows[1].textContent).toContain(t("admin.checkouts.allTails"));
    expect(rows[1].textContent).toContain(t("admin.checkouts.noExpiry"));

    // Submitting parses date input as end-of-day in America/Santiago.
    const pilotSelect = document.querySelector<HTMLSelectElement>("#pilot")!;
    const expiresInput = document.querySelector<HTMLInputElement>("#expires_at")!;
    pilotSelect.value = "p-1";
    tailSelect.value = "CC-AAA";
    expiresInput.value = "2026-07-15";
    document
      .querySelector<HTMLFormElement>("#form")!
      .dispatchEvent(new Event("submit", { cancelable: true, bubbles: true }));
    await vi.waitFor(() => expect(postedCheckout).not.toBeNull());
    expect(postedCheckout).toEqual({
      pilot_id: "p-1",
      type: "C152",
      tail: "CC-AAA",
      expires_at: Date.UTC(2026, 6, 16, 3, 59, 59, 999),
    });

    // Revoke flow: open dialog -> cancel keeps row -> open again -> confirm removes row.
    const dialog = document.querySelector<HTMLDialogElement>("#revoke-dialog")!;
    const firstRevoke = document.querySelector<HTMLButtonElement>('#list li[data-checkout-id="co-1"] .btn-revoke')!;
    firstRevoke.click();
    expect(dialog.hasAttribute("open")).toBe(true);
    expect(document.querySelector("#revoke-dialog-body")?.textContent).toContain("Ana Pérez");

    document.querySelector<HTMLButtonElement>("#revoke-cancel")!.click();
    expect(dialog.hasAttribute("open")).toBe(false);
    expect(deletedIds).toEqual([]);
    expect(document.querySelectorAll("#list li")).toHaveLength(2);

    firstRevoke.click();
    expect(dialog.hasAttribute("open")).toBe(true);
    document.querySelector<HTMLButtonElement>("#revoke-confirm")!.click();
    await vi.waitFor(() => expect(deletedIds).toEqual(["co-1"]));
    await vi.waitFor(() => expect(document.querySelectorAll("#list li")).toHaveLength(1));
    expect(document.querySelector('#list li[data-checkout-id="co-1"]')).toBeNull();
  });
});
