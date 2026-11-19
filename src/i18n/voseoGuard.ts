/**
 * Detect Argentine voseo in Spanish UI copy (neutral Chilean / tú forms only).
 */

const WORD_RE = /[A-Za-zÁÉÍÓÚÜáéíóúüñÑ]+/giu;

/** Whole-word allowlist: same spelling in any dialect, not voseo imperatives. */
export const VOSEO_ALLOWLIST = new Set([
  "está",
  "más",
  "será",
  "aquí",
  "allí",
  "país",
  "también",
  "después",
  "atrás",
  "quizás",
  "jamás",
  "demás",
  "inglés",
  "interés",
  "además",
]);

/** Canonical voseo imperatives (stressed última sílaba). */
export const VOSEO_IMPERATIVE_WORDS = [
  "permití",
  "tocá",
  "escaneá",
  "esperá",
  "elegí",
  "iniciá",
  "creá",
  "ingresá",
  "revisá",
  "probá",
  "mirá",
  "poné",
  "tené",
  "vení",
  "decí",
  "instalá",
  "descargá",
  "abrí",
  "pedí",
  "usá",
  "confirmá",
  "arrastrá",
  "copiá",
  "configurá",
  "concedé",
  "pegá",
  "unite",
  "hablá",
] as const;

const VOSEO_IMPERATIVE_SET = new Set(VOSEO_IMPERATIVE_WORDS);

/** Voseo presente (vos). */
export const VOSEO_PRESENT_WORDS = ["podés", "tenés", "querés", "sabés", "elegís", "sos"] as const;

const VOSEO_PRESENT_SET = new Set(VOSEO_PRESENT_WORDS);

export const VOSEO_IMPERATIVE_RE = new RegExp(
  `\\b(?:${VOSEO_IMPERATIVE_WORDS.join("|")})\\b`,
  "giu",
);

export const VOSEO_PRESENT_RE = new RegExp(
  `\\b(?:${VOSEO_PRESENT_WORDS.join("|")})\\b`,
  "giu",
);

export const VOSEO_PRONOUN_RE = /\bvos\b/giu;

/** Palabra entera que termina en á, é o í (imperativo voseo habitual). */
/** Imperativo voseo en á/í (evita subjuntivo «esté», «inválido», etc.). */
export const VOSEO_STRESSED_FINAL_RE = /^[A-Za-zÁÉÍÓÚÜáéíóúüñÑ]{2,}[áí]$/u;

export type VoseoHit = { match: string; index: number };

function allowlisted(word: string): boolean {
  return VOSEO_ALLOWLIST.has(word.toLowerCase());
}

function wordIsVoseo(word: string): boolean {
  const lower = word.toLowerCase();
  if (allowlisted(word)) return false;
  if (lower === "vos") return true;
  if (VOSEO_IMPERATIVE_SET.has(lower as (typeof VOSEO_IMPERATIVE_WORDS)[number])) return true;
  if (VOSEO_PRESENT_SET.has(lower as (typeof VOSEO_PRESENT_WORDS)[number])) return true;
  if (VOSEO_STRESSED_FINAL_RE.test(word)) return true;
  return false;
}

/** Returns voseo hits in `text`, excluding allowlisted words. */
export function findVoseoInText(text: string): VoseoHit[] {
  const hits: VoseoHit[] = [];
  const seen = new Set<string>();

  WORD_RE.lastIndex = 0;
  for (const m of text.matchAll(WORD_RE)) {
    const word = m[0];
    if (!wordIsVoseo(word)) continue;
    const key = `${m.index}:${word.toLowerCase()}`;
    if (seen.has(key)) continue;
    seen.add(key);
    hits.push({ match: word, index: m.index ?? 0 });
  }

  return hits.sort((a, b) => a.index - b.index);
}

export function textHasVoseo(text: string): boolean {
  return findVoseoInText(text).length > 0;
}