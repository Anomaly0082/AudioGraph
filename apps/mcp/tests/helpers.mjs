import assert from "node:assert/strict";
import { access, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

export const mcpRoot = path.resolve(import.meta.dirname, "..");
export const repositoryRoot = path.resolve(mcpRoot, "..", "..");
export const mainPath = path.join(mcpRoot, "src", "main.mjs");

export async function findControlCli() {
  const candidates = ["Debug", "Release"].map(configuration =>
    path.join(repositoryRoot, "build", configuration, "control-cli.exe"));
  for (const candidate of candidates) {
    try {
      await access(candidate);
      return candidate;
    } catch {
      // Try the next checked-in build configuration.
    }
  }
  throw new Error(`control-cli.exe was not found in ${candidates.join(" or ")}`);
}

export async function temporaryWorkspace(prefix = "audiograph-mcp-") {
  const directory = await mkdtemp(path.join(os.tmpdir(), prefix));
  return {
    directory,
    async [Symbol.asyncDispose]() {
      await rm(directory, { recursive: true, force: true });
    },
  };
}

export function pcm16Wav(frames, sample = 0.25, sampleRate = 48_000, channels = 1) {
  const bytesPerSample = 2;
  const dataBytes = frames * channels * bytesPerSample;
  const buffer = Buffer.alloc(44 + dataBytes);
  buffer.write("RIFF", 0, "ascii");
  buffer.writeUInt32LE(36 + dataBytes, 4);
  buffer.write("WAVE", 8, "ascii");
  buffer.write("fmt ", 12, "ascii");
  buffer.writeUInt32LE(16, 16);
  buffer.writeUInt16LE(1, 20);
  buffer.writeUInt16LE(channels, 22);
  buffer.writeUInt32LE(sampleRate, 24);
  buffer.writeUInt32LE(sampleRate * channels * bytesPerSample, 28);
  buffer.writeUInt16LE(channels * bytesPerSample, 32);
  buffer.writeUInt16LE(16, 34);
  buffer.write("data", 36, "ascii");
  buffer.writeUInt32LE(dataBytes, 40);
  const quantized = Math.round(sample * 32768);
  for (let offset = 44; offset < buffer.length; offset += 2) buffer.writeInt16LE(quantized, offset);
  return buffer;
}

export async function readPcm16Wav(file) {
  const buffer = await readFile(file);
  assert.equal(buffer.toString("ascii", 0, 4), "RIFF");
  assert.equal(buffer.toString("ascii", 8, 12), "WAVE");
  assert.equal(buffer.toString("ascii", 12, 16), "fmt ");
  assert.equal(buffer.readUInt16LE(20), 1, "WAV must remain PCM");
  const channels = buffer.readUInt16LE(22);
  const sampleRate = buffer.readUInt32LE(24);
  const bitsPerSample = buffer.readUInt16LE(34);
  assert.equal(buffer.toString("ascii", 36, 40), "data");
  const dataBytes = buffer.readUInt32LE(40);
  const samples = [];
  for (let offset = 44; offset < 44 + dataBytes; offset += 2) samples.push(buffer.readInt16LE(offset) / 32768);
  return { channels, sampleRate, bitsPerSample, frames: samples.length / channels, samples };
}

export function audioGraph({ mode, input = "input.wav", output = "output.wav", gainDb = -6.020599913 }) {
  const streaming = mode === "streaming";
  return {
    schema_version: 1,
    nodes: [
      { id: "input", type: streaming ? "wav_stream_input" : "wav_input", parameters: { path: input } },
      { id: "gain", type: streaming ? "stream_gain" : "gain", parameters: { gain_db: gainDb } },
      { id: "output", type: streaming ? "wav_stream_output" : "wav_output", parameters: { path: output } },
    ],
    connections: [
      { from: { node: "input", port: "audio" }, to: { node: "gain", port: "audio" } },
      { from: { node: "gain", port: "audio" }, to: { node: "output", port: "audio" } },
    ],
    exports: [{ name: "written", node: "output", port: streaming ? "frames_written" : "path" }],
  };
}

export async function openRealMcp(workspace) {
  const engine = await findControlCli();
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [mainPath, "--engine", engine, "--workspace", workspace],
    stderr: "pipe",
  });
  const client = new Client({ name: "audiograph-mcp-test", version: "1.0.0" });
  try {
    await client.connect(transport);
  } catch (error) {
    await client.close().catch(() => {});
    await transport.close().catch(() => {});
    throw error;
  }
  // SDK 1.30.0 has no public child-exit report. This version-locked test hook ensures
  // client.close() observed a graceful main-process exit instead of the SDK's later kill.
  const serverProcess = transport._process;
  if (!serverProcess) {
    await client.close().catch(() => {});
    await transport.close().catch(() => {});
    assert.fail("stdio transport must retain its spawned MCP server after connect");
  }
  const serverExit = serverProcess.exitCode !== null || serverProcess.signalCode !== null
    ? Promise.resolve({ code: serverProcess.exitCode, signal: serverProcess.signalCode })
    : new Promise(resolve => serverProcess.once("exit", (code, signal) => resolve({ code, signal })));
  return {
    client,
    transport,
    serverExit,
    async [Symbol.asyncDispose]() {
      await client.close().catch(() => {});
      await transport.close().catch(() => {});
    },
  };
}

export async function assertGracefulMcpExit(connection) {
  await connection[Symbol.asyncDispose]();
  assert.deepEqual(await connection.serverExit, { code: 0, signal: null },
    "MCP main must close its backend on EOF and exit before the SDK force-kill fallback");
}

export async function call(client, name, args = {}) {
  return client.callTool({ name, arguments: args });
}

export function envelope(result) {
  assert.ok(result.structuredContent, "tool response must include structuredContent");
  assert.equal(result.content?.[0]?.type, "text");
  assert.deepEqual(JSON.parse(result.content[0].text), result.structuredContent,
    "text and structured MCP results must represent the same control envelope");
  return result.structuredContent;
}

export async function waitForTerminal(client, taskId, timeoutMs = 5_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const response = await call(client, "audio_task_status", { task_id: taskId });
    const status = envelope(response);
    assert.equal(response.isError, undefined);
    if (["succeeded", "failed", "cancelled"].includes(status.data.state)) return status;
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  throw new Error(`task ${taskId} did not reach a terminal state within ${timeoutMs} ms`);
}

export { assert, path, readFile, writeFile };
