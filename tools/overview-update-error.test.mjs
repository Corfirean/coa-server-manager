import assert from "node:assert/strict";
import test from "node:test";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { rolldown } from "rolldown";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

const stubs = {
  react: `export const useEffect=()=>{}; export const useCallback=f=>f; export const useRef=v=>({current:v}); export const useState=v=>[globalThis.__overviewStateCalls++===0?globalThis.__overviewStatus:v,()=>{}];`,
  "@/lib/api": `export const api={}; export const asUiError=e=>e;`,
  "@/lib/utils": `export const cn=(...s)=>s.filter(Boolean).join(' '); export const formatBytes=String; export const formatUptime=String;`,
  "@/i18n": `export const useT=()=>k=>k; export const useHuman=()=>h=>h;`,
  "@/lib/serverUpdate": `export const applyServerUpdate=async()=>false; export const useServerUpdate=()=>globalThis.__overviewUpdate;`,
  "@/lib/clientJob": `export const dismissClientJob=()=>{}; export const startClientUpdate=()=>{}; export const stopClientJob=()=>{}; export const syncClient=()=>{}; export const useClientJob=()=>({phase:'idle'});`,
  "@/lib/clientUpdate": `export const checkClient=()=>{}; export const useClientStatus=()=>({status:null});`,
  "@/lib/autoRestartPrefs": `export const useAutoRestart=()=>[false,()=>{}];`,
  "@/screens/PublishCard": `export const ListingChip=()=>null;`,
  "@/screens/PublishDialog": `export const PublishRealmDialog=()=>null;`,
  "@/lib/registry": `export const useRegistryStatus=()=>({status:null});`,
  "@/lib/publishRealms": `export const usePublishRealms=()=>[];`,
  "@/components/ui/button": `import {createElement} from 'react'; export const Button=({children,disabled,onClick,className})=>createElement('button',{disabled,onClick,className},children);`,
  "@/components/ui/card": `import {createElement} from 'react'; export const Card=({children,...props})=>createElement('div',props,children);`,
};
const bundle = await rolldown({
  input: fileURLToPath(new URL("../src/screens/Overview.tsx", import.meta.url)),
  transform: { jsx: "react-jsx" },
  external: (id, importer) => id === "react/jsx-runtime" || id === "lucide-react" || (id === "react" && !importer?.endsWith("Overview.tsx")),
  plugins: [{ name: "overview-test",
    resolveId(id, importer) {
      if (id.startsWith("@/") || (id === "react" && importer?.endsWith("Overview.tsx"))) return "\0" + id;
    },
    load(id) {
      if (!id.startsWith("\0")) return;
      const name = id.slice(1);
      return stubs[name] ?? `export const ${name.split('/').at(-1)}=()=>null;`;
    },
  }],
});
const result = await bundle.generate({ format: "cjs" });
await bundle.close();
const module = { exports: {} };
new Function("require", "module", "exports", result.output[0].code)(createRequire(import.meta.url), module, module.exports);

function overview(update) {
  globalThis.__overviewStateCalls = 0;
  globalThis.__overviewStatus = { path_exists: true, busy: false, observed: Object.fromEntries(["mysql", "auth", "world"].map(k => [k, { state: "stopped" }])) };
  globalThis.__overviewUpdate = { available: false, applying: false, preview: null, uiError: null, stateError: null, ...update };
  return renderToStaticMarkup(createElement(module.exports.Overview, { server: { id: "fixture", path: "C:/CoA_Server" }, onForget() {}, onOpenUpdates() {}, onRealmChanged() {}, onReport() {} }));
}
const failure = { human: { code: "unknown", title: "Server update failed", message: "See the technical details." }, technical: "Stop this repack before moving its folder or changing its ports." };

test("Overview displays failed updates after the progress and cached offer disappear", () => {
  const html = overview({ uiError: failure });
  assert.match(html, /role="alert"/);
  assert.match(html, /Server update failed/);
  assert.match(html, /common.viewDetails/);
  assert.match(html, /common.copyError/);
});
test("Overview displays update-state failures even without an update offer", () => {
  assert.match(overview({ stateError: failure }), /Server update failed/);
});
test("Overview does not display an update failure when there is none", () => {
  assert.doesNotMatch(overview({}), /Server update failed/);
});
