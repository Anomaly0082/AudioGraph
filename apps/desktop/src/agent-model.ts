import type { AgentReply } from "./types/agent";
import { parseGraph, type GraphDocument } from "./model";

export function parseAgentReply(
  value: unknown,
  requestId: string,
  conversationId?: string,
): AgentReply {
  const reply = value as AgentReply;
  if (
    !reply ||
    reply.request_id !== requestId ||
    (conversationId !== undefined &&
      reply.conversation_id !== conversationId) ||
    (reply.run_ids !== undefined &&
      (!Array.isArray(reply.run_ids) ||
        !reply.run_ids.every(
          (id) => typeof id === "string" && /^[a-f0-9]{64}$/.test(id),
        ))) ||
    !["completed", "cancelled", "limited", "failed"].includes(reply.state) ||
    typeof reply.text !== "string" ||
    !Array.isArray(reply.events) ||
    reply.events.length > 100 ||
    !Number.isInteger(reply.model_calls) ||
    reply.model_calls < 0 ||
    reply.model_calls > 16 ||
    !Number.isInteger(reply.tool_calls) ||
    reply.tool_calls < 0 ||
    reply.tool_calls > 40 ||
    reply.events.some(
      (event) =>
        !event ||
        !["input", "assistant", "tool", "status"].includes(event.kind),
    )
  )
    throw new Error("工具助手返回了无效的请求或结果格式。");
  return reply;
}

// Only a successfully written Graph configuration is offered to the manual editor.
// Loading it is not execution and still requires explicit user confirmation.
export function graphFromToolEvent(
  event: AgentReply["events"][number],
): GraphDocument | null {
  if (
    event.kind !== "tool" ||
    event.tool !== "file_write_text" ||
    event.success !== true ||
    !!event.omitted_bytes
  )
    return null;
  const args = event.arguments as { content?: unknown } | null;
  if (!args || typeof args.content !== "string") return null;
  try {
    return parseGraph(args.content);
  } catch {
    return null;
  }
}

// This only recognizes a complete editor document; authoritative validation is
// still required before running. Preserve the original text for useful errors.
export function workflowFromToolEvent(
  event: AgentReply["events"][number],
): { text: string; label: string } | null {
  if (
    event.kind !== "tool" ||
    event.tool !== "file_write_text" ||
    event.success !== true ||
    event.omitted_bytes
  )
    return null;
  const args = event.arguments as { content?: unknown; path?: unknown } | null;
  if (
    typeof args?.content !== "string" ||
    new TextEncoder().encode(args.content).length > 64 * 1024
  )
    return null;
  try {
    const value: unknown = JSON.parse(args.content);
    const object = (v: unknown): v is Record<string, unknown> =>
      !!v && typeof v === "object" && !Array.isArray(v);
    if (
      !object(value) ||
      value.schema_version !== 1 ||
      !object(value.inputs) ||
      !Array.isArray(value.steps) ||
      !object(value.outputs)
    )
      return null;
    return {
      text: args.content,
      label:
        typeof args.path === "string"
          ? args.path + "（生成时的配置）"
          : "AI Workflow（未保存）",
    };
  } catch {
    return null;
  }
}
