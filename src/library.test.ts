import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AuthStatus, FlightListItem } from "./api";
import { applyI18nDocument, initI18n } from "./i18n/index";
import type { LocalFlightMeta } from "./live/localFlights";

const localMocks = vi.hoisted(() => ({
  listLocalFlights: vi.fn<() => Promise<LocalFlightMeta[]>>(async () => []),
  deleteLocalFlight: vi.fn<(id: string) => Promise<void>>(async () => undefined),
}));

vi.mock("./live/localFlights", () => ({
  listLocalFlights: localMocks.listLocalFlights,
  deleteLocalFlight: localMocks.deleteLocalFlight,
  isLocalSlug: (slug: string) => slug.startsWith("local:"),
}));

import { mountLibrary } from "./library";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

function loadIndexBody(): string {
  const html = fs.readFileSync(path.join(root, "index.html"), "utf8");
  const match = html.match(/<body[^>]*>([\s\S]*?)<\/body>/i);
  const body = match ? match[1] : html;
  return body.replace(/<script[\s\S]*?<\/script>/gi, "");
}

function patchDialogPolyfill(): void {
  const proto = HTMLDialogElement.prototype as HTMLDialogElement & {
    showModal?: () => void;
    close?: () => void;
  };
  proto.showModal = function showModal(this: HTMLDialogElement) {
    this.setAttribute("open", "");
  };
  proto.close = function close(this: HTMLDialogElement) {
    this.removeAttribute("open");
  };
}

function dispatchFormSubmit(form: HTMLFormElement, submitter: HTMLButtonElement): void {
  const ev = new Event("submit", { cancelable: true, bubbles: true });
  Object.defineProperty(ev, "submitter", { value: submitter });
  form.dispatchEvent(ev);
}

describe("library delete modal ID alignment and canManageLibrary gating", () => {
  beforeEach(() => {
    patchDialogPolyfill();
    document.body.innerHTML = loadIndexBody();
    initI18n({ stored: "es" });
    applyI18nDocument(document);
    localMocks.listLocalFlights.mockResolvedValue([]);
    localMocks.deleteLocalFlight.mockReset();
    localMocks.deleteLocalFlight.mockResolvedValue(undefined);
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    document.body.innerHTML = "";
  });

  it("updates #delete-title (matching index.html) and deletes remote flight when canManageLibrary is true", async () => {
    expect(document.querySelector("#delete-title")).not.toBeNull();
    expect(document.querySelector("#delete-label")).toBeNull();

    let remoteFlights: FlightListItem[] = [
      {
        slug: "vuelo-5",
        name: "Vuelo 5",
        fileName: "vuelo5.gpx",
        updatedAt: "2026-11-24T12:00:00Z",
        createdAt: "2026-11-24T12:00:00Z",
        size: 2048,
      },
    ];
    const deletedSlugs: string[] = [];
    const onDeleted = vi.fn();
    const setStatus = vi.fn();

    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = String(input);
        if (url === "/api/flights" && (!init?.method || init.method === "GET")) {
          return new Response(JSON.stringify(remoteFlights), { status: 200 });
        }
        if (url === "/api/flights/vuelo-5" && init?.method === "DELETE") {
          deletedSlugs.push("vuelo-5");
          remoteFlights = remoteFlights.filter((f) => f.slug !== "vuelo-5");
          return new Response(JSON.stringify({ ok: true }), { status: 200 });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const auth: AuthStatus = {
      authenticated: true,
      configured: true,
      canManageLibrary: true,
    };

    const lib = mountLibrary({
      getAuth: () => auth,
      currentSlug: () => "vuelo-5",
      onOpenFlight: vi.fn(async () => undefined),
      onRenamed: vi.fn(),
      onDeleted,
      setStatus,
    });

    lib.setOpen(true);
    await vi.waitFor(() => expect(document.querySelectorAll("#library-list li")).toHaveLength(1));

    const tools = document.querySelector<HTMLElement>("#library-tools")!;
    expect(tools.hidden).toBe(false);

    const deleteBtn = document.querySelector<HTMLButtonElement>("#library-list .chip.danger");
    expect(deleteBtn).not.toBeNull();
    expect(() => deleteBtn!.click()).not.toThrow();

    const deleteDialog = document.querySelector<HTMLDialogElement>("#delete-dialog")!;
    const deleteTitle = document.querySelector<HTMLElement>("#delete-title")!;
    expect(deleteDialog.hasAttribute("open")).toBe(true);
    expect(deleteTitle.textContent).toBe("¿Eliminar Vuelo 5 (vuelo-5)?");

    const deleteForm = document.querySelector<HTMLFormElement>("#delete-form")!;
    const okBtn = deleteForm.querySelector<HTMLButtonElement>('button[value="ok"]')!;
    dispatchFormSubmit(deleteForm, okBtn);

    await vi.waitFor(() => expect(deletedSlugs).toEqual(["vuelo-5"]));
    await vi.waitFor(() => expect(onDeleted).toHaveBeenCalledWith("vuelo-5"));
    await vi.waitFor(() => expect(document.querySelectorAll("#library-list li")).toHaveLength(0));
  });

  it("hides remote delete controls and no-ops the delete handler when canManageLibrary is false", async () => {
    const remoteFlights: FlightListItem[] = [
      {
        slug: "vuelo-5",
        name: "Vuelo 5",
        fileName: "vuelo5.gpx",
        updatedAt: "2026-11-24T12:00:00Z",
        createdAt: "2026-11-24T12:00:00Z",
        size: 2048,
      },
    ];
    const deleteRequests: string[] = [];
    const onDeleted = vi.fn();

    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = String(input);
        if (url === "/api/flights" && (!init?.method || init.method === "GET")) {
          return new Response(JSON.stringify(remoteFlights), { status: 200 });
        }
        if (init?.method === "DELETE") {
          deleteRequests.push(url);
          return new Response(JSON.stringify({ ok: true }), { status: 200 });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    let auth: AuthStatus = {
      authenticated: true,
      configured: true,
      canManageLibrary: false,
    };

    const lib = mountLibrary({
      getAuth: () => auth,
      currentSlug: () => "vuelo-5",
      onOpenFlight: vi.fn(async () => undefined),
      onRenamed: vi.fn(),
      onDeleted,
      setStatus: vi.fn(),
    });

    lib.setOpen(true);
    await vi.waitFor(() => expect(document.querySelectorAll("#library-list li")).toHaveLength(1));

    expect(document.querySelector<HTMLElement>("#library-tools")!.hidden).toBe(true);
    expect(document.querySelectorAll("#library-list .library-item-actions")).toHaveLength(0);
    expect(document.querySelectorAll("#library-list .chip.danger")).toHaveLength(0);

    // Even if a dialog was opened while permitted and rights are then revoked, submit must no-op.
    auth = { authenticated: true, configured: true, canManageLibrary: true };
    lib.setAuth(auth);
    await vi.waitFor(() => expect(document.querySelectorAll("#library-list .chip.danger")).toHaveLength(1));
    document.querySelector<HTMLButtonElement>("#library-list .chip.danger")!.click();

    const deleteDialog = document.querySelector<HTMLDialogElement>("#delete-dialog")!;
    expect(deleteDialog.hasAttribute("open")).toBe(true);

    // Revoke permission without closing the dialog first: submit handler must be a strict no-op.
    auth = { authenticated: true, configured: true, canManageLibrary: false };
    const deleteForm = document.querySelector<HTMLFormElement>("#delete-form")!;
    const okBtn = deleteForm.querySelector<HTMLButtonElement>('button[value="ok"]')!;
    dispatchFormSubmit(deleteForm, okBtn);

    await new Promise((r) => setTimeout(r, 20));
    expect(deleteRequests).toEqual([]);
    expect(onDeleted).not.toHaveBeenCalled();

    // Calling setAuth with canManageLibrary=false closes any open remote delete dialog and hides buttons.
    auth = { authenticated: true, configured: true, canManageLibrary: true };
    lib.setAuth(auth);
    await vi.waitFor(() => expect(document.querySelectorAll("#library-list .chip.danger")).toHaveLength(1));
    document.querySelector<HTMLButtonElement>("#library-list .chip.danger")!.click();
    expect(deleteDialog.hasAttribute("open")).toBe(true);

    auth = { authenticated: true, configured: true, canManageLibrary: false };
    lib.setAuth(auth);
    expect(deleteDialog.hasAttribute("open")).toBe(false);
    expect(document.querySelectorAll("#library-list .chip.danger")).toHaveLength(0);
  });

  it("still allows deleting local device recordings and updates #delete-title when canManageLibrary is false", async () => {
    localMocks.listLocalFlights.mockResolvedValue([
      {
        id: "local:rec-1",
        name: "Vuelo local",
        fileName: "local.gpx",
        updatedAt: "2026-11-24T12:00:00Z",
        size: 1024,
      },
    ]);
    const remoteDeleteRequests: string[] = [];
    const onDeleted = vi.fn();

    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = String(input);
        if (url === "/api/flights") {
          return new Response(JSON.stringify([]), { status: 200 });
        }
        if (init?.method === "DELETE") {
          remoteDeleteRequests.push(url);
        }
        return new Response("{}", { status: 404 });
      }),
    );

    const auth: AuthStatus = {
      authenticated: true,
      configured: true,
      canManageLibrary: false,
    };

    const lib = mountLibrary({
      getAuth: () => auth,
      currentSlug: () => null,
      onOpenFlight: vi.fn(async () => undefined),
      onRenamed: vi.fn(),
      onDeleted,
      setStatus: vi.fn(),
    });

    lib.setOpen(true);
    await vi.waitFor(() => expect(document.querySelectorAll("#library-list li")).toHaveLength(1));

    const deleteBtn = document.querySelector<HTMLButtonElement>("#library-list .chip.danger")!;
    expect(deleteBtn).not.toBeNull();
    deleteBtn.click();

    expect(document.querySelector<HTMLElement>("#delete-title")!.textContent).toBe(
      "¿Eliminar Vuelo local?",
    );

    const deleteForm = document.querySelector<HTMLFormElement>("#delete-form")!;
    const okBtn = deleteForm.querySelector<HTMLButtonElement>('button[value="ok"]')!;
    dispatchFormSubmit(deleteForm, okBtn);

    await vi.waitFor(() => expect(localMocks.deleteLocalFlight).toHaveBeenCalledWith("local:rec-1"));
    await vi.waitFor(() => expect(onDeleted).toHaveBeenCalledWith("local:rec-1"));
    expect(remoteDeleteRequests).toEqual([]);
  });
});
