import { describe, expect, it } from "vitest";
import { clubLoginRedirectUrl, safeNextPath } from "./safeNext";

describe("safeNextPath", () => {
  it("allows same-origin paths", () => {
    expect(safeNextPath("/club-admin-types.html")).toBe("/club-admin-types.html");
    expect(safeNextPath("/club-admin-map.html?x=1")).toBe("/club-admin-map.html?x=1");
  });

  it("rejects open redirects", () => {
    expect(safeNextPath("//evil")).toBe("/");
    expect(safeNextPath("https://evil.example")).toBe("/");
    expect(safeNextPath("/path\\evil")).toBe("/");
    expect(safeNextPath("/\\evil")).toBe("/");
    expect(safeNextPath(String.raw`/\evil.com`)).toBe("/");
    expect(safeNextPath("/%09/evil.com")).toBe("/");
    expect(safeNextPath("/%0a/evil")).toBe("/");
    expect(safeNextPath("/%0d%0a/evil")).toBe("/");
    expect(safeNextPath("/%5Cevil")).toBe("/");
    expect(safeNextPath("/\t/evil")).toBe("/");
    expect(safeNextPath("/\n/evil")).toBe("/");
    expect(safeNextPath("/\r/evil")).toBe("/");
  });

  it("blocks legacy and club login loops", () => {
    expect(safeNextPath("/login")).toBe("/");
    expect(safeNextPath("/login?next=/")).toBe("/");
    expect(safeNextPath("/club-login.html")).toBe("/");
  });
});

describe("clubLoginRedirectUrl", () => {
  it("encodes next query", () => {
    expect(clubLoginRedirectUrl("/club-admin-tails.html")).toBe(
      "/login?mode=club&next=%2Fclub-admin-tails.html",
    );
  });
});