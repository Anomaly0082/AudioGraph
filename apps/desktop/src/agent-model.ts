import type { AgentReply } from "./types/agent";
import { parseGraph, type GraphDocument } from "./model";

export function parseAgentReply(value: unknown, requestId: string): AgentReply {
  const reply = value as AgentReply;
  if (
    !reply ||
    reply.request_id !== requestId ||
    !["completed", "cancelled", "limited", "failed"].includes(reply.state) ||
    typeof reply.text !== "string" ||
    !Array.isArray(reply.events) ||
    reply.events.length > 100 ||
    !Number.isInteger(reply.model_calls) ||
    reply.model_calls < 0 ||
    reply.model_calls > 8 ||
    !Number.isInteger(reply.tool_calls) ||
    reply.tool_calls < 0 ||
    reply.tool_calls > 20 ||
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
    event.success !== true
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
