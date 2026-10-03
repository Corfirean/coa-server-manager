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
for (const loc of ["ru", "de", "fr", "es", "zh"]) {
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
// Settings-schema overlays (src/i18n/schema/<locale>.json) must cover every setting, category, option and preset.
const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const schemas = ["bots", "server"].map((sc) => ({ sc, d: JSON.parse(readFileSync(join(root, "schemas", `${sc}.json`), "utf8")), p: JSON.parse(readFileSync(join(root, "schemas", "presets", `${sc}.json`), "utf8")).presets }));
for (const loc of ["ru", "de", "fr", "es", "zh"]) {
  const o = JSON.parse(readFileSync(join(root, "src", "i18n", "schema", `${loc}.json`), "utf8"));
  let miss = 0;
  for (const { sc, d, p } of schemas) {
    for (const c of d.categories) if (!o.categories[`${sc}.${c.id}`]) miss++;
    for (const s of d.settings) {
      if (!o.settings[s.key]) miss++;
      for (const opt of s.options ?? []) if (!o.options[`${s.key}.${opt.value}`]) miss++;
    }
    for (const x of p) if (!o.presets[x.id]) miss++;
  }
  console.log(`${loc}: schema overlay ${miss === 0 ? "complete" : miss + " entries missing"}`);
  if (miss) errors++;
}
process.exit(errors ? 1 : 0);
