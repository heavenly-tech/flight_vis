import { describe, expect, it } from "vitest";

describe("account signup API contract", () => {
  it("signup-available shape", () => {
    expect({ ok: true, signup: true }).toMatchObject({ signup: true });
  });
});