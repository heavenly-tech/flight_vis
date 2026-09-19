import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { createAuth, parseCookie, safeNext } from "./auth.mjs";

describe("session auth", () => {
  it("rejects when FLIGHT_VIS_PASSWORD is unset", () => {
    const auth = createAuth({});
    assert.equal(auth.configured(), false);
    assert.equal(auth.checkPassword("anything"), false);
    assert.equal(auth.verify(auth.mint()), false);
  });

  it("mints and verifies a session for the configured password", () => {
    const auth = createAuth({ FLIGHT_VIS_PASSWORD: "secret" });
    assert.equal(auth.configured(), true);
    assert.equal(auth.checkPassword("secret"), true);
    assert.equal(auth.checkPassword("nope"), false);
    assert.equal(auth.verify(auth.mint()), true);
  });

  it("rejects a token after expiry or password change", () => {
    const auth = createAuth({ FLIGHT_VIS_PASSWORD: "secret" });
    const now = 1_700_000_000;
    const token = auth.mint(now);
    assert.equal(auth.verify(token, now + 10), true);
    assert.equal(auth.verify(token, now + 8 * 24 * 3600), false);
    const other = createAuth({ FLIGHT_VIS_PASSWORD: "other" });
    assert.equal(other.verify(token, now + 10), false);
  });

  it("parses the session cookie and sanitizes next", () => {
    assert.equal(parseCookie("a=1; fv_session=v1.1.abc; b=2"), "v1.1.abc");
    assert.equal(parseCookie("fv_session="), "");
    assert.equal(safeNext("/vuelo-5"), "/vuelo-5");
    assert.equal(safeNext("//evil"), "/");
    assert.equal(safeNext("https://evil.example"), "/");
    assert.equal(safeNext("/login"), "/");
  });
});
