import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const { createTaskFinalizer, mergeTaskView, taskBlocksSubmission } =
  require("../../../build/desktop-model-tests/task-finalization.js");

function deferred() {
  let resolve, reject;
  const promise = new Promise((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

function harness(state = "succeeded", overrides = {}) {
  let task = {
    id: "task-1", sessionId: "s1", state,
    submission: { mode: "offline", graph: { nodes: [], connections: [] }, options: {} },
  };
  let epoch = 1;
  let sessionId = "s1";
  const identity = { taskId: task.id, sessionId, epoch };
  const events = [];
  const services = {
    readCurrent: (key) => task && key.epoch === epoch && key.sessionId === sessionId &&
      task.sessionId === key.sessionId && task.id === key.taskId ? task : null,
    update: (key, patch) => {
      if (services.readCurrent(key)) task = mergeTaskView(task, { ...task, ...patch });
    },
    result: async (key) => {
      events.push("result");
      return { task_id: key.taskId, state, result: { output: "out.wav" } };
    },
    release: async (key) => {
      events.push("release");
      return { task_id: key.taskId, released: true };
    },
    ...overrides,
  };
  return {
    identity, services, events, finalizer: createTaskFinalizer(services),
    read: () => task,
    replace: (next, nextEpoch = epoch, nextSession = sessionId) => {
      task = next; epoch = nextEpoch; sessionId = nextSession;
    },
  };
}

test("complete terminal result is retained; release unblocks the next submission", async () => {
  const h = harness();
  assert.equal(taskBlocksSubmission(h.read(), "s1"), true);
  await h.finalizer.finalize(h.identity);
  assert.deepEqual(h.events, ["result", "release"]);
  assert.deepEqual(h.read().result, { output: "out.wav" });
  assert.equal(h.read().resultRead, true);
  assert.equal(h.read().released, true);
  assert.equal(h.read().cleanupBusy, false);
  assert.equal(taskBlocksSubmission(h.read(), "s1"), false);
  await h.finalizer.finalize(h.identity);
  assert.deepEqual(h.events, ["result", "release"]);
});

test("poll, cancel, and repeated cleanup share one result read and one release", async () => {
  const result = deferred(), release = deferred(), enteredRelease = deferred();
  const h = harness();
  h.services.result = async () => { h.events.push("result"); return result.promise; };
  h.services.release = async () => { h.events.push("release"); enteredRelease.resolve(); return release.promise; };
  const first = h.finalizer.finalize(h.identity);
  const second = h.finalizer.finalize(h.identity);
  assert.equal(first, second);
  await Promise.resolve();
  assert.equal(h.read().cleanupBusy, true);
  assert.deepEqual(h.events, ["result"]);
  result.resolve({ task_id: "task-1", state: "succeeded", result: {} });
  await enteredRelease.promise;
  assert.deepEqual(h.events, ["result", "release"]);
  assert.equal(h.finalizer.finalize(h.identity, true), first);
  release.resolve({ task_id: "task-1", released: true });
  await Promise.all([first, second]);
  assert.equal(h.read().released, true);
});

test("running, cancelling, and unknown tasks cannot be released", async () => {
  for (const state of ["running", "cancelling", "unknown"]) {
    const h = harness(state);
    await assert.rejects(h.finalizer.finalize(h.identity), /尚未确认结束/);
    assert.deepEqual(h.events, []);
    assert.equal(taskBlocksSubmission(h.read(), "s1"), true);
  }
});

test("failed and cancelled outcomes are fully read even without a result payload", async () => {
  for (const state of ["failed", "cancelled"]) {
    const h = harness(state);
    h.services.result = async () => {
      h.events.push("result");
      return { task_id: "task-1", state, errors: state === "failed" ? [{ message: "failed" }] : undefined };
    };
    await h.finalizer.finalize(h.identity);
    assert.deepEqual(h.events, ["result", "release"]);
    assert.equal(h.read().state, state);
    assert.equal(h.read().resultRead, true);
    assert.equal(h.read().released, true);
  }
});

test("missing or mismatched terminal results never release the task", async () => {
  for (const reply of [
    { task_id: "task-1", state: "succeeded" },
    { task_id: "task-1", state: "succeeded", result: null },
    { task_id: "other", state: "succeeded", result: {} },
    { task_id: "task-1", state: "running", result: {} },
    { task_id: "task-1", state: "failed", result: {} },
  ]) {
    const h = harness("succeeded", { result: async () => reply });
    await assert.rejects(h.finalizer.finalize(h.identity), /不完整或不匹配/);
    assert.deepEqual(h.events, []);
    assert.match(h.read().cleanupError, /收尾失败/);
    assert.notEqual(h.read().resultRead, true);
    assert.equal(h.read().cleanupBusy, false);
    assert.equal(taskBlocksSubmission(h.read(), "s1"), true);
  }
});

test("record persistence failure retains result and blocks until an explicit cleanup retry", async () => {
  const h = harness();
  h.services.release = async () => { h.events.push("release-failed"); throw new Error("disk full"); };
  await assert.rejects(h.finalizer.finalize(h.identity), /disk full/);
  assert.equal(h.read().state, "succeeded");
  assert.deepEqual(h.read().result, { output: "out.wav" });
  assert.equal(h.read().resultRead, true);
  assert.equal(taskBlocksSubmission(h.read(), "s1"), true);
  await assert.rejects(h.finalizer.finalize(h.identity), /disk full/);
  assert.deepEqual(h.events, ["result", "release-failed"]);
  h.services.release = async () => { h.events.push("release-retry"); return { task_id: "task-1", released: true }; };
  await h.finalizer.finalize(h.identity, true);
  assert.deepEqual(h.events, ["result", "release-failed", "release-retry"]);
  assert.equal(h.read().cleanupError, undefined);
  assert.equal(taskBlocksSubmission(h.read(), "s1"), false);
});

test("result read failure is retryable and never triggers an early release", async () => {
  const h = harness();
  h.services.result = async () => { h.events.push("result-failed"); throw new Error("read failed"); };
  await assert.rejects(h.finalizer.finalize(h.identity), /read failed/);
  assert.deepEqual(h.events, ["result-failed"]);
  h.services.result = async () => { h.events.push("result-retry"); return { task_id: "task-1", state: "succeeded", result: {} }; };
  await h.finalizer.finalize(h.identity, true);
  assert.deepEqual(h.events, ["result-failed", "result-retry", "release"]);
});

test("release must explicitly acknowledge the matching task", async () => {
  for (const reply of [{}, { task_id: "task-1", released: false }, { task_id: "other", released: true }]) {
    const h = harness("succeeded", { release: async () => reply });
    await assert.rejects(h.finalizer.finalize(h.identity), /未确认任务已收尾/);
    assert.notEqual(h.read().released, true);
    assert.equal(taskBlocksSubmission(h.read(), "s1"), true);
    assert.match(h.read().cleanupError, /收尾失败/);
  }
});

test("a late result from a disconnected session cannot release or change a new task with the same id", async () => {
  const result = deferred();
  const h = harness("succeeded", { result: async () => result.promise });
  const pending = h.finalizer.finalize(h.identity);
  await Promise.resolve();
  const next = { ...h.read(), sessionId: "s2", state: "running", cleanupBusy: false };
  h.replace(next, 2, "s2");
  result.resolve({ task_id: "task-1", state: "succeeded", result: {} });
  await pending;
  assert.equal(h.read(), next);
  assert.deepEqual(h.events, []);
  assert.equal(taskBlocksSubmission(next, "s2"), true);
});

test("a late release cannot clear or mark a replacement task as released", async () => {
  const release = deferred(), enteredRelease = deferred();
  const h = harness("succeeded", { release: async () => { enteredRelease.resolve(); return release.promise; } });
  const pending = h.finalizer.finalize(h.identity);
  await enteredRelease.promise;
  const next = { ...h.read(), id: "task-2", state: "running", released: false, cleanupBusy: false };
  h.replace(next);
  release.resolve({ task_id: "task-1", released: true });
  await pending;
  assert.equal(h.read(), next);
  assert.equal(taskBlocksSubmission(next, "s1"), true);
});

test("a late cleanup error from an old epoch cannot overwrite current state", async () => {
  const release = deferred(), enteredRelease = deferred();
  const h = harness("succeeded", { release: async () => { enteredRelease.resolve(); return release.promise; } });
  const pending = h.finalizer.finalize(h.identity);
  await enteredRelease.promise;
  const next = { ...h.read(), cleanupError: undefined, cleanupBusy: false };
  h.replace(next, 2);
  release.reject(new Error("old session failed"));
  await pending;
  assert.equal(h.read(), next);
  assert.equal(h.read().cleanupError, undefined);
});

test("terminal result and release flags cannot regress on late status or cancel responses", () => {
  const h = harness();
  const ended = { ...h.read(), result: { output: "out.wav" }, resultRead: true, released: true, cleanupBusy: false };
  assert.equal(mergeTaskView(ended, { ...h.read(), state: "running" }), ended);
  assert.equal(mergeTaskView(ended, { ...h.read(), state: "unknown" }), ended);
  const late = mergeTaskView(ended, { ...h.read(), resultRead: false, released: false, cleanupBusy: true });
  assert.deepEqual(late.result, ended.result);
  assert.equal(late.resultRead, true);
  assert.equal(late.released, true);
  assert.equal(late.cleanupBusy, false);
});

test("old-session task views never block a new backend session", () => {
  const h = harness("unknown");
  assert.equal(taskBlocksSubmission(h.read(), "s2"), false);
  assert.equal(taskBlocksSubmission(h.read(), undefined), false);
});
