const RESERVED = new Set([
  "api",
  "assets",
  "cesiumstatic",
  "samples",
  "static",
  "health",
  "login",
  "logout",
  "favicon.ico",
]);

export function normalizeSlug(input: string): string {
  return input
    .trim()
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
}

export function isValidSlug(slug: string): boolean {
  return /^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(slug) && !RESERVED.has(slug);
}

export function slugFromFileName(fileName: string): string {
  const base = fileName.replace(/\.[^.]+$/, "");
  const slug = normalizeSlug(base);
  return isValidSlug(slug) ? slug : "track";
}

export function nameFromFileName(fileName: string): string {
  return fileName.replace(/\.[^.]+$/, "") || "track";
}

export function slugFromPath(pathname: string): string | null {
  const raw = pathname.replace(/^\//, "").replace(/\/$/, "");
  if (!raw || raw.includes("/")) return null;
  const slug = normalizeSlug(raw);
  return isValidSlug(slug) ? slug : null;
}
