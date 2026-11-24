/**
 * Helper to extract a trimmed non-empty message string from release data,
 * or null if the message is absent, empty, or not a string.
 */
export function releaseNoteText(data: { message?: unknown }): string | null {
  const raw = data.message;
  if (typeof raw !== "string") {
    return null;
  }
  const trimmed = raw.trim();
  return trimmed === "" ? null : trimmed;
}
