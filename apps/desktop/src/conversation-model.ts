import { parseAgentReply } from "./agent-model";
import type { AgentEvent, AgentMode, AgentReply } from "./types/agent";

export type ConversationState = "running" | "interrupted" | AgentReply["state"];
export type ConversationTurn = {
  id: string;
  prompt: string;
  state?: ConversationState;
  reply?: AgentReply;
  error?: string | null;
  events?: AgentEvent[];
  run_ids?: string[];
  pending_tools?: {
    id: string;
    name: string;
    arguments?: unknown;
    omitted_bytes?: number;
  }[];
};
export type ConversationSummary = {
  id: string;
  title: string;
  mode: AgentMode;
  updated_at_ms: number;
  turn_count: number;
  last_state: string | null;
};
export type ConversationList = {
  records: ConversationSummary[];
  warnings: string[];
  truncated: boolean;
};
export type ConversationDetail = ConversationSummary & {
  created_at_ms: number;
  turns: ConversationTurn[];
  before?: number | null;
  has_more: boolean;
};

const object = (v: unknown): v is Record<string, unknown> =>
  !!v && typeof v === "object" && !Array.isArray(v);
const validId = (v: unknown): v is string =>
  typeof v === "string" && /^[a-f0-9]{64}$/.test(v);
const validMode = (v: unknown): v is AgentMode =>
  v === "graph" || v === "workflow";
function summary(
  v: unknown,
): v is ConversationSummary & Record<string, unknown> {
  return (
    object(v) &&
    validId(v.id) &&
    validMode(v.mode) &&
    typeof v.title === "string" &&
    Number.isSafeInteger(v.updated_at_ms) &&
    Number.isSafeInteger(v.turn_count) &&
    Number(v.turn_count) >= 0
  );
}
export function parseConversationList(value: unknown): ConversationList {
  if (
    !object(value) ||
    !Array.isArray(value.records) ||
    value.records.length > 500 ||
    !value.records.every(summary) ||
    !Array.isArray(value.warnings) ||
    !value.warnings.every((w) => typeof w === "string") ||
    typeof value.truncated !== "boolean" ||
    new Set(value.records.map((r) => r.id)).size !== value.records.length
  )
    throw new Error("会话列表格式无效。");
  return value as unknown as ConversationList;
}
export function parseConversation(
  value: unknown,
  expectedId?: string,
  expectedMode?: AgentMode,
): ConversationDetail {
  if (
    !summary(value) ||
    !object(value) ||
    (expectedId && value.id !== expectedId) ||
    (expectedMode && value.mode !== expectedMode) ||
    !Array.isArray(value.turns) ||
    value.turns.length > 100 ||
    typeof value.has_more !== "boolean" ||
    (value.has_more &&
      (!Number.isSafeInteger(value.before) || Number(value.before) <= 0))
  )
    throw new Error("会话内容与当前选择不匹配。");
  const turns = value.turns.map((v: unknown): ConversationTurn => {
    if (
      !object(v) ||
      typeof v.id !== "string" ||
      !v.id ||
      typeof v.prompt !== "string" ||
      ![
        "running",
        "interrupted",
        "completed",
        "cancelled",
        "limited",
        "failed",
      ].includes(String(v.state)) ||
      (v.run_ids !== undefined &&
        (!Array.isArray(v.run_ids) || !v.run_ids.every(validId))) ||
      (v.events !== undefined && !Array.isArray(v.events)) ||
      (v.pending_tools !== undefined &&
        (!Array.isArray(v.pending_tools) ||
          !v.pending_tools.every(
            (p) =>
              object(p) &&
              typeof p.id === "string" &&
              typeof p.name === "string",
          )))
    )
      throw new Error("会话消息格式无效。");
    const reply = v.reply == null ? undefined : parseAgentReply(v.reply, v.id);
    return { ...(v as unknown as ConversationTurn), reply };
  });
  if (new Set(turns.map((t) => t.id)).size !== turns.length)
    throw new Error("会话存在重复消息。");
  return { ...(value as unknown as ConversationDetail), turns };
}
export function mergeOlderTurns(
  older: ConversationTurn[],
  current: ConversationTurn[],
) {
  const ids = new Set(current.map((turn) => turn.id));
  return [...older.filter((turn) => !ids.has(turn.id)), ...current];
}
// Disk may still contain only a pre-tool intent when a receipt/final write fails.
// Never replace a known live receipt or an unsaved user prompt with that older page.
export function reconcileConversation(
  saved: ConversationDetail,
  live: ConversationTurn,
) {
  const durable = saved.turns.find((turn) => turn.id === live.id);
  const savedFinal =
    !!durable?.reply &&
    durable.state !== "running" &&
    durable.state !== "interrupted";
  if (savedFinal) return { detail: saved, unsaved: false };
  const turn: ConversationTurn = {
    ...durable,
    ...live,
    state: durable?.state === "interrupted" ? "interrupted" : live.state,
    run_ids: [
      ...new Set([...(durable?.run_ids ?? []), ...(live.run_ids ?? [])]),
    ],
    pending_tools: durable?.pending_tools ?? [],
    error:
      "本轮内容未完整保存，当前显示保留的临时消息与回执；请核对结果，勿直接重复执行。",
  };
  return {
    detail: {
      ...saved,
      turns: durable
        ? saved.turns.map((item) => (item.id === live.id ? turn : item))
        : [...saved.turns, turn],
    },
    unsaved: true,
  };
}
export function matchesConversationScope(
  current: {
    sessionId?: string;
    conversationId: string | null;
    mode: AgentMode;
    epoch: number;
  },
  requested: {
    sessionId: string;
    conversationId: string | null;
    mode: AgentMode;
    epoch: number;
  },
) {
  return (
    current.sessionId === requested.sessionId &&
    current.conversationId === requested.conversationId &&
    current.mode === requested.mode &&
    current.epoch === requested.epoch
  );
}
export async function readConversationPage(
  invoke: <T>(command: string, args: Record<string, unknown>) => Promise<T>,
  sessionId: string,
  conversationId: string,
  mode: AgentMode,
  before?: number | null,
) {
  const args: Record<string, unknown> = {
    sessionId,
    conversationId,
    mode,
    limit: 20,
  };
  if (before != null) args.before = before;
  return parseConversation(
    await invoke("conversation_load", args),
    conversationId,
    mode,
  );
}
