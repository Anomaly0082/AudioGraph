import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const {
  validateExperimentSpec,
  appendExperimentRound,
  normalizeExperimentRecord,
} = require("../../../build/desktop-model-tests/experiment-model.js");

function fixture() {
  const spec = {
    goal: "compare",
    base: {
      mode: "offline",
      options: {},
      graph: {
        schema_version: 1,
        nodes: [
          {
            id: "input",
            type: "wav_input",
            parameters: { path: ".audio-experiments/ex1-1/input.wav" },
          },
          { id: "gain", type: "gain", parameters: { gain_db: 0 } },
          {
            id: "output",
            type: "wav_output",
            parameters: { path: "original.wav" },
          },
        ],
        connections: [],
      },
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
  return {
    ...validateExperimentSpec(spec),
    schema_version: 1,
    id: "ex1-1",
    workspace: "C:/audio",
    created_at: 1,
    input: {
      node_id: "input",
      original_path: "source.wav",
      snapshot_path: ".audio-experiments/ex1-1/input.wav",
    },
    output_node_id: "output",
    rounds: [],
  };
}

test("checkpoint normalization preserves live states while restart normalization interrupts them", () => {
  const record = appendExperimentRound(fixture(), {
    candidates: [
      { label: "quiet", values: [-6] },
      { label: "loud", values: [6] },
    ],
  });
  record.rounds[0].candidates[0].state = "starting";
  assert.equal(
    normalizeExperimentRecord(record, false).record.rounds[0].candidates[0]
      .state,
    "starting",
  );
  assert.equal(
    normalizeExperimentRecord(record).record.rounds[0].candidates[0].state,
    "interrupted",
  );
  const secret = structuredClone(record);
  secret.apiKey = "sensitive";
  assert.throws(() => normalizeExperimentRecord(secret), /密钥/);
});
