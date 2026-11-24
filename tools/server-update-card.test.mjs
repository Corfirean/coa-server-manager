import assert from "node:assert/strict";
import test from "node:test";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { rolldown } from "rolldown";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

// The Server card in Settings -> Updates, rendered with a tiny hook runtime so that its effects really run (a plain
// server render skips them, which is where the offer used to be wiped when the tab was opened after an update attempt
// had already finished). The card must show what the shared update state, which the main screen also uses, holds.
const stubs = {
  react: `export const useState=(...a)=>globalThis.__hooks.useState(...a); export const useEffect=(...a)=>globalThis.__hooks.useEffect(...a);`,
  "@/lib/api": `export const api={pendingUpdate:async()=>null}; export const asUiError=e=>e;`,
  "@tauri-apps/plugin-dialog": `export const open=async()=>null;`,
  "@/i18n": `export const useI18n=()=>({t:(k,v)=>v?k+JSON.stringify(v):k}); export const useHuman=()=>h=>h; export const hasKey=()=>false;`,
  "@/lib/serverUpdate": `export const applyServerUpdate=async()=>false; export const clearServerUpdateResult=()=>{}; export const refreshServerUpdateState=async()=>null; export const getServerUpdateRevision=()=>0; export const useServerUpdate=()=>globalThis.__cardUpdate;`,
  "@/lib/updatePrefs": `export const useStartAfterUpdate=()=>[false,()=>{}];`,
  "@/components/ui/button": `import {createElement} from 'react'; export const Button=({children,disabled,onClick,className,...props})=>createElement('button',{disabled,onClick,className,...props},children);`,
  "@/components/ui/card": `import {createElement} from 'react'; export const Card=({children,...props})=>createElement('div',props,children);`,
};
const card = fileURLToPath(new URL("../src/screens/ServerUpdateCard.tsx", import.meta.url));
const bundle = await rolldown({
  input: card,
  transform: { jsx: "react-jsx" },
  external: (id, importer) => id === "react/jsx-runtime" || id === "lucide-react" || (id === "react" && !importer?.endsWith("ServerUpdateCard.tsx")),
  plugins: [{ name: "card-test",
    resolveId(id, importer) {
      if (id === "@/lib/serverUpdateStatus") return fileURLToPath(new URL("../src/lib/serverUpdateStatus.ts", import.meta.url));
      if (id.startsWith("@/") || id.startsWith("@tauri-apps/") || (id === "react" && importer?.endsWith("ServerUpdateCard.tsx"))) return "\0" + id;
    },
    load(id) {
      if (!id.startsWith("\0")) return;
      return stubs[id.slice(1)] ?? `export const ${id.split("/").at(-1)}=()=>null;`;
    },
  }],
});
const result = await bundle.generate({ format: "cjs" });
await bundle.close();
const module = { exports: {} };
new Function("require", "module", "exports", result.output[0].code)(createRequire(import.meta.url), module, module.exports);

const offer = { from_version: "0.261116.11201", to_version: "0.261119.11401", items: [{ path: "bin/worldserver.exe", action: "replace" }], conflicts: [], migrations: 0, download_bytes: 5 << 20 };
async function render(update) {
  globalThis.__cardUpdate = { preview: null, available: false, checking: false, applying: false, step: null, percent: 0, error: null, uiError: null, stateError: null, stateRevision: 0, result: null, ...update };
  const slots = [], deps = [];
  let index = 0, effects = [], dirty = false;
  globalThis.__hooks = {
    useState(initial) {
      const i = index++;
      if (!(i in slots)) slots[i] = typeof initial === "function" ? initial() : initial;
      return [slots[i], (next) => { const value = typeof next === "function" ? next(slots[i]) : next; if (!Object.is(value, slots[i])) { slots[i] = value; dirty = true; } }];
    },
    useEffect(fn, list) {
      const i = index++;
      const before = deps[i];
      if (!before || !list || list.some((d, k) => !Object.is(d, before[k]))) effects.push(fn);
      deps[i] = list;
    },
  };
  let html = "";
  for (let pass = 0; pass < 10; pass++) {
    index = 0; effects = []; dirty = false;
    html = renderToStaticMarkup(createElement(module.exports.ServerUpdateCard, { serverId: "fixture", onHistory() {}, onAttention() {} }));
    effects.forEach((fn) => fn());
    await new Promise((resolve) => setTimeout(resolve, 0));
    if (!dirty) break;
  }
  return html.replaceAll("&quot;", '"');
}

test("Settings card shows the installed version and the update button from the shared offer", async () => {
  const html = await render({ preview: offer, available: true });
  assert.match(html, /settings\.version\{"v":"0\.261116\.11201"\}/);
  assert.match(html, /upd\.availableFrom/);
  assert.match(html, /upd\.now/);
});

test("Settings card keeps the offer after an update attempt already finished (it is not wiped on open)", async () => {
  const html = await render({ preview: offer, available: true, stateRevision: 3 });
  assert.match(html, /settings\.version\{"v":"0\.261116\.11201"\}/);
  assert.match(html, /upd\.now/);
});

test("Settings card shows an up-to-date server as current, without an update button", async () => {
  const same = { ...offer, to_version: offer.from_version, items: [{ path: "bin/worldserver.exe", action: "skip" }], pending_migrations: 0 };
  const html = await render({ preview: same });
  assert.match(html, /upd\.current/);
  assert.doesNotMatch(html, /upd\.now/);
});

test("Settings card shows a placeholder while no check has finished", async () => {
  const html = await render({ checking: true });
  assert.doesNotMatch(html, /settings\.version/);
  assert.doesNotMatch(html, /upd\.now/);
});
