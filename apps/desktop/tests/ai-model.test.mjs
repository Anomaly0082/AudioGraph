import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const {
  canReleaseFailedAiTask,
  canRunAiProposal,
  compareProposalGraphs,
  createAiRequestId,
  getFileParameters,
  isCurrentAiResponse,
  loadedAiDraft,
  mergeSummaryError,
  normalizeAiConfig,
  normalizeAiProposal,
  normalizeBaseUrl,
  runApprovedProposal,
  sameAiConfig,
  shouldCancelAiRequest,
} = require("../../../build/desktop-model-tests/ai-model.js");

function textGraph() {
  return {
    schema_version: 1,
    nodes: [{ id: "source", type: "text_input", parameters: { text: "只生成提案，不执行" } }],
    connections: [],
    exports: [{ name: "message", node: "source", port: "text" }],
  };
}

function wavGraph() {
  return {
    schema_version: 1,
    nodes: [
      { id: "input", type: "wav_input", parameters: { path: "input.wav" } },
      { id: "gain", type: "gain", parameters: { gain_db: -6.020599913 } },
      { id: "output", type: "wav_output", parameters: { path: "output.wav" } },
    ],
    connections: [
      { from: { node: "input", port: "audio" }, to: { node: "gain", port: "audio" } },
      { from: { node: "gain", port: "audio" }, to: { node: "output", port: "audio" } },
    ],
    exports: [{ name: "file", node: "output", port: "path" }],
  };
}

test("AI endpoint normalization permits HTTPS and loopback HTTP only", () => {
  assert.equal(normalizeBaseUrl(" https://models.example/v1/ "), "https://models.example/v1");
  assert.equal(normalizeBaseUrl("http://localhost:8080/v1/"), "http://localhost:8080/v1");
  assert.equal(normalizeBaseUrl("http://127.0.0.1:8080/v1"), "http://127.0.0.1:8080/v1");
  assert.equal(normalizeBaseUrl("http://[::1]:8080/v1"), "http://[::1]:8080/v1");
  for (const rejected of ["", "not a url", "http://models.example/v1", "file:///tmp/model", "ftp://localhost/v1",
    "https://models.example/v1?route=other", "https://models.example/v1#fragment", "https://user:secret@models.example/v1"]) {
    assert.throws(() => normalizeBaseUrl(rejected));
  }
});

test("AI config trims public fields but preserves the in-memory key exactly", () => {
  const key = " dummy-key with whitespace ";
  const normalized = normalizeAiConfig({ baseUrl: "https://models.example/v1/", model: " model-a ", apiKey: key });
  assert.deepEqual(normalized, { baseUrl: "https://models.example/v1", model: "model-a", apiKey: key });
  assert.throws(() => normalizeAiConfig({ baseUrl: "https://models.example/v1", model: "  ", apiKey: key }));
  assert.equal(JSON.stringify({ ...normalized, apiKey: undefined }).includes("dummy-key"), false,
    "callers can omit the key from serializable display state");
});

test("loaded settings preserve edits made during the asynchronous read", () => {
  const draft = { baseUrl: "https://new.example/v1", model: "draft", apiKey: "draft-key" };
  const saved = { baseUrl: "https://old.example/v1", model: "saved", apiKey: "saved-key" };
  assert.deepEqual(loadedAiDraft(draft, saved, true), draft);
  assert.deepEqual(loadedAiDraft(draft, saved, false), saved);
  assert.deepEqual(loadedAiDraft(draft, null, false), draft);
  assert.equal(sameAiConfig(draft, saved), false);
  assert.equal(sameAiConfig(draft, { ...draft }), true);
  assert.equal(sameAiConfig(draft, { ...draft, apiKey: "" }), false,
    "clearing the current key must mark a saved key as modified");
  assert.equal(sameAiConfig(draft, null), false);
});

test("offline and streaming tool proposals normalize to task.start shaped values", () => {
  const offline = normalizeAiProposal({ mode: "offline", graph: textGraph() });
  assert.deepEqual(offline, { mode: "offline", graph: textGraph(), options: {} });

  const streamingGraph = wavGraph();
  streamingGraph.nodes[0].type = "wav_stream_input";
  streamingGraph.nodes[1].type = "stream_gain";
  streamingGraph.nodes[2].type = "wav_stream_output";
  const streaming = normalizeAiProposal({ mode: "streaming", graph: streamingGraph,
    options: { block_frames: 64 } });
  assert.equal(streaming.mode, "streaming");
  assert.deepEqual(streaming.options, { block_frames: 64 });
  assert.deepEqual(streaming.graph, streamingGraph);
});

test("unsupported modes, scripts, realtime devices and executor options are never runnable", () => {
  const realtimeGraph = textGraph();
  realtimeGraph.nodes = [{ id: "mic", type: "realtime_input", parameters: { device_id: "default" } }];
  const rejected = [
    { mode: "realtime", graph: textGraph() },
    { mode: "offline", graph: textGraph(), script: "require('child_process').exec('anything')" },
    { mode: "offline", graph: realtimeGraph },
    { mode: "offline", graph: textGraph(), options: { block_frames: 64 } },
    { mode: "streaming", graph: textGraph(), options: { duration_seconds: 10 } },
    { mode: "streaming", graph: textGraph(), options: { block_frames: 0 } },
    { mode: "streaming", graph: textGraph(), options: { block_frames: 1.5 } },
    { mode: "offline", graph: { schema_version: 1, nodes: [], connections: [] } },
  ];
  for (const proposal of rejected) {
    assert.equal(canRunAiProposal(proposal), false);
    assert.throws(() => normalizeAiProposal(proposal));
  }
  assert.equal(canRunAiProposal({ mode: "offline", graph: textGraph() }), true);
});

test("the controller starts only after approval and synchronously blocks double submit", async () => {
  let applies = 0, starts = 0;
  const proposal = normalizeAiProposal({ mode: "offline", graph: wavGraph() });
  const startedRef = { current: false };
  const apply = () => { applies++; };
  let release;
  const pending = new Promise((resolve) => { release = resolve; });
  const start = async () => { starts++; await pending; return "task-1"; };
  const guard = { approved: false, expectedSessionId: "session-1", currentSessionId: "session-1", startedRef };
  assert.equal(await runApprovedProposal(guard, proposal, apply, start), null);
  assert.deepEqual({ applies, starts }, { applies: 0, starts: 0 }, "denial performs no side effect");
  guard.approved = true;
  const first = runApprovedProposal(guard, proposal, apply, start);
  assert.equal(await runApprovedProposal(guard, proposal, apply, start), null);
  assert.deepEqual({ applies, starts }, { applies: 1, starts: 1 }, "double click cannot submit twice");
  release();
  assert.equal(await first, "task-1");
});

test("approval is invalidated by session changes and failed starts require a new explicit click", async () => {
  const proposal = normalizeAiProposal({ mode: "offline", graph: textGraph() });
  let starts = 0;
  const guard = { approved: true, expectedSessionId: "session-old", currentSessionId: "session-new",
    startedRef: { current: false } };
  await assert.rejects(() => runApprovedProposal(guard, proposal, () => {}, async () => { starts++; return "bad"; }),
    /连接已变化/);
  assert.equal(starts, 0);
  guard.currentSessionId = "session-old";
  await assert.rejects(() => runApprovedProposal(guard, proposal, () => {}, async () => {
    starts++; throw new Error("backend rejected");
  }), /backend rejected/);
  assert.equal(guard.startedRef.current, false);
  assert.equal(starts, 1, "there is no automatic retry");
});

test("file review is driven by discovered FilePath schema rather than node-name guesses", () => {
  const proposal = normalizeAiProposal({ mode: "offline", graph: wavGraph() });
  const nodes = [
    { typeId: "wav_input", displayName: "WAV input", parameters: [{ id: "path", type: "file_path" }] },
    { typeId: "gain", displayName: "Gain", parameters: [{ id: "gain_db", type: "Number" }] },
    { typeId: "wav_output", displayName: "WAV output", parameters: [{ id: "path", type: "file_path" }] },
  ];
  assert.deepEqual(getFileParameters(proposal, nodes), [
    { nodeId: "input", parameterId: "path", value: "input.wav" },
    { nodeId: "output", parameterId: "path", value: "output.wav" },
  ]);
  assert.deepEqual(getFileParameters(proposal, []), []);
});

test("late AI responses and stop actions are scoped to the exact active request", () => {
  assert.equal(isCurrentAiResponse("request-2", "request-2"), true);
  assert.equal(isCurrentAiResponse("request-1", "request-2"), false);
  assert.equal(isCurrentAiResponse("request-2", null), false);
  assert.equal(shouldCancelAiRequest("request-2", "request-2"), true);
  assert.equal(shouldCancelAiRequest("request-1", "request-2"), false,
    "a late stop from the old request must not cancel the new request");
  assert.equal(shouldCancelAiRequest(null, "request-2"), false);
});

test("summary errors are attached without replacing the authoritative task result", () => {
  const result = { state: "succeeded", outputs: { file: { value: "output.wav" }, peak: { value: 0.125 } } };
  const merged = mergeSummaryError(result, { code: "model_timeout", message: "解释请求超时" });
  assert.equal(merged.result, result);
  assert.match(merged.summaryError, /model_timeout/);
  assert.match(merged.summaryError, /解释请求超时/);
});

test("request identifiers are non-empty and do not collide in a short sequence", () => {
  const values = Array.from({ length: 32 }, createAiRequestId);
  assert.equal(values.every((value) => typeof value === "string" && value.length > 0), true);
  assert.equal(new Set(values).size, values.length);
});

test("only the exact AI-owned failed task in the same session can be released for repair", () => {
  const task = { id: "ai-task", sessionId: "session-a", state: "failed" };
  assert.equal(canReleaseFailedAiTask(task, "ai-task", "session-a"), true);
  assert.equal(canReleaseFailedAiTask(task, "another-task", "session-a"), false);
  assert.equal(canReleaseFailedAiTask(task, "ai-task", "session-b"), false);
  assert.equal(canReleaseFailedAiTask({ ...task, state: "succeeded" }, "ai-task", "session-a"), false);
  assert.equal(canReleaseFailedAiTask({ ...task, state: "unknown" }, "ai-task", "session-a"), false);
  assert.equal(canReleaseFailedAiTask({ ...task, state: "cancelled" }, "ai-task", "session-a"), false);
  assert.equal(canReleaseFailedAiTask(null, "ai-task", "session-a"), false);
});

test("repair preview compares Graph, mode, and options from local values", () => {
  const before = normalizeAiProposal({ mode: "offline", graph: textGraph() });
  const nextGraph = textGraph();
  nextGraph.nodes[0].parameters.text = "已修正";
  const after = normalizeAiProposal({ mode: "streaming", graph: nextGraph, options: { block_frames: 64 } });
  const changes = compareProposalGraphs(before, after);
  assert.deepEqual(changes?.map((change) => change.path), [
    "graph.nodes[0].parameters.text", "mode", "options.block_frames",
  ]);
  assert.equal(compareProposalGraphs({ graph: { nodes: "invalid" } }, after), null);
});
