export type MessageTree = Record<string, string | MessageTree>;

export function flattenMessages(tree: MessageTree, prefix = ""): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [key, value] of Object.entries(tree)) {
    if (key === "_meta") continue;
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === "string") {
      out[path] = value;
    } else if (value && typeof value === "object") {
      Object.assign(out, flattenMessages(value, path));
    }
  }
  return out;
}

export function unflattenKey(full: string): string[] {
  return full.split(".");
}