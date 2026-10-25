import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderWithoutDecorativeIcons as renderToStaticMarkup } from "./markup-helpers.mjs";
const require = createRequire(import.meta.url);
const {
  parseConversation,
  parseConversationList,
  readConversationPage,
  matchesConversationScope,
  mergeOlderTurns,
  reconcileConversation,
} = require("../../../build/desktop-model-tests/conversation-model.js");
const Panel =
  require("../../../build/desktop-model-tests/components/AgentToolsPanel.js").default;
const TurnView =
  require("../../../build/desktop-model-tests/components/ConversationTurnView.js").default;
const id = "a".repeat(64),
  runId = "b".repeat(64),
  noop = () => {};
const summary = {
  id,
  mode: "workflow",
  title: "降低底噪",
  updated_at_ms: 1,
  turn_count: 1,
  last_state: "interrupted",
};
const turn = {
  id: "request-1",
  prompt: "请调整",
  state: "interrupted",
  run_ids: [runId],
  pending_tools: [{ id: "tool-1", name: "graph_run", arguments: {} }],
  events: [],
};
const detail = {
  ...summary,
  created_at_ms: 0,
  turns: [turn],
  before: null,
  has_more: false,
};
test("conversation selection checks id and immutable mode", () => {
  assert.equal(parseConversation(detail, id, "workflow").id, id);
  assert.throws(() => parseConversation(detail, runId, "workflow"));
  assert.throws(() => parseConversation(detail, id, "graph"));
  assert.throws(() =>
    parseConversation({ ...detail, turns: [turn, turn] }, id, "workflow"),
  );
  assert.throws(() =>
    parseConversationList({
      records: [summary, summary],
      warnings: [],
      truncated: false,
    }),
  );
});
test("late requests cannot cross workspace, conversation, mode, or epoch", () => {
  const current = {
    sessionId: "session-A",
    conversationId: id,
    mode: "workflow",
    epoch: 2,
  };
  assert.equal(matchesConversationScope(current, current), true);
  for (const changed of [
    { sessionId: "session-B" },
    { conversationId: runId },
    { mode: "graph" },
    { epoch: 3 },
  ])
    assert.equal(
      matchesConversationScope({ ...current, ...changed }, current),
      false,
    );
});
test("restoring a page issues only a data read, never a model turn or a tool replay", async () => {
  const calls = [];
  const invoke = async (command, args) => {
    calls.push({ command, args });
    return detail;
  };
  const result = await readConversationPage(invoke, "s1", id, "workflow");
  assert.equal(result.turns[0].state, "interrupted");
  assert.deepEqual(calls, [
    {
      command: "conversation_load",
      args: {
        sessionId: "s1",
        conversationId: id,
        mode: "workflow",
        limit: 20,
      },
    },
  ]);
});
test("older pages retain newer receipts rather than replacing duplicate turns", () => {
  const newer = { ...turn, state: "completed" };
  const merged = mergeOlderTurns([{ ...turn, id: "older" }, turn], [newer]);
  assert.deepEqual(
    merged.map((t) => t.id),
    ["older", "request-1"],
  );
  assert.equal(merged[1].state, "completed");
});
test("conversation-cap rejection preserves unsaved user text and visible reply", () => {
  const live = {
    id: "unsaved",
    prompt: "不要丢失这条需求",
    state: "failed",
    reply: {
      request_id: "unsaved",
      state: "failed",
      text: "conversation is full",
      events: [],
      model_calls: 0,
      tool_calls: 0,
    },
  };
  const merged = reconcileConversation(detail, live);
  assert.equal(merged.unsaved, true);
  assert.equal(merged.detail.turns.at(-1).prompt, live.prompt);
  assert.equal(merged.detail.turns.at(-1).reply.text, "conversation is full");
});
test("an old pending checkpoint cannot erase a known in-memory receipt", () => {
  const live = {
    ...turn,
    state: "failed",
    reply: {
      request_id: turn.id,
      state: "failed",
      text: "receipt save failed",
      events: [
        {
          kind: "tool",
          tool: "graph_run",
          success: true,
          result: { run_id: runId },
        },
      ],
      model_calls: 1,
      tool_calls: 1,
    },
  };
  const merged = reconcileConversation(detail, live);
  assert.equal(merged.unsaved, true);
  assert.equal(merged.detail.turns[0].reply.events[0].success, true);
  assert.equal(merged.detail.turns[0].state, "interrupted");
});
test("persistent conversation UI offers selection/new/read-history and clearly shows interruption", () => {
  const agent = {
    mode: "workflow",
    busy: false,
    canStop: false,
    loading: false,
    stopping: false,
    error: "",
    prompt: "",
    attachGraph: false,
    conversationId: id,
    conversations: [summary],
    turns: [turn],
    hasOlder: true,
    warnings: [],
    spaces: { user_root: "user", ai_root: "ai", tools: [] },
    setMode: noop,
    setPrompt: noop,
    setAttachGraph: noop,
    selectConversation: noop,
    newConversation: noop,
    loadOlder: noop,
    stop: noop,
  };
  const html = renderToStaticMarkup(
    React.createElement(Panel, {
      agent,
      blocked: false,
      onSend: noop,
      onApplyGraph: noop,
      onSettings: noop,
      onOpenRun: noop,
    }),
  );
  assert.match(html, /选择AI会话|新建会话|查看更早消息/);
  assert.match(html, /aria-label="删除当前会话"/);
  assert.doesNotMatch(html, /aria-label="删除当前会话"[^>]*disabled/);
  assert.match(html, /已中断/);
  assert.match(html, /不会自动重试/);
  assert.match(html, /未确认的工具调用|graph_run/);
  assert.match(html, /查看关联运行记录/);
  assert.doesNotMatch(html, /清空对话|<details|<summary/);
  const busy = renderToStaticMarkup(
    React.createElement(Panel, {
      agent: { ...agent, busy: true },
      blocked: false,
      onSend: noop,
      onApplyGraph: noop,
      onSettings: noop,
    }),
  );
  assert.match(busy, /aria-label="选择AI会话"[^>]*disabled=""/);
  assert.match(busy, /aria-label="删除当前会话"[^>]*disabled=""/);
  assert.match(busy, /disabled="">新建会话/);
  const noCurrent = renderToStaticMarkup(React.createElement(Panel, {
    agent: { ...agent, conversationId: null }, blocked: false, onSend: noop, onApplyGraph: noop, onSettings: noop,
  }));
  assert.match(noCurrent, /aria-label="删除当前会话"[^>]*disabled=""/);
  const longTitle = "C:/AAAProject/AudioProcess/apps/desktop/src/components/ConversationTurnView.tsx";
  const titled = renderToStaticMarkup(React.createElement(Panel, {
    agent: { ...agent, conversations: [{ ...summary, title: longTitle }] },
    blocked: false, onSend: noop, onApplyGraph: noop, onSettings: noop,
  }));
  assert(titled.includes(`title="${longTitle}"`), "complete original title remains available");
  const option = titled.match(/<option[^>]*title="C:\/AAAProject[^>]*>([^<]+)<\/option>/)?.[1];
  assert(option?.includes("…"), "long title is visibly shortened without a generated summary");
  assert(!option?.startsWith("Workflow ·"), "mode is not repeated in the conversation title");
});

const renderTurn = (changes = {}, props = {}) => renderToStaticMarkup(
  React.createElement(TurnView, {
    turn: {
      ...turn,
      state: "completed",
      reply: { text: "**Reply**", model_calls: 1, tool_calls: 0, events: [] },
      ...changes,
    },
    mode: "workflow",
    busy: false,
    blocked: false,
    stopping: false,
    onApplyGraph: noop,
    ...props,
  }),
);

test("chat keeps user text literal and escaped while formatting only assistant content", () => {
  const longText = "very-long-path/".repeat(300);
  const html = renderTurn({ prompt: `**literal** <script>unsafe()</script> ${longText}` });
  assert.match(html, /class="[^"]*agent-user\b/);
  assert.match(html, /class="[^"]*agent-answer\b/);
  assert.match(html, /class="[^"]*agent-user\b[^"]*" role="group" aria-label="用户消息"/);
  assert.match(html, /class="[^"]*agent-assistant\b[^"]*" role="group" aria-label="AI 回复"/);
  assert.match(html, /\*\*literal\*\* &lt;script&gt;unsafe\(\)&lt;\/script&gt;/);
  assert.ok(html.includes(longText), "long user messages must not be truncated by rendering");
  assert.match(html, /<strong>Reply<\/strong>/);
  assert.doesNotMatch(html, /<script\b|<strong>literal<\/strong>/);
});

test("chat error and interrupted receipts retain status semantics and safe disclosure content", () => {
  const html = renderTurn({
    state: "interrupted",
    error: "<img onerror=run()>",
    reply: {
      text: "",
      model_calls: 1,
      tool_calls: 1,
      omitted_text_bytes: 10,
      events: [{ kind: "tool", tool: "sample", success: false, text: "<script>event</script>", omitted_bytes: 10 }],
    },
  }, { onOpenRun: noop });
  assert.match(html, /role="status"[^>]*>上次调用已中断|role="status"[^>]*>[\s\S]*?上次调用已中断/);
  assert.match(html, /role="alert"[^>]*>[\s\S]*?&lt;img onerror=run\(\)&gt;/);
  assert.match(html, /本轮有工具调用失败/);
  assert.match(html, /本轮没有文字回复/);
  assert.match(html, /回复过长，保存的内容已缩短/);
  assert.match(html, /aria-expanded="false"/);
  assert.match(html, /查看调用详情/);
  assert.match(html, /查看未确认的工具调用/);
  assert.match(html, /查看关联运行记录/);
  assert.match(html, /&lt;script&gt;event&lt;\/script&gt;/);
  assert.doesNotMatch(html, /<script\b|<img\b|<details\b|<summary\b/);
});

test("generated workflow actions keep permission gating and require an available callback", () => {
  const event = {
    kind: "tool", tool: "file_write_text", success: true,
    arguments: { path: "draft.workflow.json", content: JSON.stringify({ schema_version: 1, inputs: {}, steps: [], outputs: {} }) },
  };
  const changes = { reply: { text: "done", model_calls: 1, tool_calls: 1, events: [event] } };
  for (const props of [{ busy: true }, { blocked: true }]) {
    const html = renderTurn(changes, { onApplyWorkflow: noop, ...props });
    assert.match(html, /<button[^>]*disabled=""[^>]*>载入 Workflow 编辑器<\/button>/);
  }
  assert.match(renderTurn(changes, { onApplyWorkflow: noop }), /<button>载入 Workflow 编辑器<\/button>/);
  assert.doesNotMatch(renderTurn(changes), /载入 Workflow 编辑器/);
  assert.doesNotMatch(renderTurn({ reply: { ...changes.reply, events: [{ ...event, success: false }] } }, { onApplyWorkflow: noop }), /载入 Workflow 编辑器/);
});
