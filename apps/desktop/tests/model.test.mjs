import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const {
  parseGraph,
  isTerminal,
  canCancel,
  canAdvanceTaskState,
  buildTaskOptions,
  bindRealtimeDevices,
  formatError,
  isCurrentTaskResponse,
  createTemplate,
} = require("../../../build/desktop-model-tests/model.js");

const options = { blockFrames: 64, durationSeconds: 30, probe: true };

function graphFixture() {
  return {
    schema_version: 1,
    nodes: [
      { id: "mic", type: "realtime_input", parameters: { device_id: "old-in", retained: 1 } },
      { id: "custom", type: "custom_processor", parameters: { gain_db: -6, text: "中文" } },
      { id: "speaker", type: "realtime_output", parameters: { device_id: "old-out", retained: 2 } },
    ],
    connections: [
      { from: { node: "mic", port: "audio" }, to: { node: "custom", port: "audio" } },
      { from: { node: "custom", port: "audio" }, to: { node: "speaker", port: "audio" } },
    ],
    exports: [],
  };
}

test("task state helpers distinguish pending cancellation from terminal completion", () => {
  for (const state of ["queued", "running"]) {
    assert.equal(isTerminal(state), false);
    assert.equal(canCancel(state), true);
  }
  assert.equal(isTerminal("cancelling"), false);
  assert.equal(canCancel("cancelling"), false);
  for (const state of ["succeeded", "failed", "cancelled"]) {
    assert.equal(isTerminal(state), true);
    assert.equal(canCancel(state), false);
  }
});

test("mode options send only fields accepted by that executor", () => {
  assert.deepEqual(buildTaskOptions("offline", options), {});
  assert.deepEqual(buildTaskOptions("streaming", options), { block_frames: 64 });
  assert.deepEqual(buildTaskOptions("realtime", options), {
    block_frames: 64, duration_seconds: 30, probe: true,
  });
  assert.deepEqual(buildTaskOptions("offline", {
    blockFrames: Number.NaN, durationSeconds: Number.NaN, probe: false,
  }), {});
  assert.deepEqual(buildTaskOptions("streaming", {
    blockFrames: 7, durationSeconds: Number.NaN, probe: false,
  }), { block_frames: 7 });
});

test("mode option bounds are integer and mode-specific", () => {
  for (const value of [0, 65537, 2.5, Number.NaN, Number.POSITIVE_INFINITY]) {
    assert.throws(() => buildTaskOptions("streaming", { ...options, blockFrames: value }));
    assert.throws(() => buildTaskOptions("realtime", { ...options, blockFrames: value }));
  }
  for (const value of [0, 3601, 1.5, Number.NaN, Number.POSITIVE_INFINITY]) {
    assert.throws(() => buildTaskOptions("realtime", { ...options, durationSeconds: value }));
  }
  assert.equal(buildTaskOptions("streaming", { ...options, blockFrames: 1 }).block_frames, 1);
  assert.equal(buildTaskOptions("streaming", { ...options, blockFrames: 65536 }).block_frames, 65536);
  assert.equal(buildTaskOptions("realtime", { ...options, durationSeconds: 3600 }).duration_seconds, 3600);
});

test("Graph parsing preserves unknown node types, parameters and connection order", () => {
  const graph = graphFixture();
  assert.deepEqual(parseGraph(JSON.stringify(graph)), graph);
  const minimal = { schema_version: 1, nodes: [{ id: "text", type: "text_input" }], connections: [] };
  assert.deepEqual(parseGraph(JSON.stringify(minimal)), minimal);
  // 字符串内看起来像JSON的内容不应触发重复键扫描。
  graph.nodes[1].parameters.text = '\\"gain_db":1,"gain_db":2';
  assert.deepEqual(parseGraph(JSON.stringify(graph)), graph);
});

test("Graph parsing rejects malformed structure before backend submission", () => {
  for (const text of ["", "not json", "null", "[]", "{}", '{"schema_version":2,"nodes":[],"connections":[]}']) {
    assert.throws(() => parseGraph(text));
  }
  const graph = graphFixture();
  const invalid = [
    { ...graph, schema_version: "1" },
    { ...graph, nodes: [] },
    { ...graph, nodes: {} },
    { ...graph, connections: {} },
    { ...graph, nodes: [{ id: "", type: "gain" }] },
    { ...graph, nodes: [{ id: "a", type: "" }] },
    { ...graph, nodes: [{ id: "a", type: "gain", parameters: [] }] },
    { ...graph, nodes: [...graph.nodes, { id: "mic", type: "other" }] },
    { ...graph, connections: [{ from: "mic.audio", to: "speaker.audio" }] },
    { ...graph, connections: [{ from: { node: "mic" }, to: { node: "speaker", port: "audio" } }] },
    { ...graph, exports: [{ name: "result", node: "custom" }] },
  ];
  for (const value of invalid) assert.throws(() => parseGraph(JSON.stringify(value)));
});

test("duplicate JSON keys are rejected, including escaped aliases and nested parameters", () => {
  const cases = [
    '{"schema_version":1,"schema_version":1,"nodes":[{"id":"a","type":"text_input"}],"connections":[]}',
    '{"schema_version":1,"nodes":[{"id":"a","type":"text_input","ty\\u0070e":"gain"}],"connections":[]}',
    '{"schema_version":1,"nodes":[{"id":"a","type":"gain","parameters":{"gain_db":1,"gain_db":2}}],"connections":[]}',
  ];
  for (const text of cases) assert.throws(() => parseGraph(text));
  const valid = {
    schema_version: 1,
    nodes: [{ id: "a", type: "gain", parameters: { gain_db: 1 } },
      { id: "b", type: "gain", parameters: { gain_db: 2 } }],
    connections: [],
  };
  assert.deepEqual(parseGraph(JSON.stringify(valid)), valid);
});

test("device selection changes only existing endpoint bindings and does not recreate the Graph", () => {
  const graph = graphFixture();
  const saved = structuredClone(graph);
  const result = bindRealtimeDevices(graph, "new-input-id", "new-output-id");
  assert.deepEqual(graph, saved, "caller Graph must not be mutated");
  assert.notEqual(result, graph);
  const expected = structuredClone(saved);
  expected.nodes[0].parameters.device_id = "new-input-id";
  expected.nodes[2].parameters.device_id = "new-output-id";
  assert.deepEqual(result, expected);
  assert.equal(result.nodes[1].type, "custom_processor");
  assert.deepEqual(result.connections, saved.connections);
  assert.deepEqual(result.exports, saved.exports);
});

test("device selection refuses ambiguous/missing endpoints instead of silently replacing nodes", () => {
  const graph = graphFixture();
  assert.throws(() => bindRealtimeDevices({ ...graph, nodes: graph.nodes.slice(1) }, "in", "out"));
  assert.throws(() => bindRealtimeDevices({ ...graph, nodes: [
    ...graph.nodes, { id: "second", type: "realtime_input", parameters: {} },
  ] }, "in", "out"));
  assert.throws(() => bindRealtimeDevices(graph, "", "out"));
  assert.throws(() => bindRealtimeDevices(graph, "in", ""));
  assert.throws(() => bindRealtimeDevices(graph, "in\0bad", "out"));
});

test("late task polls and responses from an earlier connection are ignored", () => {
  assert.equal(isCurrentTaskResponse(3, 3, "task-7", "task-7"), true);
  assert.equal(isCurrentTaskResponse(2, 3, "task-7", "task-7"), false);
  assert.equal(isCurrentTaskResponse(3, 3, "task-6", "task-7"), false);
  assert.equal(isCurrentTaskResponse(3, 3, "task-7", null), false);
  // 重连后的服务可能从task-1重新编号，仅比较job ID不足以防止旧结果覆盖新任务。
  assert.equal(isCurrentTaskResponse(1, 2, "task-1", "task-1"), false);
});

test("same-task late polling cannot regress cancellation or a terminal state", () => {
  assert.equal(canAdvanceTaskState("queued", "running"), true);
  assert.equal(canAdvanceTaskState("queued", "cancelled"), true);
  assert.equal(canAdvanceTaskState("running", "cancelling"), true);
  assert.equal(canAdvanceTaskState("running", "succeeded"), true);
  assert.equal(canAdvanceTaskState("running", "queued"), false);
  assert.equal(canAdvanceTaskState("cancelling", "queued"), false);
  assert.equal(canAdvanceTaskState("cancelling", "running"), false);
  assert.equal(canAdvanceTaskState("cancelling", "cancelled"), true);
  for (const terminal of ["succeeded", "failed", "cancelled"]) {
    assert.equal(canAdvanceTaskState(terminal, terminal), true);
    for (const previous of ["queued", "running", "cancelling"]) {
      assert.equal(canAdvanceTaskState(terminal, previous), false);
    }
    for (const different of ["succeeded", "failed", "cancelled"].filter((state) => state !== terminal)) {
      assert.equal(canAdvanceTaskState(terminal, different), false);
    }
  }
  assert.equal(canAdvanceTaskState("unknown", "unknown"), true);
  assert.equal(canAdvanceTaskState("unknown", "running"), false);
  assert.equal(canAdvanceTaskState("unknown", "succeeded"), false);
});

test("error rendering retains the actionable backend code and location", () => {
  const message = formatError({ success: false, errors: [{
    code: "unknown_parameter", message: "Unrecognized gain setting", node_id: "gain-1",
    parameter_id: "gian_db", field_path: "/nodes/1/parameters/gian_db",
  }] });
  for (const fragment of ["unknown_parameter", "Unrecognized gain setting", "gain-1", "/nodes/1/parameters/gian_db"])
    assert.ok(message.includes(fragment), `missing error context: ${fragment}`);
  assert.ok(formatError(new Error("connection closed")).includes("connection closed"));
  assert.ok(formatError("sidecar disconnected").includes("sidecar disconnected"));
});

test("templates are valid independent documents and default text has no file side effect", () => {
  for (const kind of ["text", "wav", "stream", "realtime"]) {
    const template = createTemplate(kind);
    assert.deepEqual(parseGraph(JSON.stringify(template.graph)), template.graph);
    assert.ok(["offline", "streaming", "realtime"].includes(template.mode));
  }
  const first = createTemplate("text");
  assert.equal(first.mode, "offline");
  assert.equal(first.graph.nodes.some((node) => node.type === "text_output" || node.type === "wav_output"), false);
  first.graph.nodes[0].id = "mutated";
  assert.notEqual(createTemplate("text").graph.nodes[0].id, "mutated");
  const live = createTemplate("realtime");
  assert.equal(live.mode, "realtime");
  assert.equal(live.graph.nodes.filter((node) => node.type === "realtime_input").length, 1);
  assert.equal(live.graph.nodes.filter((node) => node.type === "realtime_output").length, 1);
});
