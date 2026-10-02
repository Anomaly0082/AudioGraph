import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

const require = createRequire(import.meta.url);
const {
  groupRuns,
  matchesRunSelection,
} = require("../../../build/desktop-model-tests/run-record-model.js");
const Page =
  require("../../../build/desktop-model-tests/pages/RunRecordsPage.js").default;
const noop = () => {};
const summary = (id, kind = "graph", parent_id = null) => ({
  id,
  kind,
  parent_id,
  origin: "ai",
  name: id,
  started_at_ms: 1000,
  finished_at_ms: 1500,
  duration_ms: 500,
  state: "succeeded",
});
const record = {
  ...summary("parent", "workflow"),
  configuration: { snapshot: "original" },
  result: { state: "succeeded", outputs: {} },
  error: null,
  files: [
    {
      space: "ai",
      role: "output",
      path: "output.wav",
      size_bytes: 44,
      sha256: "abc",
      capture_status: "captured",
    },
  ],
};
const records = {
  records: [summary("parent", "workflow"), summary("child", "graph", "parent")],
  warnings: [],
  truncated: false,
  selectedId: "parent",
  record,
  checks: null,
  loading: false,
  checking: false,
  error: "",
  select: noop,
  refresh: noop,
  checkFiles: noop,
};
const render = (overrides = {}, props = {}) =>
  renderToStaticMarkup(
    React.createElement(Page, {
      connected: true,
      records: { ...records, ...overrides },
      ...props,
    }),
  );

test("workflow children are grouped while omitted parents do not hide records", () => {
  const child = summary("child", "graph", "parent");
  const parent = summary("parent", "workflow");
  const orphan = summary("orphan", "graph", "not-in-page");
  const result = groupRuns([child, parent, orphan]);
  assert.equal(result.roots.length, 2);
  assert.equal(result.children.get("parent")[0].id, "child");
  assert.ok(result.roots.some((r) => r.id === "orphan"));
});
test("stale file checks and detail loads are scoped to both project session and selection", () => {
  assert.equal(
    matchesRunSelection("new-session", "run", "old-session", "run"),
    false,
  );
  assert.equal(matchesRunSelection("same", "new", "same", "old"), false);
  assert.equal(matchesRunSelection("same", "run", "same", "run"), true);
});
test("run details use view buttons and do not treat an unchecked reference as available", () => {
  const html = render();
  assert.match(html, /查看子 Graph/);
  assert.match(html, /查看执行配置/);
  assert.match(html, /查看执行详情/);
  assert.match(html, /output.wav/);
  assert.match(html, /尚未检查当前文件/);
  assert.doesNotMatch(html, /与记录一致|<details|<summary/);
  assert.doesNotMatch(html, /查看旧版参数实验/);
  assert.doesNotMatch(html, /当前任务控制|释放记录/);
});

test("only the matching current manual Graph row offers stop", () => {
  const live = { ...summary("live"), origin: "manual", state: "running" };
  const currentTask = {
    runId: "live",
    state: "running",
    busy: false,
    onStop: noop,
  };
  const html = render(
    { records: [live, summary("old")], record: null },
    { currentTask },
  );
  assert.equal((html.match(/aria-label="停止 /g) ?? []).length, 1);
  assert.match(html, /aria-label="停止 live"/);
  assert.doesNotMatch(html, /当前任务控制|释放记录/);
  const stale = render(
    { records: [live], record: null },
    { currentTask: { ...currentTask, runId: "different" } },
  );
  assert.doesNotMatch(stale, /aria-label="停止 /);
  const ai = render(
    { records: [{ ...live, origin: "ai" }], record: null },
    { currentTask },
  );
  assert.doesNotMatch(ai, /aria-label="停止 /);
});

test("stopping waits for termination and terminal runs have no manual release action", () => {
  const live = { ...summary("live"), origin: "manual", state: "running" };
  const currentTask = {
    runId: "live",
    state: "cancelling",
    busy: false,
    onStop: noop,
  };
  const html = render({ records: [live], record: null }, { currentTask });
  assert.match(html, /disabled="">正在停止…/);
  const ended = render(
    { records: [live], record: null },
    { currentTask: { ...currentTask, state: "succeeded" } },
  );
  assert.doesNotMatch(ended, /aria-label="停止 |当前任务控制|释放记录/);
  assert.match(ended, /已完成/);
});
test("missing and changed reference states are explicit without changing run success", () => {
  for (const [status, expected] of [
    ["missing", "文件不可用"],
    ["changed", "文件已变化"],
    ["unverified", "无法验证"],
  ]) {
    const html = render({
      checks: [{ space: "ai", path: "output.wav", role: "output", status }],
    });
    assert.match(html, new RegExp(expected));
    assert.match(html, /已完成/);
  }
});
test("empty/disconnected history and corrupt-list warnings are visible", () => {
  assert.match(
    render({ records: [], record: null }, { connected: false }),
    /打开工作区/,
  );
  assert.match(
    render({ warnings: ["损坏记录已跳过"], truncated: true }),
    /损坏记录已跳过/,
  );
  assert.match(render({ truncated: true }), /其余记录仍保留/);
});

test("history detail omission is a recording warning, not a failed run", () => {
  const html = render({
    record: {
      ...record,
      result: null,
      error: null,
      recording_warning: "结果详情超限，未完整保存",
    },
  });
  assert.match(html, /已完成/);
  assert.match(html, /banner warning/);
  assert.match(html, /结果详情超限/);
});
