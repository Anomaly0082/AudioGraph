import test from "node:test";
import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { ControlClient, ControlClientError } from "../src/control-client.mjs";
import { assert } from "./helpers.mjs";

const fixture = path.join(import.meta.dirname, "fixtures", "mock-control-child.mjs");

async function fixtureClient(t, scenario, { requestTimeoutMs = 250, shutdownGraceMs = 50 } = {}) {
  const directory = await mkdtemp(path.join(os.tmpdir(), `control-client-${scenario}-`));
  const marker = path.join(directory, "requests.txt");
  let child;
  let spawnCall;
  const client = new ControlClient({
    enginePath: path.resolve(directory, "trusted-control-cli.exe"),
    workspace: directory,
    requestTimeoutMs,
    shutdownGraceMs,
  }, {
    verifyPaths: async (enginePath, workspace) => ({ enginePath, workspace }),
    spawnImpl(executable, args, options) {
      spawnCall = { executable, args, options };
      child = spawn(process.execPath, [fixture, scenario, marker], options);
      return child;
    },
  });
  t.after(async () => {
    await client.close("test cleanup").catch(() => {});
    await rm(directory, { recursive: true, force: true });
  });
  return { client, marker, get child() { return child; }, get spawnCall() { return spawnCall; } };
}

test("correlates out-of-order replies and fixes shell/device process policy", async t => {
  const harness = await fixtureClient(t, "reorder");
  const capabilities = await harness.client.start();
  assert.equal(capabilities.success, true);
  assert.deepEqual(harness.spawnCall.args, ["--workspace", harness.spawnCall.options.cwd]);
  assert.equal(harness.spawnCall.options.shell, false);
  assert.equal(harness.spawnCall.options.windowsHide, true);
  assert.deepEqual(harness.spawnCall.options.stdio, ["pipe", "pipe", "pipe"]);
  assert.equal(harness.spawnCall.args.includes("--allow-devices"), false);
  assert.equal(harness.spawnCall.args.includes("--allow-monitor"), false);

  const [first, second] = await Promise.all([
    harness.client.request({ op: "nodes.list" }),
    harness.client.request({ op: "nodes.describe", type: "gain" }),
  ]);
  assert.equal(first.data.echoed, "nodes.list");
  assert.equal(second.data.echoed, "nodes.describe");
});

test("local validation rejects unsafe operations without invalidating the connection", async t => {
  const harness = await fixtureClient(t, "normal");
  await harness.client.start();
  await assert.rejects(harness.client.request({ op: "devices.list" }), error => {
    assert.ok(error instanceof ControlClientError);
    assert.match(error.code, /request_rejected$/);
    assert.equal(error.uncertain, false);
    return true;
  });
  await assert.rejects(harness.client.request({ op: "shell", command: "anything" }), /Unsupported operation/);
  assert.equal(harness.client.closed, false);
  assert.equal((await harness.client.request({ op: "audio.inspect", path: "selected.wav" })).data.echoed,
    "audio.inspect", "the bounded read-only inspection operation must pass the client allowlist");
  assert.equal((await harness.client.request({ op: "nodes.list" })).success, true);
});

test("local size, nesting, and Unicode checks leave a valid connection usable", async t => {
  const harness = await fixtureClient(t, "normal");
  await harness.client.start();
  await assert.rejects(
    harness.client.request({ op: "nodes.describe", type: "x".repeat(4 * 1024 * 1024) }),
    /4 MiB/,
  );
  let nested = {};
  for (let index = 0; index < 70; index += 1) nested = { child: nested };
  await assert.rejects(harness.client.request({ op: "nodes.list", nested }), /64 levels/);
  await assert.rejects(harness.client.request({ op: "nodes.describe", type: "\ud800" }), /surrogate/);
  assert.equal(harness.client.closed, false);
  assert.equal((await harness.client.request({ op: "nodes.list" })).success, true);
});

test("enforces the eight-request pending bound locally", async t => {
  const harness = await fixtureClient(t, "timeout", { requestTimeoutMs: 2_000, shutdownGraceMs: 20 });
  await harness.client.start();
  const pending = Array.from({ length: 8 }, () => harness.client.request({ op: "nodes.list" }));
  await assert.rejects(harness.client.request({ op: "nodes.list" }), error => error.code === "client_busy");
  await harness.client.close("finish bounded pending test");
  const settled = await Promise.allSettled(pending);
  assert.equal(settled.every(item => item.status === "rejected"), true);
});

for (const expected of [
  { scenario: "oversize", code: "response_too_large" },
  { scenario: "bad-json", code: "invalid_response" },
  { scenario: "eof", code: "connection_closed" },
]) {
  test(`${expected.scenario} response invalidates the connection and rejects pending work`, async t => {
    const harness = await fixtureClient(t, expected.scenario);
    await harness.client.start();
    await assert.rejects(harness.client.request({ op: "nodes.list" }), error => {
      assert.ok(error instanceof ControlClientError);
      assert.equal(error.code, expected.code);
      assert.equal(error.uncertain, true);
      return true;
    });
    assert.equal(harness.client.closed, true);
    await assert.rejects(harness.client.request({ op: "nodes.list" }), error => error.code === "connection_closed");
  });
}

test("timeout closes the connection and never blindly retries tasks.start", async t => {
  const harness = await fixtureClient(t, "timeout", { requestTimeoutMs: 200, shutdownGraceMs: 20 });
  await harness.client.start();
  let timeoutError;
  try {
    await harness.client.request({ op: "tasks.start", mode: "offline", graph: {} });
  } catch (error) {
    timeoutError = error;
  }
  assert.ok(timeoutError instanceof ControlClientError);
  assert.equal(timeoutError.code, "request_timeout");
  assert.equal(timeoutError.uncertain, true);
  assert.equal(harness.client.closed, true);
  await assert.rejects(harness.client.request({ op: "tasks.start", mode: "offline", graph: {} }),
    error => error.code === "connection_closed");
  const operations = (await readFile(harness.marker, "utf8")).trim().split(/\r?\n/);
  assert.deepEqual(operations, ["capabilities", "tasks.start"]);
});

test("close forcibly terminates only its stubborn owned backend after the grace period", async t => {
  const harness = await fixtureClient(t, "stubborn", { shutdownGraceMs: 20 });
  await harness.client.start();
  const pid = harness.child.pid;
  const report = await harness.client.close("intentional lifecycle test");
  assert.equal(report.forced, true);
  assert.equal(harness.client.closed, true);
  assert.equal(harness.child.exitCode === null && harness.child.signalCode === null, false,
    "close must wait until the owned child confirms exit");
  await assert.rejects(harness.client.request({ op: "nodes.list" }), error => error.code === "connection_closed");
  await assert.rejects(harness.client.start(), error => error.code === "connection_closed");
  assert.equal(harness.child.pid, pid, "a closed client must never spawn a replacement backend");
});
