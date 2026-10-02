import type { GraphDocument } from "../model";
export type AgentMode = "graph" | "workflow";
export type AgentEvent = {
  kind: "input" | "assistant" | "tool" | "status";
  text?: string;
  tool?: string;
  arguments?: unknown;
  result?: unknown;
  success?: boolean;
  omitted_bytes?: number;
};
export type AgentReply = {
  request_id: string;
  conversation_id?: string;
  run_ids?: string[];
  omitted_text_bytes?: number;
  state: "completed" | "cancelled" | "limited" | "failed";
  text: string;
  events: AgentEvent[];
  model_calls: number;
  tool_calls: number;
};
export type AgentSpaceInfo = {
  user_root: string;
  ai_root: string;
  tools: string[];
};
export type AgentDraftContext = {
  mode: string;
  graph: GraphDocument;
  options: Record<string, number | boolean>;
};
