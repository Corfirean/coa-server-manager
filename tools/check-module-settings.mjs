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
for (const key of ["CoAContentScaling.LFG.AllowPartialGroups", "CoAContentScaling.World.Leech.Enable"]) {
  if (moduleField(key, "ru")?.type !== "bool") throw new Error(`${key}: expected a toggle`);
}
const leech = moduleField("CoAContentScaling.World.Leech.Percent", "ru");
if (leech?.type !== "float" || leech.min !== 0 || leech.max !== 100 || leech.advanced) throw new Error("Life steal must use a visible percentage field from 0 to 100");
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
let squidSource = readFileSync(new URL("src/lib/squidSettings.ts", root), "utf8");
squidSource = squidSource.replace('import { moduleField } from "@/lib/moduleSettings";', `import { moduleField } from "data:text/javascript;base64,${Buffer.from(compiled).toString("base64")}";`);
const { squidSettingsView } = await import(`data:text/javascript;base64,${Buffer.from(stripTypeScriptTypes(squidSource)).toString("base64")}`);
const squidSample = [
  { key: "AiPlayerbot.Enabled", value: "1", default: "0", doc: "" },
  { key: "PlayerbotsDatabaseInfo", value: "private connection", default: "", doc: "" },
  { key: "Playerbots.Updates.EnableDatabases", value: "0", default: "1", doc: "" },
  { key: "AiPlayerbot.MaxRandomBots", value: "600", default: "500", doc: "" },
  { key: "AiPlayerbot.RandomBotTalk", value: "0", default: "1", doc: "" },
  { key: "AiPlayerbot.FollowDistance", value: "1.5", default: "1.5", doc: "" },
  { key: "AiPlayerbot.RandomBotAccountPrefix", value: '"rndbot"', default: '"rndbot"', doc: "Account prefix" },
];
for (const locale of ["en", "ru", "de", "fr", "es", "zh"]) {
  const view = squidSettingsView(squidSample, locale);
  if (view.settings.length !== 4 || view.settings.some(s => s.key === "AiPlayerbot.Enabled" || s.key.startsWith("Playerbots"))) throw new Error("Bots must hide master switches and managed connections");
  const count = view.settings.find(s => s.key === "AiPlayerbot.MaxRandomBots");
  if (count.value !== 600 || count.default !== 500 || count.category !== "squid.population") throw new Error("SQUID population values/defaults must be numeric and preserved");
  if (view.settings.find(s => s.key === "AiPlayerbot.RandomBotTalk").value !== false) throw new Error("SQUID switches must use boolean values");
  if (view.settings.find(s => s.key === "AiPlayerbot.FollowDistance").value !== 1.5) throw new Error("SQUID decimal settings must preserve fractions");
  if (view.categories.some(c => !c.title)) throw new Error("SQUID categories must be translated");
}
console.log("SQUID Bots view: typed values, upstream defaults, categories and hidden managed settings verified");

const upstream = squidSettingsView([{ key: "AiPlayerbot.FutureOption", value: "2.5", default: "1", doc: "old description", field: {key:"AiPlayerbot.FutureOption",type:"float",title:"Future option",description:"From the release JSON",group:"new-group",group_title:"New activity",min:0,max:3} }], "ru");
const future = upstream.settings[0];
if (future.title !== "Future option" || future.description !== "From the release JSON" || future.type !== "float" || future.min !== 0 || future.max !== 3 || future.advanced || future.category !== "squid.new-group" || upstream.categories.find(c=>c.id===future.category)?.title !== "New activity") throw new Error("New upstream JSON options must appear without handwritten definitions");
console.log("Upstream metadata: new fields, titles, ranges and arbitrary groups verified");
