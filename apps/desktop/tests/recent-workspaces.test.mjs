import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
const require = createRequire(import.meta.url);
const { createRecentWorkspaces, workspaceDisplayPath, workspaceFolderName } = require("../../../build/desktop-model-tests/recent-workspaces.js");
const Panel = require("../../../build/desktop-model-tests/components/WorkspacePanel.js").default;
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const services = (overrides = {}) => ({ list: async () => ({ paths: [] }), remember: async () => ({ paths: ["C:/opened"] }), remove: async () => ({ paths: [] }), clear: async () => ({ paths: [] }), ...overrides });

test("late initial list never overwrites a successfully opened workspace", async () => {
  const read = deferred();
  const controller = createRecentWorkspaces(services({ list: () => read.promise }));
  const initialize = controller.initialize();
  await controller.remember("session-1");
  read.resolve({ paths: ["C:/old"] });
  await initialize;
  assert.deepEqual(controller.getSnapshot().paths, ["C:/opened"]);
});
test("history mutations serialize and suppress duplicate remove clicks", async () => {
  const write = deferred(); let removals = 0; const sessions = [];
  const controller = createRecentWorkspaces(services({
    remove: () => { removals++; return write.promise; },
    remember: async (session) => { sessions.push(session); return { paths: ["C:/new"] }; },
  }));
  const removing = controller.remove("C:/old");
  await controller.remove("C:/old");
  const remembering = controller.remember("active-session");
  assert.equal(controller.getSnapshot().busy, true);
  write.resolve({ paths: [] });
  await Promise.all([removing, remembering]);
  assert.equal(removals, 1);
  assert.deepEqual(sessions, ["active-session"]);
  assert.deepEqual(controller.getSnapshot().paths, ["C:/new"]);
  assert.equal(controller.getSnapshot().busy, false);
});
test("persistence failure is a warning and does not reject successful connect", async () => {
  const controller = createRecentWorkspaces(services({ remember: async () => { throw new Error("disk full"); } }));
  await assert.doesNotReject(controller.remember("connected-session"));
  assert.match(controller.getSnapshot().warning, /disk full/);
  assert.equal(controller.getSnapshot().busy, false);
});
test("list initialization is once and never remembers or opens a workspace", async () => {
  let reads = 0; let remembers = 0;
  const controller = createRecentWorkspaces(services({ list: async () => { reads++; return { paths: ["C:/missing"] }; }, remember: async () => { remembers++; return { paths: [] }; } }));
  await controller.initialize(); await controller.initialize();
  assert.equal(reads, 1); assert.equal(remembers, 0);
  assert.deepEqual(controller.getSnapshot().paths, ["C:/missing"]);
});
function panel(overrides = {}, aiBusy = false) {
  const session = { desktop: true, busy: null, connection: null, workspace: "", setWorkspace() {}, recentWorkspaces: { paths: [], busy: false, warning: "", remove() {}, clear() {} }, ...overrides };
  return renderToStaticMarkup(React.createElement(Panel, { session, aiBusy, onClose() {}, onChoose() {}, onConnect() {}, onDisconnect() {}, onSettings() {} }));
}
test("panel shows empty state, named directories and human readable canonical paths", () => {
  assert.match(panel(), /暂无最近工作区/);
  const path = "\\\\?\\C:\\audio\\project";
  const html = panel({ recentWorkspaces: { paths: [path], busy: false, warning: "", remove() {}, clear() {} } });
  assert.match(html, /<strong>project<\/strong>/);
  assert.match(html, /title="C:\\audio\\project"/);
  assert.doesNotMatch(html, /<details/);
  assert.equal(workspaceDisplayPath("\\\\?\\UNC\\server\\share"), "\\\\server\\share");
  assert.equal(workspaceFolderName(path), "project");
});
test("opening history is disabled while connected, busy, AI active, or history pending", () => {
  const recentWorkspaces = { paths: ["C:/missing"], busy: false, warning: "", remove() {}, clear() {} };
  for (const [overrides, ai] of [[{ connection: { sessionId: "s" } }, false], [{ busy: "选择目录中" }, false], [{}, true], [{ recentWorkspaces: { ...recentWorkspaces, busy: true } }, false]]) {
    const html = panel({ recentWorkspaces, ...overrides }, ai);
    assert.match(html, /class="recent-workspace-open"[^>]*disabled=""/);
  }
});

test("workspace input displays friendly paths without changing the stored source", () => {
  for (const [raw, display] of [
    [String.raw`\\?\C:\AAAProject\Temp\TestSwitch`, String.raw`C:\AAAProject\Temp\TestSwitch`],
    [String.raw`\\?\UNC\server\share\音频`, String.raw`\\server\share\音频`],
  ]) {
    const source = {workspace: raw, connection:{sessionId:"s"}};
    const html = panel(source);
    assert.ok(html.includes(`value="${display}"`));
    assert.ok(!html.includes(raw));
    assert.equal(source.workspace, raw);
  }
});
test("failed or missing directory remains visible for retry and removal", () => {
  const html = panel({ error: "directory missing", recentWorkspaces: { paths: ["C:/missing"], busy: false, warning: "", remove() {}, clear() {} } });
  assert.match(html, /C:\/missing/); assert.match(html, /移除/);
  assert.doesNotMatch(html, /class="recent-workspace-open"[^>]*disabled/);
});

test("corrupt empty history offers explicit clear recovery", () => {
  const html = panel({ recentWorkspaces: { paths: [], busy: false, warning: "记录损坏", remove() {}, clear() {} } });
  assert.match(html, /清空记录/);
  assert.match(html, /role="alert">记录损坏/);
});
