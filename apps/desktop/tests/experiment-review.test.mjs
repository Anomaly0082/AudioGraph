import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const {
  appendExperimentRound,
  buildCandidateSubmission,
  normalizeExperimentRecord,
  validateExperimentProposal,
  validateExperimentSpec,
} = require("../../../build/desktop-model-tests/experiment-model.js");

const catalog = [
  {
    typeId: "wav_input",
    execution_domain: "synchronous",
    parameters: [{ id: "path", type: "file_path" }],
  },
  {
    typeId: "gain",
    execution_domain: "synchronous",
    parameters: [{ id: "gain_db", type: "number", minimum: -24, maximum: 12 }],
  },
  {
    typeId: "wav_output",
    execution_domain: "synchronous",
    parameters: [{ id: "path", type: "file_path" }],
  },
];

function spec() {
  return {
    goal: "Compare gain",
    base: {
      mode: "offline",
      graph: {
        schema_version: 1,
        nodes: [
          {
            id: "input",
            type: "wav_input",
            parameters: { path: ".audio-experiments/e1/input.wav" },
          },
          { id: "gain", type: "gain", parameters: { gain_db: 0 } },
          {
            id: "output",
            type: "wav_output",
            parameters: { path: "original.wav" },
          },
        ],
        connections: [
          {
            from: { node: "input", port: "audio" },
            to: { node: "gain", port: "audio" },
          },
          {
            from: { node: "gain", port: "audio" },
            to: { node: "output", port: "audio" },
          },
        ],
      },
      options: {},
    },
    parameters: [
      {
        node_id: "gain",
        parameter_id: "gain_db",
        minimum: -12,
        maximum: 6,
        integer_only: false,
      },
    ],
  };
}

function record() {
  return {
    ...validateExperimentSpec(spec(), catalog),
    schema_version: 1,
    id: "e1",
    workspace: "C:/audio",
    created_at: 1,
    input: {
      node_id: "input",
      original_path: "source.wav",
      snapshot_path: ".audio-experiments/e1/input.wav",
    },
    output_node_id: "output",
    rounds: [],
  };
}

test("candidate values remain numeric and within the selected bounds", () => {
  const parameters = spec().parameters;
  const valid = {
    candidates: [
      { label: "low", values: [-12] },
      { label: "high", values: [6] },
    ],
  };
  assert.deepEqual(validateExperimentProposal(valid, parameters), valid);
  for (const bad of [
    { label: "too low", values: [-13] },
    { label: "text", values: ["0"] },
    { label: "infinite", values: [Infinity] },
    { label: "extra value", values: [0, 1] },
  ]) {
    assert.throws(() =>
      validateExperimentProposal(
        { candidates: [valid.candidates[0], bad] },
        parameters,
      ),
    );
  }
  assert.throws(() =>
    validateExperimentProposal(
      {
        candidates: [
          valid.candidates[0],
          { label: "graph edit", values: [0], graph: { nodes: [] } },
        ],
      },
      parameters,
    ),
  );
  const integer = [{ ...parameters[0], integer_only: true }];
  assert.throws(() =>
    validateExperimentProposal(
      {
        candidates: [
          { label: "one", values: [1] },
          { label: "fraction", values: [1.5] },
        ],
      },
      integer,
    ),
  );
});

test("candidate submissions use the snapshot and unique outputs without changing the baseline", () => {
  const original = record();
  const expanded = appendExperimentRound(original, {
    candidates: [
      { label: "low", values: [-6] },
      { label: "high", values: [6] },
    ],
  });
  const round = expanded.rounds[0];
  const first = buildCandidateSubmission(expanded, round, round.candidates[0]);
  const second = buildCandidateSubmission(expanded, round, round.candidates[1]);
  assert.equal(
    first.graph.nodes[0].parameters.path,
    ".audio-experiments/e1/input.wav",
  );
  assert.equal(first.graph.nodes[1].parameters.gain_db, -6);
  assert.equal(second.graph.nodes[1].parameters.gain_db, 6);
  assert.notEqual(
    first.graph.nodes[2].parameters.path,
    second.graph.nodes[2].parameters.path,
  );
  assert.equal(original.base.graph.nodes[1].parameters.gain_db, 0);
  assert.equal(original.base.graph.nodes[2].parameters.path, "original.wav");
  assert.throws(() => {
    first.graph.nodes[1].parameters.gain_db = 10;
  }, TypeError);
  const tampered = structuredClone(expanded);
  tampered.rounds[0].candidates[0].output_path = "other.wav";
  assert.throws(() =>
    buildCandidateSubmission(
      tampered,
      tampered.rounds[0],
      tampered.rounds[0].candidates[0],
    ),
  );
});

test("restart marks unfinished candidates interrupted and rejects forged terminal records", () => {
  const running = appendExperimentRound(record(), {
    candidates: [
      { label: "first", values: [-6] },
      { label: "second", values: [6] },
    ],
  });
  running.rounds[0].candidates[0].state = "running";
  running.rounds[0].candidates[0].task_id = "task-1";
  const loaded = normalizeExperimentRecord(running);
  assert.equal(loaded.interrupted, true);
  assert.equal(loaded.record.rounds[0].candidates[0].state, "interrupted");
  assert.equal(loaded.record.rounds[0].candidates[1].state, "planned");
  const forged = structuredClone(running);
  forged.rounds[0].candidates[0].state = "succeeded";
  delete forged.rounds[0].candidates[0].result;
  assert.throws(() => normalizeExperimentRecord(forged));
  const badPath = structuredClone(running);
  badPath.rounds[0].candidates[1].output_path = "outside.wav";
  assert.throws(() => normalizeExperimentRecord(badPath));
});
