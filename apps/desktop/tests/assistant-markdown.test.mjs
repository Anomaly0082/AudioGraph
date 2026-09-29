import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

const require = createRequire(import.meta.url);
const AssistantMarkdown =
  require("../../../build/desktop-model-tests/components/AssistantMarkdown.js").default;
const AgentToolsPanel =
  require("../../../build/desktop-model-tests/components/AgentToolsPanel.js").default;
const render = (Component, props) =>
  renderToStaticMarkup(React.createElement(Component, props));
const noop = () => {};

test("assistant reply renders common Markdown and GFM table markup", () => {
  const html = render(AssistantMarkdown, {
    text: "# Heading\n\n**bold** and *italic* with `inline`\n\n- first\n- second\n\n1. ordered\n\n```js\nconst n = 1;\n```\n\n| Name | Value |\n| --- | --- |\n| a | b |",
  });

  assert.match(html, /<h1>Heading<\/h1>/);
  assert.match(html, /<strong>bold<\/strong>/);
  assert.match(html, /<em>italic<\/em>/);
  assert.match(html, /<code>inline<\/code>/);
  assert.match(html, /<ul>[\s\S]*<li>first<\/li>[\s\S]*<li>second<\/li>[\s\S]*<\/ul>/);
  assert.match(html, /<ol>[\s\S]*<li>ordered<\/li>[\s\S]*<\/ol>/);
  assert.match(html, /<pre><code class="language-js">const n = 1;\n<\/code><\/pre>/);
  assert.match(html, /<div class="markdown-table"><table>[\s\S]*<th>Name<\/th>[\s\S]*<td>b<\/td>[\s\S]*<\/table><\/div>/);
});

test("assistant reply does not activate raw HTML, unsafe links, or remote images", () => {
  const html = render(AssistantMarkdown, {
    text: '<script>alert("run")</script><img src="https://evil.test/raw.png" onerror="run()">\n\n[bad](javascript:alert%281%29) [good](https://example.test/path) ![remote](https://evil.test/remote.png)',
  });

  assert.doesNotMatch(html, /<script\b|<img\b/i);
  assert.match(html, /&lt;script&gt;/);
  assert.doesNotMatch(html, /href="javascript:/i);
  assert.match(html, /<a href="https:\/\/example\.test\/path" target="_blank" rel="noopener noreferrer">good<\/a>/);
  assert.match(html, /<span>bad<\/span>/);
  assert.match(html, /<span class="markdown-image-note">\[图片：remote\]<\/span>/);
});

test("only the main assistant reply gets Markdown formatting", () => {
  const agent = {
    mode: "workflow",
    busy: false,
    canStop: false,
    loading: false,
    error: "",
    spaces: { user_root: "C:/audio", ai_root: "C:/ai", tools: [] },
    turns: [{
      id: "turn-1",
      prompt: "**prompt stays literal**",
      reply: {
        state: "completed",
        text: "**main reply**",
        model_calls: 1,
        tool_calls: 1,
        events: [{
          kind: "tool",
          tool: "sample_tool",
          success: true,
          text: "**raw event**",
          arguments: { note: "**raw argument**" },
          result: { note: "**raw result**" },
        }],
      },
    }],
    prompt: "",
    attachGraph: false,
    setMode: noop,
    reset: noop,
    setAttachGraph: noop,
    setPrompt: noop,
  };
  const html = render(AgentToolsPanel, {
    agent,
    blocked: false,
    onSend: noop,
    onApplyGraph: noop,
    onSettings: noop,
  });

  assert.match(html, /<strong>main reply<\/strong>/);
  assert.match(html, /<p>\*\*prompt stays literal\*\*<\/p>/);
  assert.match(html, /<pre>\*\*raw event\*\*<\/pre>/);
  assert.match(html, /<pre>[\s\S]*\*\*raw argument\*\*[\s\S]*<\/pre>/);
  assert.match(html, /<pre>[\s\S]*\*\*raw result\*\*[\s\S]*<\/pre>/);
});
