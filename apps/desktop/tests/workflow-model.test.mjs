import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const { cloneSubmission, isCurrentValidationResponse } =
  require("../../../build/desktop-model-tests/workflow-model.js");
const { createTemplate } = require("../../../build/desktop-model-tests/model.js");
const { graphConnections, preparePreset, resultFields } =
  require("../../../build/desktop-model-tests/presentation.js");

function submission() {
  return {
    mode: "streaming",
    graph: {
      schema_version: 1,
      nodes: [{ id: "input", type: "wav_stream_input", parameters: { path: "original.wav" } }],
      connections: [],
      exports: [{ name: "source", node: "input", port: "audio" }],
    },
    options: { block_frames: 256 },
  };
}

test("validation response is current only for its exact session and draft/options key", () => {
  const oldKey = JSON.stringify(["session-a", "streaming", "graph-a", 256, 10, true]);
  const changedGraph = JSON.stringify(["session-a", "streaming", "graph-b", 256, 10, true]);
  const changedOptions = JSON.stringify(["session-a", "streaming", "graph-a", 512, 10, true]);
  assert.equal(isCurrentValidationResponse("session-a", oldKey, "session-a", oldKey), true);
  assert.equal(isCurrentValidationResponse("session-a", oldKey, "session-a", changedGraph), false);
  assert.equal(isCurrentValidationResponse("session-a", oldKey, "session-a", changedOptions), false);
  assert.equal(isCurrentValidationResponse("session-a", oldKey, "session-b", oldKey), false);
  assert.equal(isCurrentValidationResponse("session-a", oldKey, null, oldKey), false);
});

test("task submission captures a deep, immutable copy before later editor changes", () => {
  const draft = submission();
  const captured = cloneSubmission(draft);
  assert.deepEqual(captured, draft);
  assert.notEqual(captured, draft);
  assert.notEqual(captured.graph, draft.graph);
  assert.notEqual(captured.graph.nodes[0], draft.graph.nodes[0]);
  assert.notEqual(captured.options, draft.options);

  draft.graph.nodes[0].parameters.path = "new-demand.wav";
  draft.options.block_frames = 512;
  assert.equal(captured.graph.nodes[0].parameters.path, "original.wav");
  assert.equal(captured.options.block_frames, 256);
  assert.throws(() => { captured.graph.nodes[0].parameters.path = "late-edit.wav"; }, TypeError);
  assert.throws(() => { captured.options.block_frames = 128; }, TypeError);
});

test("all quick presets bind input and output on independent Graph copies", () => {
  for (const kind of ["wav", "denoise", "stream"]) {
    const untouched = createTemplate(kind);
    const before = structuredClone(untouched);
    const prepared = preparePreset(kind, "  source.wav  ", "  fresh-output.wav  ", "-6");
    assert.equal(prepared.graph.nodes.find((node) => node.id === "input")?.parameters?.path, "source.wav");
    assert.equal(prepared.graph.nodes.find((node) => node.id === "output")?.parameters?.path, "fresh-output.wav");
    if (kind !== "denoise") {
      assert.equal(prepared.graph.nodes.find((node) => node.id === "gain")?.parameters?.gain_db, -6);
    }
    assert.deepEqual(untouched, before, `preparing ${kind} must not mutate an existing template`);
    assert.notEqual(prepared.graph, untouched.graph);
    assert.notEqual(createTemplate(kind).graph.nodes.find((node) => node.id === "input")?.parameters?.path,
      "source.wav", "later template loads must keep their defaults");
  }
});

test("quick presets reject same-name output and enforce gain bounds", () => {
  for (const kind of ["wav", "stream", "denoise"]) {
    assert.throws(() => preparePreset(kind, " same.wav ", "same.wav", "-6"), /输出不能与输入同名/);
    assert.throws(() => preparePreset(kind, "", "fresh.wav", "-6"), /请填写输入文件/);
    assert.throws(() => preparePreset(kind, "in.wav", " ", "-6"), /请填写输入文件/);
  }
  for (const kind of ["wav", "stream"]) {
    for (const gain of ["-24", "12"]) assert.doesNotThrow(() => preparePreset(kind, "in.wav", "out.wav", gain));
    for (const gain of ["-24.1", "12.1", "", "not-a-number", "Infinity"]) {
      assert.throws(() => preparePreset(kind, "in.wav", "out.wav", gain), /增益/);
    }
  }
  assert.doesNotThrow(() => preparePreset("denoise", "in.wav", "out.wav", ""),
    "denoise has no gain control");
});

test("result summary extracts scalar fields without changing structured backend data", () => {
  const result = { outputs: {
    file: { type: "FilePath", value: "processed.wav", metadata: { frames: 12 } },
    peak: { type: "Number", value: 0.125 },
    clipped: { type: "Boolean", value: false },
    nested: { type: "Object", value: { ignored: true } },
    list: { type: "Array", value: [1, 2] },
    missing: { value: "not typed" },
  }, diagnostics: { warnings: ["retained"] } };
  const original = structuredClone(result);
  assert.deepEqual(resultFields(result), [
    { name: "file", type: "FilePath", value: "processed.wav", file: true },
    { name: "peak", type: "Number", value: "0.125", file: false },
    { name: "clipped", type: "Boolean", value: "false", file: false },
  ]);
  assert.deepEqual(result, original);
  assert.deepEqual(resultFields(null), []);
  assert.deepEqual(resultFields({ outputs: [] }), []);
});

test("Graph overview follows explicit connections rather than node array order", () => {
  const graph = { schema_version: 1, nodes: [
    { id: "output", type: "wav_output" },
    { id: "gain", type: "gain" },
    { id: "input", type: "wav_input" },
  ], connections: [
    { from: { node: "input", port: "audio" }, to: { node: "gain", port: "audio" } },
    { from: { node: "gain", port: "audio" }, to: { node: "output", port: "audio" } },
  ] };
  assert.deepEqual(graphConnections(graph), [
    "input.audio → gain.audio", "gain.audio → output.audio",
  ]);
  assert.deepEqual(graphConnections({ ...graph, connections: [] }), []);
});
