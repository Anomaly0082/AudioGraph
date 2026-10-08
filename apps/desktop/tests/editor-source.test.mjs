import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import vm from "node:vm";

const require = createRequire(import.meta.url);
const { editorModified, editorWorkspaceContext, transitionEditorWorkspace, sameGraphContent, savedEditorSource, sourceForContext, workspaceRelativeSavePath } = require("../../../build/desktop-model-tests/editor-source-model.js");

function source(overrides = {}) {
  return { context: "session-a/workspace-a", path: "graph.json", revision: "r1", content: "{}", ...overrides };
}

test("modified reflects actual content and returning to the saved snapshot clears it", () => {
  assert.equal(editorModified(source(), "{}"), false);
  assert.equal(editorModified(source(), "{\"nodes\":[]}"), true);
  assert.equal(editorModified(source(), "{}"), false);
  assert.equal(editorModified(null, "{}"), true);
});

test("saving advances revision using captured content while later edits remain modified", () => {
  const saved = savedEditorSource("a", "a", "graph.json", "r2", "captured");
  assert.equal(saved.revision, "r2");
  assert.equal(editorModified(saved, "later edit"), true);
  assert.equal(editorModified(saved, "captured"), false);
});

test("stale save responses and previous workspace destinations cannot bind", () => {
  assert.equal(savedEditorSource("a", "b", "old.json", "r2", "{}"), null);
  assert.equal(sourceForContext(source(), "session-b/workspace-a"), null);
  assert.equal(sourceForContext(source(), "session-a/workspace-b"), null);
  assert.deepEqual(sourceForContext(source(), "session-a/workspace-a"), source());
});

test("Save As establishes the new destination and reset/import remains unbound", () => {
  const saved = savedEditorSource("a", "a", "new.json", "new-revision", "{}", "ai");
  assert.equal(saved.path, "new.json");
  assert.equal(saved.space, "ai");
  assert.equal(sourceForContext(null, "a"), null);
  assert.equal(editorModified(null, saved.content), true);
});

test("browser association requires fresh file content, allowing native object-key reordering", () => {
  assert.equal(sameGraphContent({ nodes: [{ id: "a", type: "node" }], schema_version: 1 }, { schema_version: 1, nodes: [{ type: "node", id: "a" }] }), true);
  assert.equal(sameGraphContent({ nodes: [{ id: "a" }] }, { nodes: [{ id: "b" }] }), false);
  assert.equal(sameGraphContent({ nodes: ["a", "b"] }, { nodes: ["b", "a"] }), false);
});

test("save picker paths accept ordinary/verbatim Windows roots while preserving file spelling", () => {
  assert.equal(workspaceRelativeSavePath("\\\\?\\C:\\Project", "c:\\project\\Workflows\\New.json"), "Workflows/New.json");
  assert.equal(workspaceRelativeSavePath("C:\\Project", "\\\\?\\c:\\PROJECT\\New.json"), "New.json");
  assert.equal(workspaceRelativeSavePath("\\\\?\\UNC\\Server\\Share\\Project", "\\\\server\\share\\project\\New.json"), "New.json");
  assert.equal(workspaceRelativeSavePath("\\\\Server\\Share\\Project", "\\\\?\\UNC\\server\\share\\project\\New.json"), "New.json");
  assert.throws(() => workspaceRelativeSavePath("C:\\Project", "C:\\Project-sibling\\new.json"), /工作区内/);
  assert.throws(() => workspaceRelativeSavePath("C:\\Project", "C:\\Project"), /工作区内/);
});

test("remembered ownership survives disconnect and canonical Windows reconnect, never resurrecting after another workspace", () => {
  for (const [original, equivalent] of [
    ["C:\\Project", "\\\\?\\c:\\PROJECT\\"],
    ["\\\\Server\\Share\\Project", "\\\\?\\UNC\\server\\share\\PROJECT"],
  ]) {
    let state = transitionEditorWorkspace({ workspace: null, scope: "" }, null, original);
    state.source = source({ context: editorWorkspaceContext(state.owner) });
    const saved = state.source;
    state = transitionEditorWorkspace(state.owner, state.source, null);
    state = transitionEditorWorkspace(state.owner, state.source, equivalent);
    assert.equal(state.source, saved);
    state = transitionEditorWorkspace(state.owner, state.source, "D:\\Other");
    assert.equal(state.source, null);
    state = transitionEditorWorkspace(state.owner, state.source, original);
    assert.equal(state.source, null);
  }
});

test("scope changes and explicit replacements while disconnected permanently unbind the remembered source", () => {
  const owner = { workspace: "c:/project", scope: "one" };
  const saved = source({ context: editorWorkspaceContext(owner) });
  assert.equal(transitionEditorWorkspace(owner, saved, null, "two").source, null);
  const closed = transitionEditorWorkspace(owner, null, null, "one");
  assert.equal(transitionEditorWorkspace(closed.owner, closed.source, "C:/Project", "one").source, null);
});

// A small synchronous hooks host runs the actual compiled hooks without a DOM or
// another test framework. Effects flush after each render, including state updates.
function hookHost(name, api) {
  const slots = [];
  let cursor = 0, dirty = false, effects = [], args = [], result;
  const react = {
    useState(initial) {
      const index = cursor++;
      if (!(index in slots)) slots[index] = typeof initial === "function" ? initial() : initial;
      return [slots[index], (value) => {
        const next = typeof value === "function" ? value(slots[index]) : value;
        if (!Object.is(next, slots[index])) { slots[index] = next; dirty = true; }
      }];
    },
    useRef(initial) {
      const index = cursor++;
      if (!(index in slots)) slots[index] = { current: initial };
      return slots[index];
    },
    useMemo(compute) { cursor++; return compute(); },
    useEffect(effect, deps) {
      const index = cursor++;
      const previous = slots[index];
      if (!previous || deps.some((value, i) => !Object.is(value, previous[i]))) {
        slots[index] = deps;
        effects.push(effect);
      }
    },
  };
  const filename = fileURLToPath(new URL(`../../../build/desktop-model-tests/hooks/${name}.js`, import.meta.url));
  const localRequire = createRequire(filename);
  const exports = {};
  vm.runInThisContext(`(function(require, exports, setTimeout, clearTimeout) { ${readFileSync(filename, "utf8")}\n})`, { filename })(
    (id) => id === "react" ? react : id === "../api/desktop" ? api : id === "./useRecentWorkspaces" ? { useRecentWorkspaces: () => ({ remember: api.remember ?? (async () => {}) }) } : localRequire(id), exports, api.setTimeout ?? setTimeout, api.clearTimeout ?? clearTimeout,
  );
  function render(...nextArgs) {
    if (nextArgs.length) args = nextArgs;
    do {
      dirty = false; cursor = 0; effects = [];
      result = exports[name](...args);
      for (const effect of effects) effect();
    } while (dirty);
    return result;
  }
  return { render };
}

const deferred = () => {
  let resolve;
  const promise = new Promise((yes) => { resolve = yes; });
  return { promise, resolve };
};
const connection = (sessionId = "a", workspace = "C:\\Project") => ({ sessionId, workspace });

function sessionHost(overrides = {}) {
  const calls = [], history = [];
  let listener, serial = 0;
  const api = {
    desktopAvailable: () => true,
    listenBackendDisconnected: async (callback) => { listener = callback; return () => {}; },
    confirmDesktop: async (...args) => { calls.push(["confirm", ...args]); return true; },
    connectDesktop: async (workspace, allowDevices, allowMonitor) => { calls.push(["connect", workspace, allowDevices, allowMonitor]); return { workspace, allowDevices, allowMonitor, sessionId: `session-${++serial}` }; },
    disconnectDesktop: async (id) => { calls.push(["disconnect", id]); listener({ sessionId: id, message: "正常退出" }); return { forced: false, message: "正常退出" }; },
    remember: async (id) => { history.push(id); },
    requireSuccess: (reply) => reply,
    controlRequest: async (_id, request) => { calls.push(["rpc", request]); return { task_id: "manual", state: "running" }; },
    ...overrides,
  };
  return { ...hookHost("useAudioSession", api), calls, history, emit: (event) => listener(event) };
}

test("restored permissions match connection flags and never execute audio or tasks", async () => {
  for (const permissions of [{ allowDevices: false, allowMonitor: false }, { allowDevices: true, allowMonitor: false }, { allowDevices: true, allowMonitor: true }]) {
    const host = sessionHost(); let session = host.render();
    await session.applyPermissions(permissions);
    assert.deepEqual(host.calls, []);
    // No intervening render required: startup uses the synchronous permissions ref.
    await session.connectBackend("C:\\Project"); session = host.render();
    assert.equal(session.allowDevices, permissions.allowDevices);
    assert.equal(session.allowMonitor, permissions.allowMonitor);
    assert.deepEqual(host.calls, [["connect", "C:\\Project", permissions.allowDevices, permissions.allowMonitor]]);
  }
});

test("restored Graph retains raw draft and saved SHA baseline through same-root reconnect", () => {
  const host = hookHost("useGraphDraft", {}); let draft = host.render(null, "");
  const restored = { text: "{ incomplete", mode: "streaming", blockFrames: "invalid", duration: "", canvasView: { positions: {}, selectedId: "missing", zoom: 1, pan: { x: 0, y: 0 } }, source: { workspace: "C:\\Project", path: "C:\\Project\\original.json", revision: "a".repeat(64), content: "original baseline", space: "user" } };
  draft.restoreSessionState(restored, "C:\\Project"); draft = host.render(null, "");
  assert.equal(draft.graphText, "{ incomplete"); assert.equal(draft.blockFrames, "invalid"); assert.equal(draft.validationCurrent, false);
  draft = host.render("new-session", "c:/project");
  assert.equal(draft.exportSessionState().source.revision, "a".repeat(64));
  assert.equal(draft.exportSessionState().source.content, "original baseline");
  assert.equal(draft.modified, true);
  draft = host.render("other-session", "C:\\Other"); assert.equal(draft.exportSessionState().source, null);
});
test("restored Workflow retains raw text, filename, AI owner and baseline through reconnect", () => {
  const host = hookHost("useWorkflowEditor", {}); let editor = host.render(null, () => false);
  const value = { text: "unfinished Workflow", sourceSpace: "ai", path: "flows/original.json", source: { workspace: "C:\\Project", path: "flows/original.json", revision: "b".repeat(64), content: "saved baseline", space: "ai" } };
  editor.restoreSessionState(value, "C:\\Project"); editor = host.render(null, () => false);
  assert.equal(editor.path, "flows/original.json"); assert.equal(editor.validationCurrent, false);
  editor = host.render(connection("restored-session", "c:/project"), () => false);
  assert.equal(editor.text, value.text); assert.equal(editor.path, value.path);
  assert.equal(editor.exportSessionState().source.revision, value.source.revision);
  assert.equal(editor.exportSessionState().source.content, "saved baseline");
  assert.equal(editor.exportSessionState().source.space, "ai");
});

test("permissions update disconnected flags atomically; no-op skips reconnect", async () => {
  const host = sessionHost(); let session = host.render();
  await session.applyPermissions({ allowDevices: true, allowMonitor: true });
  session = host.render();
  assert.equal(session.allowMonitor, true); assert.equal(host.calls.length, 0);
  await session.connectBackend("C:\\Project"); session = host.render();
  await session.applyPermissions({ allowDevices: true, allowMonitor: true });
  assert.equal(host.calls.length, 1);
  await session.applyPermissions({ allowDevices: false, allowMonitor: true }); session = host.render();
  assert.deepEqual(host.calls.slice(1), [["disconnect", "session-1"], ["connect", "C:\\Project", false, false]]);
  assert.equal(session.connection.sessionId, "session-2"); assert.equal(session.allowMonitor, false);
  assert.equal(session.error, ""); assert.equal(host.history.length, 2);
});

test("active manual task cancellation retains flags and connection; confirmation locks rapid toggles", async () => {
  const dialog = deferred();
  const host = sessionHost({ confirmDesktop: () => dialog.promise }); let session = host.render();
  await session.connectBackend("C:\\Project"); session = host.render();
  await session.startSubmission({ mode: "offline", graph: { nodes: [], edges: [] }, options: {} });
  const update = session.applyPermissions({ allowDevices: true, allowMonitor: true });
  await assert.rejects(session.applyPermissions({ allowDevices: false, allowMonitor: false }), /请等待/);
  assert.equal(session.allowDevices, false);
  dialog.resolve(false); await update;
  assert.equal(session.connection.sessionId, "session-1"); assert.equal(session.allowDevices, false);
  assert.equal(host.calls.filter((entry) => entry[0] === "disconnect").length, 0);
});

test("disconnect completion is a barrier and later old events cannot disconnect the new session", async () => {
  const cleanup = deferred(); const host = sessionHost({ disconnectDesktop: () => cleanup.promise });
  let session = host.render(); await session.connectBackend("C:\\Project"); session = host.render();
  const update = session.applyPermissions({ allowDevices: true, allowMonitor: false });
  assert.equal(host.calls.length, 1); assert.equal(session.allowDevices, false);
  cleanup.resolve({ forced: true, message: "清理完成" }); await update; session = host.render();
  assert.deepEqual(host.calls[1], ["connect", "C:\\Project", true, false]);
  host.emit({ sessionId: "session-1", message: "迟到退出", forced: true }); session = host.render();
  assert.equal(session.connection.sessionId, "session-2"); assert.equal(session.error, "");
  assert.match(session.forcedWarning, /强制/);
});

test("unknown manual task explicitly prompts before reconnect and never auto-submits", async () => {
  const host = sessionHost({ controlRequest: async () => ({ task_id: "uncertain", state: "unknown" }) });
  let session = host.render(); await session.connectBackend("C:\\Project"); session = host.render();
  await assert.rejects(session.startSubmission({ mode: "offline", graph: { nodes: [], edges: [] }, options: {} }), /未知任务状态/);
  await session.applyPermissions({ allowDevices: true, allowMonitor: true }); session = host.render();
  assert.match(host.calls.find((entry) => entry[0] === "confirm")[1], /停止并取消.*输出文件会保留/);
  assert.equal(host.calls.filter((entry) => entry[0] === "connect").length, 2);
  assert.equal(host.calls.filter((entry) => entry[0] === "rpc").length, 0);
  assert.equal(session.task.state, "unknown"); assert.equal(session.error, "");
});

test("closing reconnect suppresses old finalizer errors and new terminal polling responses", async () => {
  const timers = [], cleanup = deferred(); let rejectResult;
  const result = new Promise((_resolve, reject) => { rejectResult = reject; });
  let statusCalls = 0;
  const host = sessionHost({
    setTimeout: (callback) => { timers.push(callback); return timers.length; }, clearTimeout: () => {},
    disconnectDesktop: () => cleanup.promise,
    controlRequest: async (_session, request) => {
      if (request.op === "tasks.start") return { task_id: "manual", state: "running" };
      if (request.op === "tasks.status") { statusCalls++; return { task_id: "manual", state: "succeeded" }; }
      if (request.op === "tasks.result") return result;
      throw new Error("must not release during reconnect");
    },
  });
  let session = host.render(); await session.connectBackend("C:\\Project"); session = host.render();
  await session.startSubmission({ mode: "offline", graph: { nodes: [], edges: [] }, options: {} }); session = host.render();
  const polling = timers.shift()(); await Promise.resolve(); await Promise.resolve();
  const updating = session.applyPermissions({ allowDevices: true, allowMonitor: false }); await Promise.resolve();
  rejectResult(new Error("old backend gone")); await polling;
  cleanup.resolve({ forced: false, message: "done" }); await updating; session = host.render();
  assert.equal(statusCalls, 1); assert.equal(session.error, ""); assert.equal(session.task.cleanupError, undefined);
});

test("polling retries after transport failure during an unsuccessful disconnect", async () => {
  const timers = []; let rejectDisconnect;
  const cleanup = new Promise((_resolve, reject) => { rejectDisconnect = reject; });
  let statusCalls = 0;
  const host = sessionHost({
    setTimeout: (callback) => { timers.push(callback); return timers.length; }, clearTimeout: () => {},
    disconnectDesktop: () => cleanup,
    controlRequest: async (_session, request) => {
      if (request.op === "tasks.status" && statusCalls++ === 0) throw new Error("closing transport");
      return { task_id: "manual", state: "running" };
    },
  });
  let session = host.render(); await session.connectBackend("C:\\Project"); session = host.render();
  await session.startSubmission({ mode: "offline", graph: { nodes: [], edges: [] }, options: {} }); session = host.render();
  const updating = session.applyPermissions({ allowDevices: true, allowMonitor: false }); await Promise.resolve();
  await timers.shift()(); assert.equal(timers.length, 1);
  rejectDisconnect(new Error("disconnect rejected")); await assert.rejects(updating, /disconnect rejected/);
  await timers.shift()(); session = host.render();
  assert.equal(statusCalls, 2); assert.equal(session.task.state, "running");
  assert.equal(session.connection.sessionId, "session-1"); assert.equal(session.allowDevices, false);
});

test("busy AI guard rejects before any disconnect; changed workspace rejects late reconnect", async () => {
  const pending = deferred(); let attempts = 0;
  const host = sessionHost({ connectDesktop: async (workspace, allowDevices, allowMonitor) => {
    if (attempts++) return pending.promise;
    return { workspace, sessionId: "old", allowDevices, allowMonitor };
  } }); let session = host.render(); await session.connectBackend("C:\\Project"); session = host.render();
  await assert.rejects(session.applyPermissions({ allowDevices: true, allowMonitor: false }, () => true), /AI/);
  assert.equal(host.calls.length, 0);
  const update = session.applyPermissions({ allowDevices: true, allowMonitor: false });
  await Promise.resolve(); session.setWorkspace("D:\\Other");
  pending.resolve({ workspace: "C:\\Project", sessionId: "new", allowDevices: true, allowMonitor: false });
  await assert.rejects(update, /连接或工作区已变化/); session = host.render();
  assert.equal(session.connection, null); assert.equal(session.workspace, "D:\\Other");
});

test("disconnect failure keeps original permissions; reconnect failure keeps requested retry state", async () => {
  const host = sessionHost({ disconnectDesktop: async () => { throw new Error("disconnect failed"); } }); let session = host.render();
  await session.connectBackend("C:\\Project"); session = host.render();
  await assert.rejects(session.applyPermissions({ allowDevices: true, allowMonitor: false }), /disconnect failed/);
  session = host.render(); assert.equal(session.connection.sessionId, "session-1"); assert.equal(session.allowDevices, false);
  let attempts = 0;
  const failing = sessionHost({ connectDesktop: async (workspace, allowDevices, allowMonitor) => {
    if (attempts++) throw new Error("connect failed");
    return { sessionId: "old", workspace, allowDevices, allowMonitor };
  } }); session = failing.render();
  await session.connectBackend("C:\\Project"); session = failing.render();
  await assert.rejects(session.applyPermissions({ allowDevices: true, allowMonitor: true }), /connect failed/);
  session = failing.render(); assert.equal(session.connection, null); assert.equal(session.allowMonitor, true);
  assert.equal(session.workspace, "C:\\Project"); assert.equal(failing.history.length, 1);
});

test("actual Graph hook keeps dirty source through reconnect and rejects late save association", async () => {
  const model = require("../../../build/desktop-model-tests/model.js");
  const graph = model.createTemplate("text").graph;
  const pending = deferred();
  const writes = [];
  const host = hookHost("useGraphDraft", {
    chooseGraphFile: async () => "C:/Project/original.json",
    confirmDesktop: async () => true,
    loadGraphDesktop: async () => ({ graph, path: "C:/Project/original.json", revision: "r1" }),
    chooseNewGraphFile: async () => { throw new Error("Unexpected Save As"); },
    saveGraphDesktop: (...args) => { writes.push(args); return pending.promise; },
  });
  let hook = host.render("a", "C:\\Project");
  assert.equal(await hook.loadGraph(connection()), true);
  hook = host.render();
  assert.equal(hook.modified, false);
  hook.editGraph(`${hook.graphText}\n`);
  hook = host.render();
  let oldValidationKey = hook.validationKey;
  assert.equal(hook.acceptValidation("a", oldValidationKey), true);
  const saving = hook.saveGraph(connection());
  hook.editGraph(`${hook.graphText}\n`);
  hook = host.render();
  oldValidationKey = hook.validationKey;
  assert.equal(hook.acceptValidation("a", oldValidationKey), true);
  hook = host.render(null, "");
  assert.equal(hook.modified, true);
  assert.equal(await hook.saveGraph(connection()), null);
  hook = host.render("b", "\\\\?\\c:\\PROJECT");
  pending.resolve({ path: "C:/Project/original.json", revision: "r2" });
  await saving;
  hook = host.render();
  assert.equal(hook.fileLabel, "C:/Project/original.json");
  assert.equal(hook.modified, true);
  assert.equal(hook.probe, true);
  assert.equal(hook.validationCurrent, false);
  await hook.saveGraph(connection("b", "\\\\?\\c:\\PROJECT"));
  assert.equal(writes[1][3], "r1", "old completion must not advance the revision after reconnect");
  hook = host.render(null, "");
  hook = host.render("a", "C:\\Project");
  assert.equal(hook.acceptValidation("a", oldValidationKey), false, "even reused session tokens must reject pre-disconnect validation");
  assert.equal(hook.isCurrentDraftKey(oldValidationKey), false);
  hook = host.render(null, "");
  hook.replaceGraph(hook.graphText);
  hook = host.render("c", "C:/Project");
  assert.equal(hook.modified, true);
  assert.equal(hook.fileLabel, "未保存的 Graph");
});

test("actual Workflow hook retains AI origin despite selector changes and rejects stale save on reconnect", async () => {
  const pending = deferred();
  const writes = [];
  const host = hookHost("useWorkflowEditor", {
    confirmDesktop: async () => true,
    invokeDesktop: async (command, payload) => {
      if (command === "workflow_editor_load") return { space: "ai", path: "original.json", revision: "r1", text: "{}" };
      if (command === "workflow_editor_save") { writes.push(payload); return pending.promise; }
      throw new Error(`Unexpected I/O: ${command}`);
    },
  });
  let hook = host.render(connection(), () => false);
  assert.equal(await hook.loadFromFile("ai", "original.json", "{}", () => true), true);
  hook = host.render();
  hook.editText("{\"edited\":true}");
  hook.setSourceSpace("user"); hook.setPath("selector.json");
  hook = host.render();
  const saving = hook.save();
  hook.editText("{\"edited\":\"while saving\"}");
  hook = host.render(null, () => false);
  assert.equal(hook.fileLabel, "ai: original.json");
  assert.equal(hook.path, "selector.json");
  assert.equal(hook.modified, true);
  hook = host.render(connection("b", "\\\\?\\C:\\PROJECT"), () => false);
  pending.resolve({ space: "ai", path: "original.json", revision: "r2" });
  await saving;
  hook = host.render();
  assert.equal(hook.modified, true);
  await hook.save();
  assert.equal(writes[1].space, "ai");
  assert.equal(writes[1].path, "original.json");
  assert.equal(writes[1].expectedRevision, "r1");
  hook = host.render(null, () => false);
  assert.equal(await hook.save(), false);
  assert.equal(await hook.run(), false);
  assert.equal(hook.applyText("{}"), true);
  hook = host.render(connection("c"), () => false);
  assert.equal(hook.modified, true);
  assert.equal(hook.path, "");
});
