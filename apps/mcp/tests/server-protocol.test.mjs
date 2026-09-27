import test from "node:test";
import path from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { assert, call, envelope } from "./helpers.mjs";

const fixture = path.join(import.meta.dirname, "fixtures", "mock-mcp-server.mjs");

async function openFixture(t) {
  const transport = new StdioClientTransport({ command: process.execPath, args: [fixture], stderr: "pipe" });
  const client = new Client({ name: "server-protocol-test", version: "1.0.0" });
  try {
    await client.connect(transport);
  } catch (error) {
    await client.close().catch(() => {});
    await transport.close().catch(() => {});
    throw error;
  }
  t.after(async () => {
    await client.close().catch(() => {});
    await transport.close().catch(() => {});
  });
  return client;
}

test("tool result mapping uses full envelopes and distinguishes operation/task failure", async t => {
  const client = await openFixture(t);

  const businessFailure = await call(client, "audio_describe_node", { type: "missing" });
  assert.equal(businessFailure.isError, true);
  assert.equal(envelope(businessFailure).errors[0].code, "unknown_node_type");

  const failedStatus = await call(client, "audio_task_status", { task_id: "task-fixture" });
  assert.equal(failedStatus.isError, undefined,
    "status is a successful control operation even when it reports a failed task");
  assert.equal(envelope(failedStatus).data.state, "failed");

  const failedResult = await call(client, "audio_task_result", { task_id: "task-fixture" });
  assert.equal(failedResult.isError, true, "a terminal failed/cancelled result must be salient to an MCP caller");
  assert.equal(envelope(failedResult).data.state, "failed");

  const cancelled = await call(client, "audio_cancel_task", { task_id: "task-fixture" });
  assert.equal(cancelled.isError, undefined);
  assert.equal(envelope(cancelled).data.state, "cancelled");
  const released = await call(client, "audio_release_task", { task_id: "task-fixture" });
  assert.equal(released.isError, undefined);
  assert.equal(envelope(released).data.released, true);
});

test("strict graph fields are located while parameter keys remain backend-owned", async t => {
  const client = await openFixture(t);
  const strictFailure = await client.callTool({ name: "audio_describe_node", arguments: { type: "gain", extra: 1 } });
  assert.equal(strictFailure.isError, true);
  assert.match(strictFailure.content[0].text, /invalid|unrecognized|argument/i);
  const invalidGraph = {
    schema_version: 1,
    nodes: [{ id: "future", type: "future_node", parameters: { future_parameter: 1 }, future_node_field: true }],
    connections: [],
  };
  const graphFailure = await call(client, "audio_validate_graph", { mode: "offline", graph: invalidGraph });
  assert.equal(graphFailure.isError, true);
  assert.match(graphFailure.content[0].text, /graph\.nodes\.0|nodes.*0/i);
  assert.match(graphFailure.content[0].text, /future_node_field/);

  const graph = {
    schema_version: 1,
    nodes: [{ id: "future", type: "future_node", parameters: { future_parameter: 1 } }],
    connections: [],
  };
  const validation = await call(client, "audio_validate_graph", { mode: "offline", graph });
  assert.equal(validation.isError, undefined,
    "the MCP schema must preserve backend-owned parameter keys");
  const validationEnvelope = envelope(validation);
  assert.equal(validationEnvelope.success, true);
  assert.deepEqual(validationEnvelope.data.received_graph, graph,
    "unknown parameter names and values must reach the authoritative backend unchanged");
});

test("audio_inspect forwards only an explicit path through the controlled operation", async t => {
  const client = await openFixture(t);
  const inspected = await call(client, "audio_inspect", { path: "selected.wav" });
  assert.equal(inspected.isError, undefined);
  assert.equal(envelope(inspected).data.echoed, "audio.inspect");
  const missing = await client.callTool({ name: "audio_inspect", arguments: {} });
  assert.equal(missing.isError, true);
  const extra = await client.callTool({ name: "audio_inspect", arguments: { path: "selected.wav", prompt: "guess.wav" } });
  assert.equal(extra.isError, true, "natural-language or extra path hints must not enter the inspect operation");
});
