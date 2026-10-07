import assert from "node:assert/strict";
import test from "node:test";
import { isUpdateCurrent } from "../src/lib/serverUpdateStatus.ts";

const same = { from_version: "1", to_version: "1", items: [{ action: "skip" }], migrations: 10 };

test("same files and version cannot hide pending database repairs", () => {
  assert.equal(isUpdateCurrent({ ...same, pending_migrations: 1 }), false);
  assert.equal(isUpdateCurrent({ ...same, pending_migrations: 0 }), true);
});
test("unknown history is not presented as up to date", () => {
  assert.equal(isUpdateCurrent(same), false);
  assert.equal(isUpdateCurrent({ ...same, migrations: 0 }), true);
});
test("file changes and new releases remain available with no pending SQL", () => {
  assert.equal(isUpdateCurrent({ ...same, pending_migrations: 0, items: [{ action: "replace" }] }), false);
  assert.equal(isUpdateCurrent({ ...same, pending_migrations: 0, to_version: "2" }), false);
});
