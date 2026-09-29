import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const nodeModules = fileURLToPath(new URL("../node_modules", import.meta.url));
const testFiles = [
  "tests/model.test.mjs",
  "tests/ai-model.test.mjs",
  "tests/workflow-model.test.mjs",
  "tests/pages.test.mjs",
  "tests/graph-editor-model.test.mjs",
  "tests/node-inspector.test.mjs",
  "tests/graph-canvas.test.mjs",
  "tests/canvas-interaction.test.mjs",
  "tests/experiment-model.test.mjs",
  "tests/experiment-runner.test.mjs",
  "tests/experiment-review.test.mjs",
  "tests/experiments-page.test.mjs",
  "tests/agent-model.test.mjs",
  "tests/disclosure.test.mjs",
  "tests/assistant-markdown.test.mjs",
  "tests/run-records.test.mjs",
];
const result = spawnSync(process.execPath, ["--test", ...testFiles], {
  cwd: fileURLToPath(new URL("../", import.meta.url)),
  env: { ...process.env, NODE_PATH: [nodeModules, process.env.NODE_PATH].filter(Boolean).join(process.platform === "win32" ? ";" : ":") },
  stdio: "inherit",
});
if (result.error) throw result.error;
process.exitCode = result.status ?? 1;
