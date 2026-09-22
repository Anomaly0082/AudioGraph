import test from "node:test";
import path from "node:path";
import { assertUnambiguousJson, parseArgs } from "../src/main.mjs";
import { assert } from "./helpers.mjs";

function toolCall(versionToken = "1") {
  return `{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"audio_validate_graph","arguments":{"mode":"offline","graph":{"schema_version":${versionToken},"nodes":[],"connections":[]}}}}`;
}

test("raw MCP guard rejects ambiguous JSON before the SDK parser", () => {
  assert.doesNotThrow(() => assertUnambiguousJson(toolCall("1")));
  assert.throws(() => assertUnambiguousJson('{"jsonrpc":"2.0","id":1,"id":2}'), /Duplicate JSON key "id"/);
  assert.throws(() => assertUnambiguousJson(toolCall("1.0")), /integer token 1/);
  assert.throws(() => assertUnambiguousJson(toolCall("1e0")), /integer token 1/);
});

test("host arguments cannot enable devices, a shell, or a relative engine", () => {
  const absoluteEngine = path.resolve("control-cli.exe");
  const absoluteWorkspace = path.resolve("workspace with spaces");
  assert.deepEqual(parseArgs(["--engine", absoluteEngine, "--workspace", absoluteWorkspace]), {
    workspace: absoluteWorkspace,
    enginePath: absoluteEngine,
  });
  assert.throws(() => parseArgs(["--engine", absoluteEngine, "--workspace", absoluteWorkspace, "--allow-devices", "true"]), /Usage/);
  assert.throws(() => parseArgs(["--engine", "control-cli.exe", "--workspace", absoluteWorkspace]), /absolute/);
  assert.throws(() => parseArgs(["--engine", absoluteEngine, "--engine", absoluteEngine, "--workspace", absoluteWorkspace]), /Duplicate/);
});
