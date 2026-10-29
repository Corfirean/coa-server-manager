import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { releasesForUpdate, localizeNotes } from "../src/lib/releaseHistoryModel.ts";

const release = (version, prerelease = false) => ({ version, prerelease, notes: [], tag: version, date: "2026-10-28" });
test("intermediate update shows only versions actually being installed", () => {
  const records = [release("0.9.0"), release("0.10.0"), release("0.11.0"), release("0.10.1", true)];
  assert.deepEqual(releasesForUpdate(records, "0.9.0", "0.10.0").map(r => r.version), ["0.10.0"]);
  assert.deepEqual(releasesForUpdate(records, "0.9.0", "0.11.0").map(r => r.version), ["0.10.0", "0.11.0"]);
  assert.deepEqual(releasesForUpdate(records, "custom", "0.10.0").map(r => r.version), ["0.10.0"]);
  assert.deepEqual(releasesForUpdate(records, "0.10.0", "0.10.0"), []);
});
test("a changed or partly translated record falls back as a whole", () => {
  assert.deepEqual(localizeNotes(["Changed", "New"], { Changed: "Переведено" }), { notes: ["Changed", "New"], fallback: true });
});
test("latest published Manager and server records have complete translations in five languages", () => {
  const translations = JSON.parse(readFileSync(new URL("../src/data/latest-note-translations.json", import.meta.url)));
  for (const component of ["server", "manager"]) {
    const path = component === "server" ? "../src/data/server-history.json" : "../changelog/history.json";
    const history = JSON.parse(readFileSync(new URL(path, import.meta.url)));
    const latest = history.releases.at(-1);
    for (const locale of ["ru", "de", "fr", "es", "zh"]) {
      assert.equal(localizeNotes(latest.notes, translations[component][locale]).fallback, false, `${component}/${locale}`);
    }
  }
});
