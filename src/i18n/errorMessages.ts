import { apiErrorMessage, t } from "./index";

const PARSE_ERROR_KEYS: Record<string, string> = {
  "gpx file is not valid xml.": "globe.errorInvalidGpx",
  "no track points found in gpx.": "globe.errorEmptyTrack",
  "no b-record fixes found in igc.": "globe.errorEmptyTrack",
  "track needs at least two points.": "globe.errorTooFewPoints",
  "at least two fixes are required.": "globe.errorTooFewPoints",
};

/**
 * Maps caught errors (API errors, parser errors, HTTP statuses, or unexpected exceptions)
 * to friendly localized messages without exposing internal codes, JSON payloads, or env vars.
 */
export function friendlyErrorMessage(error: unknown, fallbackKey = "api.genericError"): string {
  if (error instanceof Error) {
    const raw = error.message.trim();
    if (raw) {
      const lower = raw.toLowerCase();
      const parseKey = PARSE_ERROR_KEYS[lower];
      if (parseKey) return t(parseKey);
      if (lower.includes("flight_vis_password") || lower === "locked") {
        return t("globe.uploadLocked");
      }
      if (lower.includes("log in to save")) {
        return t("globe.logInToSave");
      }
      if (lower.includes("log in to manage")) {
        return t("globe.logInToManage");
      }
      const mapped = apiErrorMessage(raw);
      if (mapped !== t("api.genericError")) {
        return mapped;
      }
    }
  }
  return t(fallbackKey);
}
