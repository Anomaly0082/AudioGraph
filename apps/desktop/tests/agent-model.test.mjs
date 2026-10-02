import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
const require = createRequire(import.meta.url);
const {
  parseAgentReply,
  graphFromToolEvent,
  workflowFromToolEvent,
} = require("../../../build/desktop-model-tests/agent-model.js");
const Panel =
  require("../../../build/desktop-model-tests/components/AgentToolsPanel.js").default;
const graph = {
  schema_version: 1,
  nodes: [{ id: "t", type: "text_input", parameters: { text: "你好" } }],
  connections: [],
};
const noop = () => {};

test("call traces are grouped closed while failures and Graph actions remain visible", () => {
  const agent = {
    mode: "graph",
    busy: false,
    canStop: false,
    loading: false,
    stopping: false,
    prompt: "",
    error: "",
    spaces: null,
    attachGraph: true,
    setMode: noop,
    setPrompt: noop,
    setAttachGraph: noop,
    reset: noop,
    stop: noop,
    turns: [
      {
        id: "r1",
        prompt: "prepare graph",
        reply: {
          state: "completed",
          text: "方案已生成",
          model_calls: 2,
          tool_calls: 2,
          events: [
            { kind: "input", text: "request-snapshot" },
            {
              kind: "tool",
              tool: "file_read_text",
              success: false,
              result: { error: "missing" },
            },
            {
              kind: "tool",
              tool: "file_write_text",
              success: true,
              arguments: { path: "g.json", content: JSON.stringify(graph) },
            },
            { kind: "assistant", text: "raw-model-output" },
            { kind: "status", text: "completed" },
          ],
        },
      },
    ],
  };
  const html = renderToStaticMarkup(
    React.createElement(Panel, {
      agent,
      blocked: false,
      onSend: noop,
      onApplyGraph: noop,
      onSettings: noop,
    }),
  );
  const split = html.indexOf('class="disclosure agent-call-details"');
  assert.ok(split > 0);
  const visiblePrefix = html.slice(0, split);
  assert.match(visiblePrefix, /方案已生成/);
  assert.match(visiblePrefix, /本轮有工具调用失败/);
  assert.match(visiblePrefix, /载入 Graph 编辑器/);
  const details = html.slice(split);
  assert.match(
    details,
    /aria-expanded="false"[^>]*>查看调用详情<\/button><div[^>]*hidden=""/,
  );
  assert.match(details, /request-snapshot/);
  assert.match(details, /raw-model-output/);
  assert.match(details, /工具参数/);
  assert.match(details, /执行结果/);
  assert.match(details, /g\.json/);
  assert.match(details, /missing/);
  assert.doesNotMatch(html, /<details|<summary/);
});

test("agent reply must match exact request and bounded call counters", () => {
  const result = {
    request_id: "r1",
    state: "completed",
    text: "done",
    events: [],
    model_calls: 2,
    tool_calls: 1,
  };
  assert.equal(parseAgentReply(result, "r1"), result);
  assert.throws(() => parseAgentReply(result, "old"));
  assert.doesNotThrow(() => parseAgentReply({ ...result, model_calls: 16, tool_calls: 40 }, "r1"));
  assert.throws(() => parseAgentReply({ ...result, tool_calls: 41 }, "r1"));
  assert.throws(() => parseAgentReply({ ...result, model_calls: 17 }, "r1"));
  assert.throws(() => parseAgentReply({ ...result, model_calls: -1 }, "r1"));
  assert.throws(() => parseAgentReply({ ...result, state: "running" }, "r1"));
  assert.throws(() => parseAgentReply({ ...result, conversation_id: "wrong" }, "r1", "a".repeat(64)));
  assert.throws(() => parseAgentReply({ ...result, run_ids: ["../outside"] }, "r1"));
});
test("only successful Graph text writes can be loaded into the editor", () => {
  const event = {
    kind: "tool",
    tool: "file_write_text",
    success: true,
    arguments: { path: "g.json", content: JSON.stringify(graph) },
  };
  assert.deepEqual(graphFromToolEvent(event), graph);
  assert.equal(graphFromToolEvent({ ...event, success: false }), null);
  assert.equal(graphFromToolEvent({ ...event, tool: "file_read_text" }), null);
  assert.equal(
    graphFromToolEvent({ ...event, arguments: { content: "plain text" } }),
    null,
  );
});

test("Workflow loader recognizes only complete successful configuration text without executing it", () => {
  const text = JSON.stringify({schema_version:1,inputs:{},steps:[],outputs:{}});
  const event = {kind:"tool",tool:"file_write_text",success:true,arguments:{path:"test.workflow.json",content:text}};
  assert.equal(workflowFromToolEvent(event).text,text);
  assert.match(workflowFromToolEvent(event).label,/test.workflow.json/);
  assert.equal(workflowFromToolEvent({...event,success:false}),null);
  assert.equal(workflowFromToolEvent({...event,omitted_bytes:20}),null);
  assert.equal(workflowFromToolEvent({...event,arguments:{content:JSON.stringify(graph)}}),null);
  assert.equal(workflowFromToolEvent({...event,tool:"file_read_text"}),null);
  assert.equal(workflowFromToolEvent({...event,arguments:{content:"broken"}}),null);
  assert.equal(workflowFromToolEvent({...event,arguments:{content:text+" ".repeat(65536)}}),null);
  const View = require("../../../build/desktop-model-tests/components/ConversationTurnView.js").default;
  const html = renderToStaticMarkup(React.createElement(View,{mode:"workflow",busy:false,blocked:false,stopping:false,
    turn:{id:"r1",prompt:"创建配置",state:"completed",reply:{state:"completed",text:"已生成",events:[event],model_calls:1,tool_calls:1}},
    onApplyGraph:()=>assert.fail("render cannot apply"),onApplyWorkflow:()=>assert.fail("render cannot apply")}));
  assert.match(html,/载入 Workflow 编辑器/);
  assert.doesNotMatch(html,/>运行 Workflow<\/button>/);
});
test("Graph and Workflow tool panels expose different execution boundaries", () => {
  const agent = {
    mode: "graph",
    busy: false,
    canStop: false,
    loading: false,
    stopping: false,
    prompt: "",
    error: "",
    turns: [],
    spaces: {
      user_root: "C:/user",
      ai_root: "C:/managed/ai",
      tools: ["nodes_list", "file_read_text"],
    },
    attachGraph: true,
    setMode: noop,
    setPrompt: noop,
    setAttachGraph: noop,
    reset: noop,
    stop: noop,
  };
  const render = (value) =>
    renderToStaticMarkup(
      React.createElement(Panel, {
        agent: value,
        blocked: false,
        onSend: noop,
        onApplyGraph: noop,
        onSettings: noop,
      }),
    );
  const html = render(agent);
  assert.match(html, /不提供运行工具/);
  assert.doesNotMatch(html, /API Key|apiKey|<pre>.*Authorization/s);
  const workflow = render({
    ...agent,
    mode: "workflow",
    busy: true,
    canStop: true,
  });
  assert.match(workflow, /可在 AI 工作区运行 Graph 和 Workflow/);
  assert.match(workflow, />停止<\/button>/);
});
