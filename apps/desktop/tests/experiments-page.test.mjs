import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
const require = createRequire(import.meta.url);
const Page =
  require("../../../build/desktop-model-tests/pages/ExperimentsPage.js").default;
const noop = () => {};
const graph = {
  schema_version: 1,
  nodes: [
    { id: "input", type: "wav_input", parameters: { path: "input.wav" } },
    { id: "gain", type: "gain", parameters: { gain_db: -6 } },
    { id: "output", type: "wav_output", parameters: { path: "output.wav" } },
  ],
  connections: [],
};
const record = {
  schema_version: 1,
  id: "ex123-1",
  workspace: "C:/audio",
  created_at: 123,
  goal: "试听选择参数",
  base: { mode: "offline", graph, options: {} },
  parameters: [
    {
      node_id: "gain",
      parameter_id: "gain_db",
      minimum: -24,
      maximum: 12,
      integer_only: false,
    },
  ],
  input: {
    node_id: "input",
    original_path: "input.wav",
    snapshot_path: ".audio-experiments/ex123-1/input.wav",
  },
  output_node_id: "output",
  rounds: [
    {
      id: "r1",
      candidates: [
        {
          id: "c1",
          label: "较轻",
          values: [-12],
          state: "succeeded",
          output_path: ".audio-experiments/ex123-1/r1-c1.wav",
          result: { outputs: { clipped: { type: "Number", value: 0 } } },
        },
        {
          id: "c2",
          label: "较响",
          values: [0],
          state: "failed",
          output_path: ".audio-experiments/ex123-1/r1-c2.wav",
          errors: [{ code: "failed", message: "fixture" }],
        },
      ],
    },
  ],
};
function render(overrides = {}) {
  const ex = {
    record: null,
    history: [],
    busy: false,
    running: false,
    error: "",
    notice: "",
    pendingProposal: null,
    setError: noop,
    refresh: noop,
    load: noop,
    create: noop,
    propose: noop,
    stop: noop,
    addManual: noop,
    runRound: noop,
    rate: noop,
    restore: noop,
    acceptProposal: noop,
    rejectProposal: noop,
    ...overrides,
  };
  return renderToStaticMarkup(
    React.createElement(Page, {
      experiments: ex,
      session: { connection: null, task: null },
      draft: { mode: "offline", localGraph: { graph } },
      setup: {
        goal: "",
        parameters: {},
        manual: "[[-6],[0]]",
        notes: {},
        ratings: {},
      },
      onSetup: noop,
      blocked: false,
      onEditor: noop,
      onSettings: noop,
      onCopy: noop,
    }),
  );
}
test("experiment setup requires workspace and explicit creation", () => {
  const html = render();
  assert.match(html, /先打开音频所在的工作区/);
  assert.match(html, /创建实验并复制输入/);
  assert.doesNotMatch(html, /API Key|apiKey/);
});
test("experiment view distinguishes successful outputs and failed candidates", () => {
  const html = render({ record });
  assert.match(html, /r1-c1.wav/);
  assert.match(html, /r1-c2.wav/);
  assert.match(html, /fixture/);
  assert.equal((html.match(/>恢复到编辑器</g) ?? []).length, 1);
  assert.equal((html.match(/>保存评价</g) ?? []).length, 1);
});
test("busy experiment keeps stop visible and does not claim audio quality", () => {
  const html = render({ record, busy: true, running: true, canStop: true });
  assert.match(html, /停止实验/);
  assert.match(html, /人工评价/);
  assert.doesNotMatch(html, /已试听|自动评选最佳/);
});
