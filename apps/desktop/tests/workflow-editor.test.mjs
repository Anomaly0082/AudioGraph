import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

const require = createRequire(import.meta.url);
const {
  blankWorkflowText,
  canRunWorkflow,
  createWorkflowSnapshot,
  isCurrentWorkflowSnapshot,
  sameWorkflowContext,
  workflowRelativePath,
  workflowRunSucceeded,
} = require("../../../build/desktop-model-tests/workflow-editor-model.js");
const Page =
  require("../../../build/desktop-model-tests/pages/WorkflowEditorPage.js").default;

function snapshot(overrides = {}) {
  return createWorkflowSnapshot({
    sessionId: "session-a",
    contextKey: "workspace-a",
    epoch: 1,
    revision: 3,
    text: blankWorkflowText,
    ...overrides,
  });
}

test("Workflow execution retains the exact text snapshot while the next draft changes", () => {
  const draft = { ...snapshot() };
  const captured = createWorkflowSnapshot(draft);
  draft.text = '{"schema_version":1,"steps":[{"id":"next"}]}';
  draft.revision += 1;
  assert.equal(captured.text, blankWorkflowText);
  assert.equal(Object.isFrozen(captured), true);
  assert.equal(isCurrentWorkflowSnapshot(captured, draft), false);
  assert.equal(
    sameWorkflowContext(captured, draft),
    true,
    "history may report the submitted run without validating the next draft",
  );
  assert.throws(() => {
    captured.text = "changed";
  }, TypeError);
});

test("validation belongs to exact text, revision, session, workspace and epoch", () => {
  const validated = snapshot();
  assert.equal(isCurrentWorkflowSnapshot(validated, snapshot()), true);
  for (const changed of [
    { text: "{}" },
    { revision: 4 },
    { sessionId: "session-b" },
    { contextKey: "workspace-b" },
    { epoch: 2 },
  ]) {
    assert.equal(
      isCurrentWorkflowSnapshot(validated, snapshot(changed)),
      false,
    );
  }
  assert.equal(isCurrentWorkflowSnapshot(validated, null), false);
  assert.equal(
    sameWorkflowContext(validated, snapshot({ epoch: 2 })),
    false,
    "a response from an earlier session lifetime must stay stale even after reconnecting",
  );
});

test("Workflow cannot run without current successful validation or while another operation owns the workspace", () => {
  const current = snapshot();
  assert.equal(canRunWorkflow(current, null, false, false), false);
  assert.equal(canRunWorkflow(null, current, false, false), false);
  assert.equal(canRunWorkflow(current, current, false, false), true);
  assert.equal(
    canRunWorkflow(current, snapshot({ revision: 2 }), false, false),
    false,
  );
  assert.equal(
    canRunWorkflow(
      current,
      snapshot({ sessionId: "session-old" }),
      false,
      false,
    ),
    false,
  );
  assert.equal(canRunWorkflow(current, current, true, false), false);
  assert.equal(canRunWorkflow(current, current, false, true), false);
});

test("file inputs use relative workspace paths", () => {
  assert.equal(
    workflowRelativePath(" workflows\\new.workflow.json "),
    "workflows/new.workflow.json",
  );
  for (const path of [
    "",
    "../escape.json",
    "dir/../escape.json",
    "C:\\absolute.json",
    "/absolute.json",
    "\\\\server\\file.json",
  ]) {
    assert.throws(() => workflowRelativePath(path), /相对文件路径/);
  }
});

test("only authoritative succeeded state counts as a successful Workflow run", () => {
  assert.equal(workflowRunSucceeded("succeeded"), true);
  for (const state of [
    "failed",
    "cancelled",
    "limited",
    "interrupted",
    "unknown",
  ]) {
    assert.equal(workflowRunSucceeded(state), false);
  }
});

const noop = () => {};
function render(editorOverrides = {}, propsOverrides = {}) {
  const editor = {
    text: blankWorkflowText,
    fileLabel: "未保存的 Workflow",
    sourceSpace: "user",
    path: "workflows/new.workflow.json",
    action: null,
    busy: false,
    error: "",
    notice: "",
    lastRunId: null,
    lastState: null,
    validationCurrent: false,
    editText: noop,
    setSourceSpace: noop,
    setPath: noop,
    load: noop,
    save: noop,
    resetBlank: noop,
    validate: noop,
    run: noop,
    stop: noop,
    ...editorOverrides,
  };
  return renderToStaticMarkup(
    React.createElement(Page, {
      editor,
      connected: true,
      locked: false,
      onOpenRun: noop,
      ...propsOverrides,
    }),
  );
}

test("Workflow page exposes explicit JSON file controls and requires validation before run", () => {
  const html = render();
  assert.match(html, /Workflow 相对文件路径/);
  assert.match(html, /Workflow 文件工作区/);
  assert.match(html, /另存为新文件/);
  assert.match(html, /内部 Graph 在 AI 工作区运行/);
  assert.match(html, /<textarea[^>]*aria-label="Workflow JSON"/);
  assert.match(
    html,
    /<button class="primary" disabled="">运行 Workflow<\/button>/,
  );
  assert.doesNotMatch(html, /<details|<summary|API Key|apiKey|结果面板/);
  assert.match(
    render({ validationCurrent: true }),
    /<button class="primary">运行 Workflow<\/button>/,
  );
  assert.match(
    render({ validationCurrent: true }, { locked: true }),
    /<button class="primary" disabled="">运行 Workflow<\/button>/,
  );
});

test("running Workflow keeps next draft editable and stopping stays visibly busy", () => {
  const running = render({
    action: "running",
    busy: true,
    validationCurrent: false,
  });
  assert.match(running, /<button>停止<\/button>/);
  assert.doesNotMatch(running.match(/<textarea[^>]*>/)?.[0] ?? "", /disabled/);
  assert.match(running, /<button disabled="">校验<\/button>/);
  const stopping = render({ action: "stopping", busy: true });
  assert.match(stopping, /<button disabled="">停止<\/button>/);
  assert.match(stopping, /正在停止并清理/);
  assert.doesNotMatch(stopping.match(/<textarea[^>]*>/)?.[0] ?? "", /disabled/);
});

test("completed Workflow links to existing run history and save notice makes no run claim", () => {
  const completed = render({ lastRunId: "run-a", lastState: "succeeded" });
  assert.match(completed, /查看运行记录/);
  assert.match(completed, /上次运行：成功/);
  const saved = render({ notice: "已另存为新文件。" });
  assert.match(saved, /已另存为新文件/);
  assert.doesNotMatch(saved, /上次运行|查看运行记录|运行已结束/);
});
