// Checks the UI translation files: every locale key must exist in en.ts, {placeholders} must match the English text,
// and the coverage of each language is reported. Exit code 1 on structural errors (missing key in en, placeholder mismatch).
// Usage: node tools/check-i18n.mjs
import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const dir = join(dirname(fileURLToPath(import.meta.url)), "..", "src", "i18n", "locales");

function load(locale) {
  const src = readFileSync(join(dir, `${locale}.ts`), "utf8");
  const map = new Map();
  // "key": "value",  — values are JSON string literals
  for (const m of src.matchAll(/^\s*("(?:[^"\\]|\\.)*")\s*:\s*("(?:[^"\\]|\\.)*")\s*,?\s*$/gm)) {
    map.set(JSON.parse(m[1]), JSON.parse(m[2]));
  }
  return map;
}

const vars = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(",");
const en = load("en");
let errors = 0;

console.log(`en: ${en.size} keys (source of truth)`);
for (const loc of ["ru", "de", "fr", "es"]) {
  const d = load(loc);
  let missing = 0;
  for (const [k, v] of d) {
    if (!en.has(k)) {
      console.error(`${loc}: key not in en.ts: ${k}`);
      errors++;
    } else if (vars(v) !== vars(en.get(k))) {
      console.error(`${loc}: placeholders differ for ${k}: {${vars(v)}} vs en {${vars(en.get(k))}}`);
      errors++;
    }
  }
  for (const k of en.keys()) if (!d.has(k)) missing++;
  const pct = ((100 * (en.size - missing)) / en.size).toFixed(1);
  console.log(`${loc}: ${en.size - missing}/${en.size} translated (${pct}%), ${missing} fall back to English`);
}
process.exit(errors ? 1 : 0);
