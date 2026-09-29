import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const {
  appendExperimentRound,
  validateExperimentSpec,
} = require("../../../build/desktop-model-tests/experiment-model.js");
const {
  runExperimentBatch,
  readAuthoritativeTask,
  readAuthoritativeBusy,
} = require("../../../build/desktop-model-tests/experiment-runner.js");

function initialRecord() {
  const spec = validateExperimentSpec({
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
          { id: "output", type: "wav_output", parameters: { path: "old.wav" } },
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
  });
  return appendExperimentRound(
    {
      ...spec,
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
    },
    {
      candidates: [
        { label: "quiet", values: [-6] },
        { label: "loud", values: [6] },
      ],
    },
  );
}

function harness(overrides = {}) {
  let record = initialRecord();
  let task = null;
  let stopped = false;
  const events = [];
  let count = 0;
  const services = {
    readRecord: () => record,
    persist: async (value) => {
      events.push(
        `save:${value.rounds[0].candidates.map((c) => c.state).join(",")}`,
      );
      record = value;
    },
    readTask: () => task,
    ensureBound: () => {},
    stopped: () => stopped,
    hasTask: () => task !== null,
    validate: async () => {
      events.push("validate");
      return true;
    },
    start: async (submission) => {
      count++;
      events.push(`start:${count}`);
      task = {
        id: `task-${count}`,
        sessionId: "s1",
        state: "running",
        submission,
      };
      return task.id;
    },
    waitTerminal: async (id) => {
      events.push(`terminal:${id}`);
      task = { ...task, state: "succeeded", result: { path: `out-${id}.wav` } };
      return task;
    },
    cancel: async (id) => {
      events.push(`cancel:${id}`);
      task = { ...task, state: "cancelled" };
    },
    release: async () => {
      events.push("release");
      assert.equal(record.rounds[0].candidates[count - 1].state, "succeeded");
      task = null;
    },
    ownTask: (id) => events.push(`owned:${id}`),
    ...overrides,
  };
  return {
    services,
    events,
    getRecord: () => record,
    getTask: () => task,
    setTask: (value) => {
      task = value;
    },
    setStopped: () => {
      stopped = true;
    },
  };
}

test("batch uses one task at a time and persists terminal results before release", async () => {
  const h = harness();
  await runExperimentBatch("r1", h.services);
  assert.equal(h.getRecord().rounds[0].candidates[0].state, "succeeded");
  assert.equal(h.getRecord().rounds[0].candidates[1].state, "succeeded");
  assert.deepEqual(
    h.events.filter((event) => event.startsWith("start")),
    ["start:1", "start:2"],
  );
  const firstRelease = h.events.indexOf("release");
  const terminalSave = h.events.lastIndexOf(
    "save:succeeded,planned",
    firstRelease,
  );
  assert.ok(terminalSave >= 0 && terminalSave < firstRelease);
  assert.ok(h.events.indexOf("start:2") > firstRelease);
});

test("stop during start cancels the task once it becomes owned and does not start the next", async () => {
  let resolveStart;
  let started;
  const enteredStart = new Promise((resolve) => {
    started = resolve;
  });
  const h = harness();
  h.services.start = async (submission) => {
    started();
    return await new Promise((resolve) => {
      resolveStart = (id) => {
        h.setTask({ id, sessionId: "s1", state: "running", submission });
        resolve(id);
      };
    });
  };
  h.services.waitTerminal = async () => {
    throw new Error("stopped while waiting");
  };
  const run = runExperimentBatch("r1", h.services);
  await enteredStart;
  h.setStopped();
  resolveStart("late-task");
  await assert.rejects(run, /stopped while waiting/);
  assert.ok(h.events.includes("cancel:late-task"));
  assert.equal(h.getRecord().rounds[0].candidates[1].state, "planned");
  assert.equal(h.events.includes("release"), false);
});

test("running checkpoint failure cancels owned task and leaves later candidates untouched", async () => {
  const h = harness();
  const persist = h.services.persist;
  h.services.persist = async (record) => {
    if (record.rounds[0].candidates[0].state === "running")
      throw new Error("disk full");
    await persist(record);
  };
  await assert.rejects(runExperimentBatch("r1", h.services), /disk full/);
  assert.ok(h.events.includes("cancel:task-1"));
  assert.equal(
    h.events.some((event) => event === "start:2" || event === "release"),
    false,
  );
  assert.equal(h.getRecord().rounds[0].candidates[0].state, "starting");
});

test("synchronous session reads remain authoritative after release before React rerenders", () => {
  const stale = {
    id: "old",
    sessionId: "s1",
    state: "succeeded",
    submission: initialRecord().base,
  };
  assert.equal(
    readAuthoritativeTask({ task: stale, readTask: () => null }),
    null,
  );
  assert.equal(
    readAuthoritativeBusy({ busy: "释放记录", readBusy: () => null }),
    null,
  );
  assert.equal(readAuthoritativeTask({ task: stale }), stale);
});

test("failed candidate is recorded and its task remains visible", async () => {
  const h = harness();
  h.services.waitTerminal = async () => {
    const running = h.getTask();
    const failed = {
      ...running,
      state: "failed",
      errors: [{ message: "decoder failed" }],
    };
    h.setTask(failed);
    return failed;
  };
  await assert.rejects(runExperimentBatch("r1", h.services), /候选运行失败/);
  assert.equal(h.getRecord().rounds[0].candidates[0].state, "failed");
  assert.equal(h.getRecord().rounds[0].candidates[1].state, "planned");
  assert.equal(h.events.includes("release"), false);
  assert.equal(h.getTask().state, "failed");
});
