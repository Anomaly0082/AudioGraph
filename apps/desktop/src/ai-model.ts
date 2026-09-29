import {
  buildTaskOptions,
  formatError,
  parseGraph,
  type GraphDocument,
  type Mode,
} from "./model";

export type AiMode = Exclude<Mode, "realtime">;
export type AiConfig = { baseUrl: string; model: string; apiKey: string };
export type AiSettingsLoad = { path: string; config: AiConfig | null };
export type AiSettingsWrite = { path: string };
export type AiProposal = {
  mode: AiMode;
  graph: GraphDocument;
  options: Record<string, number | boolean>;
};
export type AiGenerateResponse = {
  requestId: string;
  text: string;
  proposal?: unknown;
  repairContext?: AiRepairContext;
  inspection?: AudioInspection;
  usage?: unknown;
};
export type AiRepairContext = { proposal: unknown; errors: unknown };
export type AudioInspection = {
  path: string;
  sample_rate: number;
  channels: number;
  frame_count: number;
  duration_seconds: number;
  encoding: "pcm_s16le";
};
export type AiSummaryResponse = {
  requestId: string;
  text: string;
  usage?: unknown;
};
export type AiTaskSnapshot = {
  id: string;
  sessionId: string;
  state: string;
  errors?: unknown[];
  result?: unknown;
};

export function canReleaseFailedAiTask(
  task: AiTaskSnapshot | null,
  ownedTaskId: string | null,
  sessionId: string | null,
): boolean {
  return (
    task !== null &&
    ownedTaskId !== null &&
    sessionId !== null &&
    task.id === ownedTaskId &&
    task.sessionId === sessionId &&
    task.state === "failed"
  );
}

type NodeParameter = { id: string; type: string };
export type AiNodeInfo = {
  typeId: string;
  displayName: string;
  parameters?: NodeParameter[];
};

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function createAiRequestId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function")
    return crypto.randomUUID();
  return `ai-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

export function normalizeBaseUrl(value: string): string {
  const trimmed = value.trim();
  if (!trimmed) throw new Error("请填写模型服务地址。");
  let url: URL;
  try {
    url = new URL(trimmed);
  } catch {
    throw new Error("模型服务地址不是有效 URL。");
  }
  if (url.username || url.password || url.search || url.hash) {
    throw new Error("模型服务地址不能包含用户名、密码、查询参数或片段。");
  }
  const loopback =
    url.hostname === "localhost" ||
    url.hostname === "127.0.0.1" ||
    url.hostname === "[::1]";
  if (url.protocol !== "https:" && !(url.protocol === "http:" && loopback)) {
    throw new Error(
      "远程模型服务必须使用 HTTPS；HTTP 仅允许 localhost、127.0.0.1 或 ::1。",
    );
  }
  return url.toString().replace(/\/$/, "");
}

export function normalizeAiConfig(config: AiConfig): AiConfig {
  const model = config.model.trim();
  if (!model) throw new Error("请填写模型名称。");
  return {
    baseUrl: normalizeBaseUrl(config.baseUrl),
    model,
    apiKey: config.apiKey,
  };
}

export function sameAiConfig(left: AiConfig, right: AiConfig | null): boolean {
  return (
    right !== null &&
    left.baseUrl === right.baseUrl &&
    left.model === right.model &&
    left.apiKey === right.apiKey
  );
}

export function loadedAiDraft(
  current: AiConfig,
  loaded: AiConfig | null,
  editedWhileLoading: boolean,
): AiConfig {
  return editedWhileLoading || loaded === null ? current : loaded;
}

export function canRunAiProposal(value: unknown): boolean {
  try {
    normalizeAiProposal(value);
    return true;
  } catch {
    return false;
  }
}

export function normalizeAiProposal(value: unknown): AiProposal {
  if (!record(value) || "script" in value)
    throw new Error("AI 提案不是受支持的 Graph 提案。");
  if (value.mode !== "offline" && value.mode !== "streaming") {
    throw new Error(
      "AI 只允许提议 offline 或 streaming，不会隐式启用实时设备。",
    );
  }
  const graph = parseGraph(JSON.stringify(value.graph));
  if (
    graph.nodes.some(
      (node) =>
        node.type === "realtime_input" || node.type === "realtime_output",
    )
  ) {
    throw new Error("AI 提案不得包含实时设备节点。");
  }
  const rawOptions = value.options === undefined ? {} : value.options;
  if (!record(rawOptions)) throw new Error("AI 提案的 options 必须是对象。");
  const allowed =
    value.mode === "streaming" ? new Set(["block_frames"]) : new Set<string>();
  if (Object.keys(rawOptions).some((key) => !allowed.has(key)))
    throw new Error("AI 提案包含当前模式不支持的执行选项。");
  const blockFrames =
    rawOptions.block_frames === undefined ? 256 : rawOptions.block_frames;
  const options = buildTaskOptions(value.mode, {
    blockFrames: typeof blockFrames === "number" ? blockFrames : Number.NaN,
    durationSeconds: 10,
    probe: true,
  });
  return { mode: value.mode, graph, options };
}

export function isCurrentAiResponse(
  responseRequestId: string,
  currentRequestId: string | null,
): boolean {
  return currentRequestId !== null && responseRequestId === currentRequestId;
}

export function shouldCancelAiRequest(
  stoppingRequestId: string | null,
  currentRequestId: string | null,
): boolean {
  return stoppingRequestId !== null && stoppingRequestId === currentRequestId;
}

export function mergeSummaryError<T>(
  result: T,
  error: unknown,
): { result: T; summaryError: string } {
  return { result, summaryError: formatError(error) };
}

export type ApprovalGuard = {
  approved: boolean;
  expectedSessionId: string;
  currentSessionId: string | null;
  startedRef: { current: boolean };
};

export async function runApprovedProposal(
  guard: ApprovalGuard,
  proposal: AiProposal,
  apply: (value: AiProposal) => void,
  start: (value: AiProposal) => Promise<string>,
): Promise<string | null> {
  if (!guard.approved) return null;
  if (guard.expectedSessionId !== guard.currentSessionId)
    throw new Error("连接已变化，未执行旧会话的 AI 提案。");
  if (guard.startedRef.current) return null;
  guard.startedRef.current = true;
  try {
    apply(proposal);
    return await start(proposal);
  } catch (reason) {
    guard.startedRef.current = false;
    throw reason;
  }
}

export function getFileParameters(
  proposal: AiProposal,
  nodes: AiNodeInfo[],
): { nodeId: string; parameterId: string; value: unknown }[] {
  const catalog = new Map(nodes.map((node) => [node.typeId, node]));
  const files: { nodeId: string; parameterId: string; value: unknown }[] = [];
  for (const graphNode of proposal.graph.nodes) {
    const descriptor = catalog.get(graphNode.type);
    for (const parameter of descriptor?.parameters ?? []) {
      if (
        (parameter.type === "file_path" || parameter.type === "FilePath") &&
        graphNode.parameters &&
        parameter.id in graphNode.parameters
      ) {
        files.push({
          nodeId: graphNode.id,
          parameterId: parameter.id,
          value: graphNode.parameters[parameter.id],
        });
      }
    }
  }
  return files;
}

export type GraphChange = { path: string; before?: unknown; after?: unknown };

export function compareProposalGraphs(
  before: unknown,
  after: AiProposal,
): GraphChange[] | null {
  if (!record(before)) return null;
  let previous: GraphDocument;
  try {
    previous = parseGraph(JSON.stringify(before.graph));
  } catch {
    return null;
  }
  const changes: GraphChange[] = [];
  function walk(path: string, left: unknown, right: unknown): void {
    if (Object.is(left, right)) return;
    if (Array.isArray(left) && Array.isArray(right)) {
      for (
        let index = 0;
        index < Math.max(left.length, right.length);
        index++
      ) {
        walk(`${path}[${index}]`, left[index], right[index]);
      }
    } else if (record(left) && record(right)) {
      for (const key of new Set([
        ...Object.keys(left),
        ...Object.keys(right),
      ])) {
        walk(path ? `${path}.${key}` : key, left[key], right[key]);
      }
    } else {
      changes.push({ path, before: left, after: right });
    }
  }
  walk("graph", previous, after.graph);
  walk("mode", before.mode, after.mode);
  walk("options", before.options ?? {}, after.options);
  return changes;
}
