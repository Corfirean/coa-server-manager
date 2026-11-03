import assert from "node:assert/strict";
import test from "node:test";
import { updateCheckQueue } from "../src/lib/updateCheckQueue.ts";

test("duplicate checks share one result and foreground waits for background on the same server", async () => {
  const check = updateCheckQueue();
  let release;
  let calls = 0;
  const background = check("server", "background", () => { calls++; return new Promise(resolve => { release = resolve; }); });
  assert.equal(check("server", "background", () => { throw Error("duplicate"); }), background);
  const foreground = check("server", "foreground", async () => { calls++; return "full preview"; });
  await Promise.resolve();
  await Promise.resolve();
  assert.equal(calls, 1);
  release("background preview");
  assert.equal(await background, "background preview");
  assert.equal(await foreground, "full preview");
  assert.equal(calls, 2);
});

test("one server's failure releases its queue and does not block other servers", async () => {
  const check = updateCheckQueue();
  let fail;
  const first = check("one", "a", () => new Promise((_, reject) => { fail = reject; }));
  const failed = assert.rejects(first, /offline/);
  const next = check("one", "b", async () => "retry");
  assert.equal(await check("two", "a", async () => "independent"), "independent");
  fail(Error("offline"));
  await failed;
  assert.equal(await next, "retry");
  assert.equal(await check("one", "a", async () => "recovered"), "recovered");
});
