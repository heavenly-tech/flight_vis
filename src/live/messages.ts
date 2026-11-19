import {
  bundledLocaleIds,
  CANONICAL_LOCALE,
  getLocale,
  initI18n,
  setLocale,
  t,
  type LocaleId,
} from "../i18n/index";

export type LiveLang = LocaleId;

export function detectLang(): LiveLang {
  return getLocale() as LiveLang;
}

export function setLiveLang(lang: LiveLang): void {
  void setLocale(lang);
}

export function msg(key: string, lang?: LiveLang): string {
  if (lang && lang !== getLocale()) {
    initI18n();
    void setLocale(lang);
  }
  return t(`viewer.${key}`);
}

export { bundledLocaleIds, CANONICAL_LOCALE, initI18n, t };