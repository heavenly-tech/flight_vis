import { createHash, createHmac, timingSafeEqual } from "node:crypto";

export const COOKIE = "fv_session";
export const TTL_SEC = 7 * 24 * 3600;
const SALT = "flight-vis-login-v1";

function readPassword(env = process.env) {
  return String(env.FLIGHT_VIS_PASSWORD || "").trim();
}

function signKey(secret) {
  return createHash("sha256").update(SALT).update(secret).digest();
}

function equal(a, b) {
  if (a.length !== b.length) return false;
  return timingSafeEqual(a, b);
}

export function createAuth(env = process.env) {
  const password = () => readPassword(env);
  const configured = () => Boolean(password());

  function mint(now = Math.floor(Date.now() / 1000)) {
    const secret = password();
    const exp = now + TTL_SEC;
    const payload = `v1.${exp}`;
    const sig = createHmac("sha256", signKey(secret)).update(payload).digest("hex");
    return `${payload}.${sig}`;
  }

  function verify(token, now = Math.floor(Date.now() / 1000)) {
    if (!configured() || !token) return false;
    const parts = String(token).split(".");
    if (parts.length !== 3 || parts[0] !== "v1") return false;
    const exp = Number(parts[1]);
    if (!Number.isInteger(exp)) return false;
    const payload = `${parts[0]}.${parts[1]}`;
    const expect = createHmac("sha256", signKey(password())).update(payload).digest("hex");
    const got = parts[2];
    if (!equal(Buffer.from(expect, "utf8"), Buffer.from(got, "utf8"))) return false;
    return exp > now;
  }

  function checkPassword(candidate) {
    if (!configured()) return false;
    const a = createHash("sha256").update(password()).digest();
    const b = createHash("sha256").update(String(candidate || "")).digest();
    return equal(a, b);
  }

  return { password, configured, mint, verify, checkPassword };
}

export const defaultAuth = createAuth();

export function parseCookie(header, name = COOKIE) {
  if (!header) return null;
  for (const part of String(header).split(";")) {
    const [key, ...rest] = part.trim().split("=");
    if (key === name) return rest.join("=");
  }
  return null;
}

export function setCookieHeader(token, secure) {
  let flags = `${COOKIE}=${token}; Path=/; Max-Age=${TTL_SEC}; HttpOnly; SameSite=Lax`;
  if (secure) flags += "; Secure";
  return flags;
}

export function clearCookieHeader(secure) {
  let flags = `${COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax`;
  if (secure) flags += "; Secure";
  return flags;
}

export function safeNext(next) {
  const value = String(next || "").trim();
  if (!value.startsWith("/") || value.startsWith("//")) return "/";
  if (value.includes("\\") || value.includes("://")) return "/";
  if (value === "/login" || value.startsWith("/login?")) return "/";
  return value;
}

export function isSecureRequest(c) {
  const proto = (c.req.header("x-forwarded-proto") || "").split(",")[0].trim().toLowerCase();
  if (proto === "https") return true;
  try {
    return new URL(c.req.url).protocol === "https:";
  } catch {
    return false;
  }
}

export function wantsJson(c) {
  const accept = c.req.header("accept") || "";
  const type = c.req.header("content-type") || "";
  return accept.includes("application/json") || type.includes("application/json");
}
