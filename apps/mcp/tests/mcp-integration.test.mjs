import test from "node:test";
import {
  assert,
  path,
  writeFile,
  audioGraph,
  assertGracefulMcpExit,
  call,
  envelope,
  openRealMcp,
  pcm16Wav,
  readFile,
  readPcm16Wav,
  temporaryWorkspace,
  waitForTerminal,
} from "./helpers.mjs";

const expectedTools = [
  "audio_capabilities",
  "audio_list_nodes",
  "audio_describe_node",
  "audio_inspect",
  "audio_validate_graph",
  "audio_start_task",
  "audio_task_status",
  "audio_cancel_task",
  "audio_task_result",
  "audio_release_task",
];

test("SDK initialize and tools/list expose only the offline/streaming surface", async t => {
  const workspace = await temporaryWorkspace();
  let connection;
  t.after(async () => {
    try { if (connection) await assertGracefulMcpExit(connection); }
    finally { await workspace[Symbol.asyncDispose](); }
  });
  connection = await openRealMcp(workspace.directory);

  const listed = await connection.client.listTools();
  assert.deepEqual(listed.tools.map(tool => tool.name).sort(), [...expectedTools].sort());
  for (const forbidden of ["device", "realtime", "shell", "exec", "script"])
    assert.equal(listed.tools.some(tool => tool.name.toLowerCase().includes(forbidden)), false);

  const capabilitiesResult = await call(connection.client, "audio_capabilities");
  const capabilities = envelope(capabilitiesResult);
  assert.equal(capabilitiesResult.isError, undefined);
  assert.equal(capabilities.success, true);

  const nodesResult = await call(connection.client, "audio_list_nodes");
  const nodes = envelope(nodesResult);
  assert.equal(nodes.success, true);
  assert.ok(nodes.data.nodes.some(node => node.type === "gain" || node.type_id === "gain"));
  assert.equal(nodes.data.nodes.some(node => String(node.type ?? node.type_id).includes("realtime")), false,
    "MCP discovery must omit device-backed realtime nodes");

  const describedResult = await call(connection.client, "audio_describe_node", { type: "gain" });
  const described = envelope(describedResult);
  assert.equal(described.success, true);
  assert.match(described.data.node.description, /gain/i);
  const realtimeDescriptionResult = await call(connection.client, "audio_describe_node", { type: "realtime_gain" });
  const realtimeDescription = envelope(realtimeDescriptionResult);
  assert.equal(realtimeDescriptionResult.isError, true);
  assert.ok(realtimeDescription.errors.some(error => error.code === "node_not_exposed"));

  await writeFile(path.join(workspace.directory, "inspect.wav"), pcm16Wav(4410, 0.25, 44_100, 2));
  const inspectedResult = await call(connection.client, "audio_inspect", { path: "inspect.wav" });
  const inspected = envelope(inspectedResult);
  assert.equal(inspectedResult.isError, undefined);
  assert.equal(inspected.data.sample_rate, 44_100);
  assert.equal(inspected.data.channels, 2);
  assert.equal(inspected.data.frame_count, 4410);
  assert.equal(inspected.data.encoding, "pcm_s16le");
  assert.equal("samples" in inspected.data, false, "inspection must not load or return audio samples");

  const unknownField = await connection.client.callTool({ name: "audio_capabilities", arguments: { ignored: true } });
  assert.equal(unknownField.isError, true, "unknown MCP input fields must not be silently stripped");
  assert.match(unknownField.content[0].text, /invalid|unrecognized|argument/i);
  const realtime = await connection.client.callTool({ name: "audio_start_task", arguments: { mode: "realtime", graph: {} } });
  assert.equal(realtime.isError, true);
  assert.match(realtime.content[0].text, /invalid|realtime|argument/i);
  for (const request of [
    { name: "audio_devices", arguments: {} },
    { name: "shell", arguments: { command: "whoami" } },
  ]) {
    const unavailable = await connection.client.callTool(request);
    assert.equal(unavailable.isError, true);
    assert.match(unavailable.content[0].text, /not found|unknown/i);
  }
});

for (const { mode, frames, output, options } of [
  { mode: "offline", frames: 17, output: "离线 output.wav" },
  { mode: "streaming", frames: 301, output: "流式 output.wav", options: { block_frames: 17 } },
]) {
  test(`real control-cli ${mode} graph preserves WAV format, frames, and gain`, async t => {
    const workspace = await temporaryWorkspace(`audiograph-${mode}-`);
    let connection;
    t.after(async () => {
      try { if (connection) await assertGracefulMcpExit(connection); }
      finally { await workspace[Symbol.asyncDispose](); }
    });
    await writeFile(path.join(workspace.directory, "输入 sample.wav"), pcm16Wav(frames, 0.25));
    connection = await openRealMcp(workspace.directory);
    const graph = audioGraph({ mode, input: "输入 sample.wav", output });

    const validatedResult = await call(connection.client, "audio_validate_graph", { mode, graph, ...(options && { options }) });
    const validated = envelope(validatedResult);
    assert.equal(validatedResult.isError, undefined);
    assert.equal(validated.success, true);

    const startedResult = await call(connection.client, "audio_start_task", { mode, graph, ...(options && { options }) });
    const started = envelope(startedResult);
    assert.equal(startedResult.isError, undefined);
    assert.equal(started.success, true);
    assert.match(started.data.task_id, /^task-/);

    const terminal = await waitForTerminal(connection.client, started.data.task_id);
    assert.equal(terminal.data.state, "succeeded");
    const resultResponse = await call(connection.client, "audio_task_result", { task_id: started.data.task_id });
    const result = envelope(resultResponse);
    assert.equal(resultResponse.isError, undefined);
    assert.equal(result.success, true);
    assert.equal(result.data.state, "succeeded");

    const wav = await readPcm16Wav(path.join(workspace.directory, output));
    assert.deepEqual({ sampleRate: wav.sampleRate, channels: wav.channels, bitsPerSample: wav.bitsPerSample, frames: wav.frames },
      { sampleRate: 48_000, channels: 1, bitsPerSample: 16, frames });
    for (const sample of wav.samples) assert.ok(Math.abs(sample - 0.125) <= 1 / 32768);

    const cancelAfterTerminal = envelope(await call(connection.client, "audio_cancel_task", { task_id: started.data.task_id }));
    assert.equal(cancelAfterTerminal.success, true);
    assert.equal(cancelAfterTerminal.data.state, "succeeded", "late cancellation must not replace a terminal result");
    const release = envelope(await call(connection.client, "audio_release_task", { task_id: started.data.task_id }));
    assert.equal(release.success, true);
    const missingResult = await call(connection.client, "audio_task_status", { task_id: started.data.task_id });
    const missing = envelope(missingResult);
    assert.equal(missingResult.isError, true);
    assert.equal(missing.success, false);
    assert.ok(missing.errors.some(error => error.code === "unknown_job"));
  });
}

test("backend validation errors retain isError, structuredContent, and field locations", async t => {
  const workspace = await temporaryWorkspace("audiograph-boundary-");
  let connection;
  t.after(async () => {
    try { if (connection) await assertGracefulMcpExit(connection); }
    finally { await workspace[Symbol.asyncDispose](); }
  });
  await writeFile(path.join(workspace.directory, "input.wav"), pcm16Wav(17));
  await writeFile(path.join(workspace.directory, "exists.wav"), Buffer.from("do not overwrite"));
  connection = await openRealMcp(workspace.directory);

  const cases = [
    { graph: audioGraph({ mode: "offline", output: "../escape.wav" }), code: "path_not_allowed" },
  ];

  for (const expected of cases) {
    const response = await call(connection.client, "audio_validate_graph", { mode: "offline", graph: expected.graph });
    const failure = envelope(response);
    assert.equal(response.isError, true);
    assert.equal(failure.success, false);
    if (expected.code) assert.ok(failure.errors.some(error => error.code === expected.code));
  }

  const malformedGraph = audioGraph({ mode: "offline", output: "bad.wav" });
  malformedGraph.nodes[1].unexpected = true;
  const malformed = await call(connection.client, "audio_validate_graph", { mode: "offline", graph: malformedGraph });
  assert.equal(malformed.isError, true);
  assert.match(malformed.content[0].text, /graph\.nodes\.1|nodes.*1/i);
  assert.match(malformed.content[0].text, /unexpected/);

  const overwriteGraph = audioGraph({ mode: "offline", output: "exists.wav" });
  const overwriteStart = envelope(await call(connection.client, "audio_start_task", {
    mode: "offline", graph: overwriteGraph,
  }));
  assert.equal(overwriteStart.success, true,
    "output existence is intentionally checked by the worker, not pure graph validation");
  const overwriteTerminal = await waitForTerminal(connection.client, overwriteStart.data.task_id);
  assert.equal(overwriteTerminal.data.state, "failed");
  const overwriteResponse = await call(connection.client, "audio_task_result", { task_id: overwriteStart.data.task_id });
  const overwriteResult = envelope(overwriteResponse);
  assert.equal(overwriteResponse.isError, true);
  assert.ok(overwriteResult.data.errors.some(error => error.code === "output_exists"));

  assert.equal(await readFile(path.join(workspace.directory, "exists.wav"), "utf8"), "do not overwrite");
});
