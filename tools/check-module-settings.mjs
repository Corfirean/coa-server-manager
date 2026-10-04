// Validate the module controls independently of the server/bot schema overlays.
import { readFileSync } from "node:fs";
import { stripTypeScriptTypes } from "node:module";
const root = new URL("../", import.meta.url);
let source = readFileSync(new URL("src/lib/moduleSettings.ts", root), "utf8");
source = source.replace(/import (\w+) from "(\.\.\/\.\.\/[^"\n]+\.json)";/g, (_, name, path) => {
  const data = JSON.parse(readFileSync(new URL(path, new URL("src/lib/", root)), "utf8"));
  return `const ${name} = ${JSON.stringify(data)};`;
});
const compiled = stripTypeScriptTypes(source);
const { moduleSettingDefinitions, moduleField, canonicalModuleValue, auctionTypeChanges, auctionCategorySettings, auctionCategories, auctionQualities } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}`);
const ids = new Set();
for (const field of moduleSettingDefinitions) {
  if (ids.has(field.key)) throw new Error(`Duplicate module setting: ${field.key}`);
  ids.add(field.key);
  for (const [label, translations] of [["title", field.title], ["description", field.description], ...(field.options ?? []).map(o => [o.value, o.label])]) {
    if (translations.length !== 6 || translations.some(t => !t?.trim())) throw new Error(`${field.key}: incomplete ${label}`);
  }
  if (field.min !== undefined && field.max !== undefined && field.min > field.max) throw new Error(`${field.key}: invalid range`);
}
for (const [value, expected] of [["0", "None"], ["1", "Light"], ["2", "Full"], ['"light"', "Light"]]) {
  if (canonicalModuleValue("CoAContentScaling.SoloAssist.Mode", value) !== expected) throw new Error(`Solo mode alias: ${value}`);
}
const difficulty = moduleField("CoAContentScaling.Difficulty.DamageMultiplier", "ru");
if (difficulty.type !== "range" || difficulty.min !== 0.25 || difficulty.max !== 2) throw new Error("Damage slider bounds disagree with the server");
const sample = [
  { key: "AuctionHouseBot.ListProportion.CategoryWeapon.QualityNormal", value: "20", default: "10" },
  { key: "AuctionHouseBot.ListProportion.CategoryWeapon.QualityRare", value: "7", default: "5" },
  { key: "AuctionHouseBot.ListProportion.CategoryArmor.QualityNormal", value: "50", default: "50" },
];
const weapon = auctionCategorySettings(sample, "Weapon");
const off = auctionTypeChanges(weapon, false);
if (Object.keys(off).length !== 2 || Object.values(off).some(v => v !== "0")) throw new Error("Turning off a type must exclude all its qualities, and only that type");
const remembered = Object.fromEntries(weapon.map(item => [item.key, item.value]));
if (JSON.stringify(auctionTypeChanges(weapon, true, remembered)) !== JSON.stringify(remembered)) throw new Error("Custom weights must survive off/on");
const defaults = auctionTypeChanges(weapon, true);
if (defaults[weapon[0].key] !== "10" || defaults[weapon[1].key] !== "5") throw new Error("Enabling a type must restore documented weights");
const zero = weapon.map(item => ({...item, value: "0", default: "0"}));
const enabledZero = auctionTypeChanges(zero, true);
if (enabledZero[weapon[0].key] !== "1" || enabledZero[weapon[1].key] !== "0") throw new Error("Zero-default types must become enabled without adding missing keys");
for (const label of [...Object.values(auctionCategories), ...Object.values(auctionQualities)]) if (label.length !== 6 || label.some(t => !t)) throw new Error("Incomplete auction type/quality translation");
console.log(`${ids.size} module controls: all six languages, unique keys, ranges and legacy aliases verified`);
