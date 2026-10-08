import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";
const require = createRequire(import.meta.url);
const { validateSessionState, createSessionWriter } = require("../../../build/desktop-model-tests/session-state-model.js");
function state(text = "invalid JSON") {
  return { schema_version: 1, workspace: null, page: "editor", editorKind: "workflow", graph: { text, mode: "offline", blockFrames: "NaN", duration: "", canvasView: { positions: {}, selectedId: "", zoom: 1, pan: { x: 0, y: 0 } }, source: null }, workflow: { text: "unfinished workflow", sourceSpace: "user", path: "", source: null } };
}
test("recovery preserves raw invalid drafts and rejects invalid permissions and canvas bounds", () => {
  const draft = state(); assert.equal(validateSessionState(draft), draft);
  assert.throws(() => validateSessionState({ ...draft, permissions: {} }));
  const bad = state(); bad.graph.canvasView.pan.x = Infinity; assert.throws(() => validateSessionState(bad));
  bad.graph.canvasView.pan.x = 0; bad.graph.canvasView.zoom = .24; assert.throws(() => validateSessionState(bad));
});
test("recovery accepts legacy files and roundtrips explicit audio permission choices", () => {
  assert.equal(validateSessionState(state()).permissions, undefined);
  for (const permissions of [{ allowDevices: false, allowMonitor: false }, { allowDevices: true, allowMonitor: false }, { allowDevices: true, allowMonitor: true }]) {
    const restored = validateSessionState(JSON.parse(JSON.stringify({ ...state(), permissions })));
    assert.deepEqual(restored.permissions, permissions);
  }
  for (const permissions of [null, {}, { allowDevices: true }, { allowDevices: "true", allowMonitor: false }, { allowDevices: false, allowMonitor: true }, { allowDevices: true, allowMonitor: true, execute: true }]) {
    assert.throws(() => validateSessionState({ ...state(), permissions }));
  }
});
test("source metadata excludes internal contexts and preserves original revision and baseline", () => {
  const draft = state(); draft.workspace = "C:/project";
  draft.graph.source = { workspace: "C:/project", path: "C:/project/g.json", revision: "a".repeat(64), content: "original raw content", space: "user" };
  assert.equal(validateSessionState(draft).graph.source.content, "original raw content");
  draft.graph.source.context = "internal"; assert.throws(() => validateSessionState(draft));
});
test("source owner matches verbatim drive and UNC successful connection roots", () => {
  for (const [workspace, owner] of [["\\\\?\\C:\\Project\\", "c:/project"], ["\\\\?\\UNC\\Server\\Share\\", "//server/share"]]) {
    const draft = state(); draft.workspace = workspace;
    draft.graph.source = { workspace: owner, path: `${owner}/g.json`, revision: "a".repeat(64), content: "baseline", space: "user" };
    assert.equal(validateSessionState(draft), draft);
  }
});
test("writer serializes saves and the latest pending snapshot wins", async () => {
  let release; const seen = [];
  const writer = createSessionWriter(async value => { seen.push(value.graph.text); if (seen.length === 1) await new Promise(resolve => release = resolve); });
  writer.enqueue(state("first")); const flush = writer.flush();
  writer.enqueue(state("middle")); writer.enqueue(state("latest"));
  release(); await flush; assert.deepEqual(seen, ["first", "latest"]);
});
test("writer reports failures and can subsequently save a newer snapshot", async () => {
  let failed = false; const seen = [];
  const writer = createSessionWriter(async value => { if (!failed) { failed = true; throw new Error("disk failure"); } seen.push(value.graph.text); });
  writer.enqueue(state("first")); await assert.rejects(writer.flush(), /disk failure/);
  writer.enqueue(state("retry")); await writer.flush(); assert.deepEqual(seen, ["retry"]);
});

const deferred = () => { let resolve; const promise = new Promise(done => resolve = done); return { promise, resolve }; };
const tick = async () => { for (let i = 0; i < 8; i++) await Promise.resolve(); };
function recoveryHost(overrides = {}) {
  const slots = [], effects = [], calls = [], timers = new Map();
  let cursor = 0, current = state(), close, cancel, timerId = 0;
  const react = {
    useState(initial) { const i = cursor++; if (!(i in slots)) slots[i] = initial; return [slots[i], value => slots[i] = typeof value === "function" ? value(slots[i]) : value]; },
    useRef(initial) { const i = cursor++; if (!(i in slots)) slots[i] = { current: initial }; return slots[i]; },
    useEffect(effect, deps) { const i = cursor++; if (!slots[i] || deps.some((value, j) => !Object.is(value, slots[i].deps[j]))) { slots[i] = { deps, effect }; effects.push(effect); } },
  };
  const api = {
    listenSessionClose: async callback => { close = callback; return () => {}; },
    listenSessionCloseCancelled: async callback => { cancel = callback; return () => {}; },
    invokeDesktop: async (command, args) => { calls.push([command, args]); return command === "session_state_load" ? { state: null } : command === "session_state_close_confirmation" ? true : undefined; },
    confirmDesktop: async () => false,
    ...overrides,
  };
  const filename = fileURLToPath(new URL("../../../build/desktop-model-tests/hooks/useSessionState.js", import.meta.url));
  const localRequire = createRequire(filename), exports = {};
  vm.runInThisContext(`(function(require,exports,setTimeout,clearTimeout){${readFileSync(filename, "utf8")}\n})`, { filename })(id => id === "react" ? react : id === "../api/desktop" ? api : localRequire(id), exports,
    (callback, ms) => { if (ms === 0) { queueMicrotask(callback); return 0; } const id = ++timerId; timers.set(id, callback); return id; }, id => timers.delete(id));
  function render() { cursor = 0; const value = exports.useSessionState(true, () => current, async restored => { current = restored; }); while (effects.length) effects.shift()(); return value; }
  return { render, calls, set: value => current = value, close: id => close(id), cancel: id => cancel(id), replay: () => { for (const slot of slots) if (slot?.effect) slot.effect(); }, timers: () => { const jobs = [...timers.values()]; timers.clear(); jobs.forEach(job => job()); } };
}
test("StrictMode effect replay reads once and close-before-load flushes restored draft", async () => {
  const loaded = deferred();
  const host = recoveryHost({ invokeDesktop: async (command, args) => { host.calls.push([command, args]); return command === "session_state_load" ? loaded.promise : undefined; } });
  assert.equal(host.render().ready, false); host.replay(); await tick(); host.close("first");
  assert.equal(host.calls.filter(([command]) => command === "session_state_save").length, 0);
  loaded.resolve({ state: state("restored raw draft") }); await tick(); host.render(); await tick();
  assert.equal(host.calls.filter(([command]) => command === "session_state_load").length, 1);
  assert.equal(host.calls.find(([command]) => command === "session_state_save")[1].state.graph.text, "restored raw draft");
  assert.equal(host.calls.filter(([command]) => command === "session_state_finish_close").length, 1);
});
test("native cancellation releases closing and stale pending flush cannot finish a new close", async () => {
  const saved = deferred();
  const host = recoveryHost({ invokeDesktop: async (command, args) => { host.calls.push([command, args]); return command === "session_state_load" ? { state: null } : command === "session_state_save" ? saved.promise : undefined; } });
  host.render(); await tick(); host.render(); host.close("old"); await tick();
  assert.equal(host.render().closing, true); host.cancel("old"); assert.equal(host.render().closing, false);
  saved.resolve(); await tick(); assert.equal(host.calls.filter(([command]) => command === "session_state_finish_close").length, 0);
});
test("corrupt load blocks autosave until explicit reset; save failure cancels close", async () => {
  const host = recoveryHost({ invokeDesktop: async (command, args) => { host.calls.push([command, args]); if (command === "session_state_load") throw new Error("corrupt"); if (command === "session_state_close_confirmation") return true; } });
  host.render(); await tick(); assert.equal(host.render().disabled, true); host.timers(); await tick();
  assert.equal(host.calls.filter(([command]) => command === "session_state_save").length, 0);
  host.close("failed"); await tick();
  assert.equal(host.calls.filter(([command]) => command === "session_state_cancel_close").length, 1);
  assert.equal(host.render().closing, false);
});
test("failed exit save claims one confirmation and deduplicates close replay", async () => {
  const dialog = deferred(); let confirmations = 0;
  const host = recoveryHost({
    invokeDesktop: async (command, args) => { host.calls.push([command, args]); if (command === "session_state_load") return { state: null }; if (command === "session_state_save") throw new Error("disk full"); if (command === "session_state_close_confirmation") return true; },
    confirmDesktop: () => { confirmations++; return dialog.promise; },
  });
  host.render(); await tick(); host.render(); host.close("one"); await tick(); await tick(); host.close("one"); await tick();
  assert.equal(host.calls.filter(([command]) => command === "session_state_close_confirmation").length, 1);
  assert.equal(confirmations, 1);
  dialog.resolve(false); await tick(); assert.equal(host.render().closing, false);
});
test("native confirmation owner suppresses JS dialog and native cancel releases closing", async () => {
  let confirmations = 0;
  const host = recoveryHost({
    invokeDesktop: async (command, args) => { host.calls.push([command, args]); if (command === "session_state_load") return { state: null }; if (command === "session_state_save") throw new Error("disk full"); if (command === "session_state_close_confirmation") return false; },
    confirmDesktop: async () => { confirmations++; return false; },
  });
  host.render(); await tick(); host.render(); host.close("native"); await tick(); await tick();
  assert.equal(confirmations, 0); assert.equal(host.render().closing, true);
  host.cancel("native"); assert.equal(host.render().closing, false);
  assert.equal(host.calls.filter(([command]) => command === "session_state_finish_close" || command === "session_state_cancel_close").length, 0);
});
test("explicit reset after corrupt load clears then saves the current draft and enables autosave", async () => {
  const host = recoveryHost({
    invokeDesktop: async (command, args) => { host.calls.push([command, args]); if (command === "session_state_load") throw new Error("corrupt"); },
    confirmDesktop: async () => true,
  });
  host.render(); await tick(); assert.equal(host.render().disabled, true);
  host.set(state("current unsaved raw draft")); await host.render().reset();
  assert.equal(host.render().disabled, false);
  assert.deepEqual(host.calls.map(([command]) => command), ["session_state_load", "session_state_clear", "session_state_save"]);
  assert.equal(host.calls.at(-1)[1].state.graph.text, "current unsaved raw draft");
});
test("rejected exit dialog releases native close request and a new close can save and finish", async () => {
  let failSave = true;
  const host = recoveryHost({
    invokeDesktop: async (command, args) => {
      host.calls.push([command, args]);
      if (command === "session_state_load") return { state: null };
      if (command === "session_state_save" && failSave) throw new Error("disk full");
      if (command === "session_state_close_confirmation") return true;
    },
    confirmDesktop: async () => { throw new Error("dialog unavailable"); },
  });
  host.render(); await tick(); host.render(); host.close("dialog-rejected"); await tick(); await tick();
  assert.equal(host.render().closing, false);
  assert.deepEqual(host.calls.filter(([command]) => command === "session_state_cancel_close"), [["session_state_cancel_close", { requestId: "dialog-rejected" }]]);
  failSave = false; host.close("retry"); await tick(); await tick();
  assert.deepEqual(host.calls.filter(([command]) => command === "session_state_finish_close"), [["session_state_finish_close", { requestId: "retry" }]]);
});
