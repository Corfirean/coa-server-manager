// Drives the real Tauri window over WebView2's DevTools protocol. Usage: node cdp.mjs <expression>
const port = process.env.CDP_PORT ?? "9222";
const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = targets.find((t) => t.type === "page");
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((r) => (ws.onopen = r));
let id = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const d = JSON.parse(m.data);
  if (d.id && pending.has(d.id)) pending.get(d.id)(d);
};
export function evaluate(expression) {
  return new Promise((resolve) => {
    const n = ++id;
    pending.set(n, resolve);
    ws.send(JSON.stringify({ id: n, method: "Runtime.evaluate", params: { expression, awaitPromise: true, returnByValue: true } }));
  });
}
if (process.argv[2] && process.argv[2] !== "-") {
  const r = await evaluate(process.argv[2]);
  console.log(JSON.stringify(r.result?.result?.value ?? r.result, null, 2));
  ws.close();
}

export async function screenshot(file) {
  const r = await new Promise((resolve) => {
    const n = ++id;
    pending.set(n, resolve);
    ws.send(JSON.stringify({ id: n, method: "Page.captureScreenshot", params: { format: "png" } }));
  });
  (await import("node:fs")).writeFileSync(file, Buffer.from(r.result.data, "base64"));
}
if (process.argv[3]) {
  await screenshot(process.argv[3]);
  ws.close();
}
