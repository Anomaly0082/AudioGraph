// Optional production-mode headless regression and before/after benchmark.
// Uses real hook/panel/components, local mocked native reads and replies, and
// source instrumentation held only in memory. Never calls an online model.
// Supply the same existing runtime paths as the other headless smoke scripts.
// Run from apps/desktop: node tests/conversation-input-performance-smoke.mjs
// Reference run: Windows, Node 24.14.1, Chromium headless shell 143.0.7499.4,
// production React, 4,395 bytes of GFM per reply, eight typed input events.
// Input-to-commit medians without/with memo: 20 replies 85.6/0.9 ms;
// 40 replies 176.2/1.6 ms. Markdown renders: 160/0 and 320/0 respectively.
// This isolates the real conversation subtree rather than timing the full App
// or browser paint. Timing varies by machine; render-count assertions are fixed.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const require = createRequire(import.meta.url);
const app = fileURLToPath(new URL("../", import.meta.url));
if (!process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE || !process.env.AUDIOGRAPH_CHROMIUM)
  throw new Error("Supply existing AUDIOGRAPH_PLAYWRIGHT_MODULE and AUDIOGRAPH_CHROMIUM paths.");
const { chromium } = require(process.env.AUDIOGRAPH_PLAYWRIGHT_MODULE);
const markdown = Array.from({ length: 12 }, (_, i) => `### Step ${i + 1}: inspect audio

Review **sample rate** and _channels_ before conversion. Keep the original file for verification.

| Property | Input | Output |
| --- | --- | --- |
| sample rate | 96000 Hz | 48000 Hz |
| channels | stereo | mono |

- Read the source locally
- Convert and verify the output

\`\`\`json
{"sample_rate":48000,"channels":1,"format":"mp3"}
\`\`\`

`).join("");
const summary = {
  id: "a".repeat(64), title: "Local performance fixture", mode: "graph",
  updated_at_ms: 1, turn_count: 0, last_state: "completed",
};
const nativeMock = `
export async function invokeDesktop(command, args) {
  window.nativeCalls.push(command);
  if (command === 'agent_spaces') return {user_root:'C:/mock',ai_root:'C:/mock-ai',tools:[]};
  if (command === 'conversation_list') return {records:[window.summary],warnings:[],truncated:false};
  if (command === 'conversation_load') return {...window.summary,created_at_ms:1,has_more:false,turns:window.turns};
  if (command === 'agent_turn') {
    const reply={request_id:args.requestId,conversation_id:args.conversationId,state:'completed',text:window.nextReply,events:[],model_calls:1,tool_calls:0};
    window.turns.push({id:args.requestId,prompt:args.prompt,state:'completed',reply});
    window.summary.turn_count=window.turns.length;
    return reply;
  }
  throw Error('Unexpected native command '+command);
}`;
const fixture = `
import React,{useLayoutEffect} from 'react';
import {createRoot} from 'react-dom/client';
import {useAgentTools} from './src/hooks/useAgentTools';
import Panel from './src/components/AgentToolsPanel';
window.counts={turn:0,markdown:0,hook:0};window.samples=[];window.nativeCalls=[];
document.addEventListener('input',()=>{window.inputStart=performance.now();},true);
function Fixture() {
  const agent=useAgentTools({connection:{sessionId:'mock',workspace:'C:/mock'},settings:{config:{baseUrl:'https://local-mock.invalid/v1',model:'local-mock',apiKey:'test-only'}},blocked:()=>false});
  useLayoutEffect(()=>{if(window.inputStart){window.samples.push(performance.now()-window.inputStart);window.inputStart=0;}},[agent.prompt]);
  window.agent=agent;
  // Fresh callbacks deliberately reproduce App's props; memoizing the entire
  // turn would not solve this case. The expensive leaf has just a text prop.
  return <Panel agent={agent} blocked={false} onSend={()=>void agent.send()} onSettings={()=>{}} onApplyGraph={()=>{}} onOpenRun={()=>{}} onApplyWorkflow={()=>{}}/>;
}
createRoot(document.getElementById('root')).render(<Fixture/>);
`;
async function bundle(withMemo) {
  return (await build({
    stdin: { contents: fixture, resolveDir: app, loader: "tsx" },
    bundle: true, write: false, minify: true, format: "iife", platform: "browser",
    define: { "process.env.NODE_ENV": '"production"' },
    plugins: [{ name: "local-performance-instrumentation", setup(builder) {
      builder.onResolve({ filter: /api\/desktop$/ }, () => ({ path: "desktop", namespace: "mock" }));
      builder.onLoad({ filter: /.*/, namespace: "mock" }, () => ({ contents: nativeMock, loader: "js" }));
      builder.onLoad({ filter: /(AssistantMarkdown|ConversationTurnView|useAgentTools)\.tsx?$/ }, async ({ path }) => {
        let contents = await readFile(path, "utf8");
        if (path.endsWith("AssistantMarkdown.tsx")) {
          const marker = "function AssistantMarkdown({ text }: { text: string }) {";
          assert(contents.includes(marker), "Update render instrumentation when the component signature changes.");
          contents = contents.replace(marker, `${marker} window.counts.markdown++;`);
          if (!withMemo) {
            assert(contents.includes("export default memo(AssistantMarkdown);"));
            contents = contents.replace("export default memo(AssistantMarkdown);", "export default AssistantMarkdown;");
          }
        } else if (path.endsWith("ConversationTurnView.tsx")) {
          assert(contents.includes("  const events ="));
          contents = contents.replace("  const events =", "  window.counts.turn++;\n  const events =");
        } else {
          assert(contents.includes("  const [mode,"));
          contents = contents.replace("  const [mode,", "  window.counts.hook++;\n  const [mode,");
        }
        return { contents, loader: "tsx" };
      });
    } }],
  })).outputFiles[0].text;
}
const browser = await chromium.launch({ headless: true, executablePath: process.env.AUDIOGRAPH_CHROMIUM });
const results = [];
try {
  for (const withMemo of [false, true]) {
    const js = await bundle(withMemo);
    // 20 is the current page size. 40 represents history after loading older messages.
    for (const count of [0, 20, 40]) {
      const page = await browser.newPage();
      const errors = [], requests = [];
      page.on("pageerror", error => errors.push(error.message));
      await page.route("**/*", route => { requests.push(route.request().url()); return route.abort(); });
      await page.setContent('<div id="root"></div>');
      await page.evaluate(({ markdown, count, summary }) => {
        window.summary = { ...summary, turn_count: count };
        window.turns = Array.from({ length: count }, (_, i) => ({
          id: `turn-${i}`, prompt: `Inspect local audio ${i}`, state: "completed",
          reply: { request_id: `turn-${i}`, state: "completed", text: markdown, events: [], model_calls: 1, tool_calls: 0 },
        }));
      }, { markdown, count, summary });
      await page.addScriptTag({ content: js });
      await page.waitForFunction(count => window.agent && !window.agent.loading && window.agent.turns.length === count, count);
      const prompt = page.locator("#agent-prompt");
      await prompt.fill("warmup");
      await page.evaluate(() => { window.counts = { turn: 0, markdown: 0, hook: 0 }; window.samples = []; });
      await prompt.pressSequentially("abcdefgh");
      const measurement = await page.evaluate(() => ({ counts: window.counts, samples: window.samples, dom: document.querySelectorAll("*").length }));
      assert.equal(measurement.samples.length, 8);
      assert.equal(measurement.counts.hook, 8);
      assert.equal(measurement.counts.turn, 8 * count);
      assert.equal(measurement.counts.markdown, withMemo ? 0 : 8 * count);
      assert.equal(await prompt.inputValue(), "warmupabcdefgh");
      const sorted = [...measurement.samples].sort((a, b) => a - b);
      results.push({ withMemo, turns: count, markdownBytesEach: Buffer.byteLength(markdown), inputEvents: 8,
        counts: measurement.counts, domElements: measurement.dom,
        inputToCommitMedianMs: sorted[Math.floor(sorted.length / 2)], inputToCommitMaxMs: Math.max(...sorted) });

      if (withMemo && count === 20) {
        // A changed text prop must update, even while identity and turn ID stay stable.
        await page.evaluate(async () => {
          window.counts.markdown = 0;
          window.turns = window.turns.map((turn, i) => i ? turn : { ...turn, reply: { ...turn.reply, text: "**Updated existing reply**" } });
          await window.agent.selectConversation(window.summary.id);
        });
        await page.getByText("Updated existing reply", { exact: true }).waitFor();
        assert.equal(await page.evaluate(() => window.counts.markdown), 1);

        // Exercise a real hook send + completed native reply + durable reread.
        // No fetch/online API exists in this mock boundary.
        await page.evaluate(() => { window.nextReply = '<script>window.unsafeRan=true</script><img src="https://evil.test/raw.png">\n\n**New completed reply** [bad](javascript:alert%281%29) [good](https://example.test/path) ![remote](https://evil.test/image.png)'; });
        await prompt.fill("local mocked request");
        await page.getByRole("button", { name: "发送", exact: true }).click();
        await page.waitForFunction(() => !window.agent.busy && window.agent.turns.length === 21);
        await page.getByText("New completed reply", { exact: true }).waitFor();
        assert.equal(await page.evaluate(() => window.nativeCalls.filter(command => command === "agent_turn").length), 1);
        assert.equal(await page.evaluate(() => window.counts.markdown), 2, "Only the changed reply and new completed reply render.");
        assert.equal(await page.locator(".assistant-markdown img, .assistant-markdown script, .assistant-markdown a[href^='javascript:']").count(), 0);
        assert.equal(await page.evaluate(() => window.unsafeRan), undefined);
        const good = page.getByRole("link", { name: "good", exact: true });
        assert.equal(await good.getAttribute("href"), "https://example.test/path");
        assert.equal(await good.getAttribute("target"), "_blank");
        assert.equal(await good.getAttribute("rel"), "noopener noreferrer");
        assert.equal(await page.getByText("[图片：remote]", { exact: true }).count(), 1);
      }
      assert.deepEqual(errors, []);
      assert.deepEqual(requests, [], "The fixture must not initiate any network request.");
      await page.close();
    }
  }
  for (const count of [0, 20, 40]) {
    assert.equal(results.find(result => result.turns === count && result.withMemo).domElements,
      results.find(result => result.turns === count && !result.withMemo).domElements);
  }
  console.log(JSON.stringify({ passed: true, environment: { node: process.version, chromium: browser.version(), mode: "production", timing: "input event to React layout-effect commit; no timing thresholds" }, results }, null, 2));
} finally { await browser.close(); }
