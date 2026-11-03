import assert from "node:assert/strict";
import test from "node:test";
import { registerHooks } from "node:module";

let handler;
let enabled = true;
let closeToTray = false;
let nextUpdate = null;
const calls = [];
globalThis.__closeTest = {
  invoke: async (cmd, args) => { calls.push([cmd, args]); return cmd === "close_to_tray" ? closeToTray : undefined; },
  window: { onCloseRequested: async cb => { handler = cb; return () => { handler = null; }; } },
  check: async () => nextUpdate,
};
globalThis.localStorage = { getItem: () => enabled ? "on" : "off", setItem() {} };
registerHooks({ resolve(id, context, next) {
  const stubs = {
    react: "export const useSyncExternalStore=(subscribe,read)=>read();",
    "@tauri-apps/api/core": "export const invoke=(...a)=>globalThis.__closeTest.invoke(...a);",
    "@tauri-apps/api/window": "export const getCurrentWindow=()=>globalThis.__closeTest.window;",
    "@tauri-apps/plugin-updater": "export const check=()=>globalThis.__closeTest.check();",
    "@tauri-apps/plugin-process": "export const relaunch=async()=>{};",
  };
  if (id in stubs) return { shortCircuit:true, url:"data:text/javascript,"+encodeURIComponent(stubs[id]) };
  return next(id, context);
}});
const { installOnClose, lookForUpdate } = await import("../src/lib/selfUpdate.ts");
async function close() {
  const stop = installOnClose();
  await Promise.resolve();
  let prevented = false;
  await handler({ preventDefault() { prevented = true; } });
  assert.equal(prevented, true, "Tauri must never auto-destroy the hidden main window");
  stop();
}

test("ordinary close delegates to native preferences without destroying the window", async () => {
  calls.length = 0;
  await close();
  assert.deepEqual(calls, [["close_manager", undefined]]);
});

test("a cached update with automatic updates off still uses native close preferences", async () => {
  nextUpdate = { version:"99", download:async()=>{}, install:async()=>{throw new Error("should not install")} };
  await lookForUpdate({download:true});
  enabled=false;calls.length=0;
  await close();
  assert.deepEqual(calls, [["close_manager", undefined]]);
});

test("failed automatic installation releases exit mode and preserves ordinary close handling", async () => {
  enabled=true;calls.length=0;
  await close();
  assert.deepEqual(calls, [["close_to_tray",undefined],["manager_installing",{enabled:true}],["manager_installing",{enabled:false}],["close_manager",undefined]]);
});

test("minimizing to tray never starts a cached automatic installation", async () => {
  await lookForUpdate({download:true});
  closeToTray=true;calls.length=0;
  await close();
  assert.deepEqual(calls, [["close_to_tray",undefined],["close_manager",undefined]]);
});
