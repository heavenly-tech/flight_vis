import { DEMO_FLIGHT_SLUG } from "./demoFlight";

export type PublicLiveFlight = { flightId: string; tail: string };

export type PublicLiveSummary = { flights: PublicLiveFlight[] };

export function isFullGlobeEntry(pathname: string, search: string): boolean {
  const norm = pathname.replace(/\/+$/, "") || "/";
  if (norm === "/map") return true;
  return new URLSearchParams(search).get("all") === "1";
}

export function isSiteRootPath(pathname: string): boolean {
  const norm = pathname.replace(/\/+$/, "") || "/";
  return norm === "/" || norm === "/index.html";
}

export function publicLiveSummaryUrl(club: string | null): string {
  if (club) {
    return `/api/public/live-summary?club=${encodeURIComponent(club)}`;
  }
  return "/api/public/live-summary";
}

function clubQuery(club: string | null, leading: "?" | "&"): string {
  if (!club) return "";
  return `${leading}club=${encodeURIComponent(club)}`;
}

/** Replace-navigation target for `/` or `/demo` when the anonymous root should not show the full globe. */
export function resolvePublicRootRedirect(
  summary: PublicLiveSummary,
  club: string | null,
): string | null {
  const { flights } = summary;
  if (flights.length === 1) {
    const id = encodeURIComponent(flights[0].flightId);
    return `/club-flight.html?id=${id}${clubQuery(club, "&")}`;
  }
  if (flights.length > 1) {
    return `/public-map.html${clubQuery(club, "?")}`;
  }
  return null;
}

export function demoFallbackPath(): string {
  return `/${DEMO_FLIGHT_SLUG}`;
}