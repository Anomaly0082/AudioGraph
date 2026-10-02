import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
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
  assert.match(busy, /aria-label="选择AI会话" disabled=""/);
  assert.match(busy, /disabled="">新建会话/);
});
