import { describe, expect, it, vi } from "vitest";
import {
  TELEMETRY_VISIBLE_STORAGE_KEY,
  applyTelemetryVisibility,
  isEditableTarget,
  loadTelemetryVisible,
  saveTelemetryVisible,
  shouldHandleShortcut,
} from "./telemetryVisibility";

function createMemoryStorage(initial?: Record<string, string>): Storage {
  const map = new Map<string, string>(Object.entries(initial ?? {}));
  return {
    get length() {
      return map.size;
    },
    clear() {
      map.clear();
    },
    getItem(key: string) {
      return map.has(key) ? map.get(key)! : null;
    },
    key(index: number) {
      return [...map.keys()][index] ?? null;
    },
    removeItem(key: string) {
      map.delete(key);
    },
    setItem(key: string, value: string) {
      map.set(key, String(value));
    },
  };
}

describe("telemetryVisibility storage", () => {
  it("defaults to shown when unset or unavailable", () => {
    expect(loadTelemetryVisible(createMemoryStorage())).toBe(true);
    expect(loadTelemetryVisible(null)).toBe(true);
  });

  it("loads false for stored false/0 and true for true/unexpected values", () => {
    expect(
      loadTelemetryVisible(createMemoryStorage({ [TELEMETRY_VISIBLE_STORAGE_KEY]: "false" })),
    ).toBe(false);
    expect(
      loadTelemetryVisible(createMemoryStorage({ [TELEMETRY_VISIBLE_STORAGE_KEY]: "0" })),
    ).toBe(false);
    expect(
      loadTelemetryVisible(createMemoryStorage({ [TELEMETRY_VISIBLE_STORAGE_KEY]: "true" })),
    ).toBe(true);
    expect(
      loadTelemetryVisible(createMemoryStorage({ [TELEMETRY_VISIBLE_STORAGE_KEY]: "garbage" })),
    ).toBe(true);
  });

  it("persists and round-trips visibility state", () => {
    const storage = createMemoryStorage();
    saveTelemetryVisible(false, storage);
    expect(storage.getItem(TELEMETRY_VISIBLE_STORAGE_KEY)).toBe("false");
    expect(loadTelemetryVisible(storage)).toBe(false);

    saveTelemetryVisible(true, storage);
    expect(storage.getItem(TELEMETRY_VISIBLE_STORAGE_KEY)).toBe("true");
    expect(loadTelemetryVisible(storage)).toBe(true);
  });

  it("handles throwing storage cleanly", () => {
    const broken = {
      getItem: vi.fn(() => {
        throw new Error("denied");
      }),
      setItem: vi.fn(() => {
        throw new Error("quota");
      }),
    };
    expect(loadTelemetryVisible(broken)).toBe(true);
    expect(() => saveTelemetryVisible(false, broken)).not.toThrow();
  });
});

describe("shouldHandleShortcut", () => {
  it("matches h and H when no editable element is focused", () => {
    const button = document.createElement("button");
    expect(shouldHandleShortcut({ key: "h", target: document.body })).toBe(true);
    expect(shouldHandleShortcut({ key: "H", target: document.body })).toBe(true);
    expect(shouldHandleShortcut({ key: "h", target: button })).toBe(true);
    expect(shouldHandleShortcut({ key: "h", target: null })).toBe(true);
  });

  it("ignores other keys including Space and T", () => {
    expect(shouldHandleShortcut({ key: "t", target: document.body })).toBe(false);
    expect(shouldHandleShortcut({ key: "T", target: document.body })).toBe(false);
    expect(shouldHandleShortcut({ key: " ", target: document.body })).toBe(false);
    expect(shouldHandleShortcut({ key: "Escape", target: document.body })).toBe(false);
  });

  it("ignores shortcut when ctrl, meta, or alt modifiers are held", () => {
    expect(shouldHandleShortcut({ key: "h", ctrlKey: true, target: document.body })).toBe(false);
    expect(shouldHandleShortcut({ key: "h", metaKey: true, target: document.body })).toBe(false);
    expect(shouldHandleShortcut({ key: "h", altKey: true, target: document.body })).toBe(false);
  });

  it("ignores repeated or already-prevented events", () => {
    expect(shouldHandleShortcut({ key: "h", repeat: true, target: document.body })).toBe(false);
    expect(
      shouldHandleShortcut({ key: "h", defaultPrevented: true, target: document.body }),
    ).toBe(false);
  });

  it("ignores input, textarea, select, and contentEditable targets", () => {
    const input = document.createElement("input");
    const textarea = document.createElement("textarea");
    const select = document.createElement("select");
    const editableTrue = document.createElement("div");
    editableTrue.setAttribute("contenteditable", "true");
    const editableEmpty = document.createElement("div");
    editableEmpty.setAttribute("contenteditable", "");
    const editablePlain = document.createElement("div");
    editablePlain.setAttribute("contenteditable", "plaintext-only");
    const editableChild = document.createElement("span");
    editableTrue.append(editableChild);
    const editableText = document.createTextNode("hello");
    editableChild.append(editableText);

    expect(shouldHandleShortcut({ key: "h", target: input })).toBe(false);
    expect(shouldHandleShortcut({ key: "h", target: textarea })).toBe(false);
    expect(shouldHandleShortcut({ key: "h", target: select })).toBe(false);
    expect(shouldHandleShortcut({ key: "h", target: editableTrue })).toBe(false);
    expect(shouldHandleShortcut({ key: "h", target: editableEmpty })).toBe(false);
    expect(shouldHandleShortcut({ key: "h", target: editablePlain })).toBe(false);
    expect(shouldHandleShortcut({ key: "h", target: editableChild })).toBe(false);
    expect(shouldHandleShortcut({ key: "h", target: editableText })).toBe(false);
  });

  it("allows shortcut inside an explicitly non-editable descendant", () => {
    const editable = document.createElement("div");
    editable.setAttribute("contenteditable", "true");
    const nonEditable = document.createElement("div");
    nonEditable.setAttribute("contenteditable", "false");
    editable.append(nonEditable);

    expect(isEditableTarget(nonEditable)).toBe(false);
    expect(shouldHandleShortcut({ key: "h", target: nonEditable })).toBe(true);
  });
});

describe("applyTelemetryVisibility", () => {
  it("shows HUD and syncs aria-pressed and body class when flight is loaded", () => {
    const hud = document.createElement("aside");
    const button = document.createElement("button");
    const body = document.createElement("body");
    hud.hidden = true;

    const shown = applyTelemetryVisibility({
      hud,
      toggleButton: button,
      body,
      visible: true,
      hasFlight: true,
    });

    expect(shown).toBe(true);
    expect(hud.hidden).toBe(false);
    expect(button.getAttribute("aria-pressed")).toBe("true");
    expect(body.classList.contains("telemetry-hidden")).toBe(false);
  });

  it("hides HUD even when a flight is loaded if user preference is hidden", () => {
    const hud = document.createElement("aside");
    const button = document.createElement("button");
    const body = document.createElement("body");
    hud.hidden = false;

    const shown = applyTelemetryVisibility({
      hud,
      toggleButton: button,
      body,
      visible: false,
      hasFlight: true,
    });

    expect(shown).toBe(false);
    expect(hud.hidden).toBe(true);
    expect(button.getAttribute("aria-pressed")).toBe("false");
    expect(body.classList.contains("telemetry-hidden")).toBe(true);
  });

  it("keeps HUD hidden before a flight loads while reflecting toggle aria-pressed", () => {
    const hud = document.createElement("aside");
    const button = document.createElement("button");
    const body = document.createElement("body");

    const shown = applyTelemetryVisibility({
      hud,
      toggleButton: button,
      body,
      visible: true,
      hasFlight: false,
    });

    expect(shown).toBe(false);
    expect(hud.hidden).toBe(true);
    expect(button.getAttribute("aria-pressed")).toBe("true");
    expect(body.classList.contains("telemetry-hidden")).toBe(false);
  });
});
