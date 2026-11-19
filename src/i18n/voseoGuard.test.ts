import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { findVoseoInText, textHasVoseo } from "./voseoGuard";

const root = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));

describe("voseo matcher", () => {
  it("flags common voseo imperatives and present forms", () => {
    const bad = [
      "Creá tu cuenta",
      "Iniciá sesión",
      "Elegí un club",
      "permití instalar",
      "Escaneá el QR",
      "esperá aprobación",
      "Ya podés ingresar",
      "¿Ya tenés cuenta?",
      "Usá este enlace",
      "Confirmá tu correo",
      "hablá con vos",
    ];
    for (const s of bad) {
      expect(textHasVoseo(s), `expected voseo in: ${s}`).toBe(true);
    }
  });

  it("allows neutral tú forms and allowlisted words", () => {
    const ok = [
      "Crea tu cuenta",
      "Inicia sesión",
      "Elige un club",
      "permite instalar",
      "Escanea el QR",
      "espera aprobación",
      "Ya puedes ingresar",
      "¿Ya tienes cuenta?",
      "Usa este enlace",
      "Confirma tu correo",
      "También está aquí después del país — más o menos, quizás jamás demás atrás, inglés e interés.",
      "El correo ya está registrado",
    ];
    for (const s of ok) {
      expect(findVoseoInText(s), `unexpected voseo in: ${s}`).toEqual([]);
    }
  });

  it("catalogs, HTML fallbacks, and mail templates have no voseo", () => {
    const paths: string[] = [
      path.join(root, "locales", "es.json"),
      ...fs
        .readdirSync(root)
        .filter((f) => f.endsWith(".html"))
        .map((f) => path.join(root, f)),
    ];

    const violations: string[] = [];
    for (const file of paths) {
      const text = fs.readFileSync(file, "utf8");
      const body = file.endsWith(".json")
        ? collectJsonStringValues(JSON.parse(text)).join("\n")
        : text;
      const hits = findVoseoInText(body);
      for (const h of hits) {
        violations.push(`${path.relative(root, file)}: «${h.match}»`);
      }
    }

    expect(violations).toEqual([]);
  });
});

function collectJsonStringValues(value: unknown): string[] {
  if (typeof value === "string") return [value];
  if (value && typeof value === "object" && !Array.isArray(value)) {
    return Object.values(value).flatMap(collectJsonStringValues);
  }
  return [];
}