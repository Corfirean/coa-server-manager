import assert from "node:assert/strict";
import test from "node:test";
import { registerHooks } from "node:module";

let applyFailure = false;
let stateFailure = false;
let pending = null;
let serverRunning = true;
const calls = [];
globalThis.__hotfixApi = {
  status: async () => ({ observed: Object.fromEntries(["mysql", "auth", "world"].map(key => [key, { state: serverRunning ? "running" : "stopped" }])) }),
  stop: async () => { calls.push("stop"); },
  applyUpdate: async () => { calls.push("apply"); if (applyFailure) throw new Error("SQL failure"); return { txn: { state: "committed", to_version: "2" } }; },
  pendingUpdate: async () => { calls.push("disk"); if (stateFailure) throw new Error("damaged journal"); return pending; },
  checkUpdate: async () => ({ from_version: "2", to_version: "2", items: [], conflicts: [], migrations: 1, pending_migrations: 0 }),
};
registerHooks({ resolve(specifier, context, next) {
  if (specifier === "react") return { shortCircuit: true, url: "data:text/javascript,export const useSyncExternalStore=(subscribe,getSnapshot)=>getSnapshot();export const useState=()=>{throw new Error('React state is not used in this test');};" };
  if (specifier === "@/lib/api") return { shortCircuit: true, url: "data:text/javascript,export const api=globalThis.__hotfixApi;export const asUiError=e=>({human:{code:'unknown'},technical:e.message});" };
  if (specifier === "@tauri-apps/api/event") return { shortCircuit: true, url: "data:text/javascript,export const listen=async()=>()=>{};" };
  if (specifier === "./serverUpdateStatus" && context.parentURL?.endsWith("/serverUpdate.ts")) return { shortCircuit: true, url: new URL("../src/lib/serverUpdateStatus.ts", import.meta.url).href };
  if (specifier === "./updatePrefs" && context.parentURL?.endsWith("/serverUpdate.ts")) return { shortCircuit: true, url: new URL("../src/lib/updatePrefs.ts", import.meta.url).href };
  return next(specifier, context);
} });
const { applyServerUpdate, refreshServerUpdateState, checkServerUpdate, useServerUpdate } = await import("../src/lib/serverUpdate.ts");

test("a failed update rereads its unfinished transaction from disk", async () => {
  calls.length = 0;
  applyFailure = true;
  pending = { id: "interrupted", state: "failed", message: "Recovery required" };
  assert.equal(await applyServerUpdate("failed-case", { choices: {} }), false);
  assert.deepEqual(calls, ["apply", "disk"]);
  assert.equal(await refreshServerUpdateState("failed-case"), pending);
});
test("unreadable state blocks another apply until a successful reread", async () => {
  calls.length = 0;
  applyFailure = false;
  stateFailure = true;
  await applyServerUpdate("corrupt-case", { choices: {} });
  assert.equal(await applyServerUpdate("corrupt-case", { choices: {} }), false);
  assert.deepEqual(calls, ["apply", "disk"]);
  stateFailure = false;
  pending = null;
  assert.equal(await refreshServerUpdateState("corrupt-case"), null);
  assert.equal(await applyServerUpdate("corrupt-case", { choices: {} }), true);
  assert.deepEqual(calls, ["apply", "disk", "disk", "apply", "disk"]);
});
test("refresh after recovery returns the latest disk state, then its absence", async () => {
  pending = { id: "recovery", state: "failed", message: "Restore failed at database verification" };
  assert.equal(await refreshServerUpdateState("recovery-case"), pending);
  pending = null;
  assert.equal(await refreshServerUpdateState("recovery-case"), null);
});

test("a stopped server is left stopped after a successful update by default", async () => {
  calls.length = 0;
  serverRunning = false;
  applyFailure = false;
  stateFailure = false;
  pending = null;
  try {
    assert.equal(await applyServerUpdate("stopped-case", { choices: {} }), true);
    assert.deepEqual(calls, ["apply", "stop", "disk"]);
  } finally { serverRunning = true; }
});

test("a rejected recheck removes an earlier cached update offer", async () => {
  const original = globalThis.__hotfixApi.checkUpdate;
  try {
    globalThis.__hotfixApi.checkUpdate = async () => ({ from_version: "0.261005.22", to_version: "0.261005.24", items: [], migrations: 1, pending_migrations: 1 });
    await checkServerUpdate("stale-offer");
    assert.equal(useServerUpdate("stale-offer").available, true);
    globalThis.__hotfixApi.checkUpdate = async () => { throw new Error("Installed version is newer than package"); };
    await checkServerUpdate("stale-offer");
    assert.equal(useServerUpdate("stale-offer").available, false);
    assert.equal(useServerUpdate("stale-offer").preview, null);
  } finally { globalThis.__hotfixApi.checkUpdate = original; }
});
