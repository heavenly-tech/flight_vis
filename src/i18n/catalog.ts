import es from "../../locales/es.json";
import en from "../../locales/en.json";
import manifest from "../../locales/manifest.json";
import { flattenMessages, type MessageTree } from "./flatten";

export type LocaleId = keyof typeof manifest.locales;

export const CANONICAL_LOCALE = (manifest.canonical ?? "es") as LocaleId;

const bundled: Record<string, MessageTree> = { es, en };

export function bundledLocaleIds(): string[] {
  return Object.keys(manifest.locales);
}

export function flattenCatalog(locale: string): Record<string, string> {
  const tree = bundled[locale] ?? bundled[CANONICAL_LOCALE];
  return flattenMessages(tree as MessageTree);
}

export function localeBcp47(locale: string): string {
  const entry = manifest.locales[locale as LocaleId];
  return entry?.bcp47 ?? locale;
}

export function localeLabel(locale: string): string {
  const entry = manifest.locales[locale as LocaleId];
  return entry?.label ?? locale;
}