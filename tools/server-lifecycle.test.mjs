import test from "node:test";
import assert from "node:assert/strict";
import { ServerLifecycle, serverLifecycle } from "../src/lib/serverLifecycle.ts";

test("polls during a slow deliberate stop never rearm crash restart", () => {
  const state = new ServerLifecycle();
  state.observe(true, false, false);
  state.request("stopping");
  for (let i = 0; i < 50; i++) assert.equal(state.observe(true, true, false), false);
  assert.equal(state.observe(false, false, false), false);
  assert.equal(state.observe(false, false, false), false);
  state.request("starting");
  state.complete("starting");
  state.observe(true, false, false);
  assert.equal(state.observe(false, false, false), true);
  assert.equal(state.observe(false, false, false), false);
});

test("update validation and rollback shutdowns are not crashes", () => {
  const state = new ServerLifecycle();
  state.observe(true, false, false);
  assert.equal(state.observe(false, false, true), false);
  assert.equal(state.observe(false, false, false), false);
  state.observe(true, false, false);
  assert.equal(state.observe(false, false, true), false);
  state.observe(true, false, false);
  assert.equal(state.observe(false, false, false), true);
});

test("navigation retains a pending stop and a later external start rearms detection", () => {
  const state = serverLifecycle("navigation");
  state.observe(true, false, false);
  state.request("stopping");
  const returned = serverLifecycle("navigation");
  assert.equal(returned, state);
  assert.equal(returned.observe(false, false, false), false);
  returned.complete("stopping");
  assert.equal(returned.observe(false, false, false), false);
  returned.observe(true, false, false);
  assert.equal(returned.observe(false, false, false), true);
});

test("an update between status polls cannot look like a crash after completion", () => {
  const state = new ServerLifecycle();
  state.observe(true, false, false);
  state.beginMaintenance();
  state.endMaintenance();
  assert.equal(state.observe(false, false, false), false);
  state.observe(true, false, false);
  assert.equal(state.observe(false, false, false), true);
});


test("an intentional restart remains suppressed across navigation until the action completes", () => {
  const state = serverLifecycle("restart-navigation");
  state.observe(true, false, false);
  state.request("restarting");
  const returned = serverLifecycle("restart-navigation");
  assert.equal(returned.observe(false, false, false), false);
  assert.equal(returned.observe(true, false, false), false);
  returned.complete("restarting");
  returned.observe(true, false, false);
  assert.equal(returned.observe(false, false, false), true);
});
