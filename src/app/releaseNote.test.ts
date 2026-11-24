import { describe, expect, it } from "vitest";
import { releaseNoteText } from "./releaseNote";

describe("releaseNoteText", () => {
  it("returns null when message is undefined", () => {
    expect(releaseNoteText({})).toBeNull();
  });

  it("returns null when message is null", () => {
    expect(releaseNoteText({ message: null })).toBeNull();
  });

  it("returns null when message is not a string", () => {
    expect(releaseNoteText({ message: 123 })).toBeNull();
    expect(releaseNoteText({ message: true })).toBeNull();
    expect(releaseNoteText({ message: {} })).toBeNull();
  });

  it("returns null when message is empty string", () => {
    expect(releaseNoteText({ message: "" })).toBeNull();
  });

  it("returns null when message is whitespace only", () => {
    expect(releaseNoteText({ message: "   " })).toBeNull();
    expect(releaseNoteText({ message: "\n\t  " })).toBeNull();
  });

  it("returns trimmed string for valid message", () => {
    expect(releaseNoteText({ message: "hello" })).toBe("hello");
    expect(releaseNoteText({ message: "  hello  " })).toBe("hello");
    expect(releaseNoteText({ message: "Desinstala la app anterior antes de instalar esta versi\u00f3n." })).toBe(
      "Desinstala la app anterior antes de instalar esta versi\u00f3n.",
    );
  });

  it("preserves HTML entities literally", () => {
    expect(releaseNoteText({ message: "<b>x</b>" })).toBe("<b>x</b>");
  });
});
