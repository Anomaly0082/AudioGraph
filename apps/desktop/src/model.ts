export type Mode = "offline" | "streaming" | "realtime";
export type TaskState =
  | "queued"
  | "running"
  | "cancelling"
  | "succeeded"
  | "failed"
  | "cancelled";
export type TemplateKind = "text" | "wav" | "denoise" | "stream" | "realtime";

export type GraphNode = {
  id: string;
  type: string;
  parameters?: Record<string, unknown>;
  [key: string]: unknown;
};
export type GraphDocument = {
  schema_version: 1;
  nodes: GraphNode[];
  connections: {
    from: { node: string; port: string };
    to: { node: string; port: string };
    [key: string]: unknown;
  }[];
  exports?: {
    name: string;
    node: string;
    port: string;
    [key: string]: unknown;
  }[];
  [key: string]: unknown;
};
export type OptionValues = {
  blockFrames: number;
  durationSeconds: number;
  probe: boolean;
};

function object(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
function text(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && !value.includes("\0");
}

// JSON.parse 会静默吞掉重复键。语法解析成功后再扫描原文，比较解码后的对象键。
function inspectJson(source: string) {
  let cursor = 0;
  const whitespace = () => {
    while (/\s/.test(source[cursor] ?? "")) cursor++;
  };
  const string = () => {
    const begin = cursor++;
    while (source[cursor] !== '"') {
      if (source[cursor] === "\\") cursor++;
      cursor++;
    }
    cursor++;
    return JSON.parse(source.slice(begin, cursor)) as string;
  };
  const visit = (depth: number) => {
    if (depth > 64) throw new Error("Graph JSON 嵌套不能超过 64 层。");
    whitespace();
    if (source[cursor] === "{") {
      cursor++;
      whitespace();
      const keys = new Set<string>();
      while (source[cursor] !== "}") {
        const key = string();
        if (keys.has(key)) throw new Error(`Graph JSON 存在重复键：${key}`);
        keys.add(key);
        whitespace();
        cursor++;
        whitespace();
        const start = cursor;
        visit(depth + 1);
        if (
          depth === 0 &&
          key === "schema_version" &&
          source.slice(start, cursor).trim() !== "1"
        ) {
          throw new Error("schema_version 必须写为整数 1。");
        }
        whitespace();
        if (source[cursor] !== ",") break;
        cursor++;
        whitespace();
      }
      cursor++;
    } else if (source[cursor] === "[") {
      cursor++;
      whitespace();
      while (source[cursor] !== "]") {
        visit(depth + 1);
        whitespace();
        if (source[cursor] !== ",") break;
        cursor++;
        whitespace();
      }
      cursor++;
    } else if (source[cursor] === '"') {
      string();
    } else {
      const begin = cursor;
      while (cursor < source.length && !/[\s,\]}]/.test(source[cursor]))
        cursor++;
      const value = source.slice(begin, cursor);
      if (
        value !== "true" &&
        value !== "false" &&
        value !== "null" &&
        !Number.isFinite(Number(value))
      ) {
        throw new Error("Graph JSON 不允许超出范围的数值。");
      }
    }
  };
  visit(0);
}

export function parseGraph(
  source: string,
  options: { allowEmpty?: boolean } = {},
): GraphDocument {
  if (new TextEncoder().encode(source).length > 4 * 1024 * 1024) {
    throw new Error("Graph JSON 不能超过 4 MiB。");
  }
  const value: unknown = JSON.parse(source);
  inspectJson(source);
  if (!object(value) || value.schema_version !== 1)
    throw new Error("Graph 必须是 schema_version 为 1 的对象。");
  if (
    !Array.isArray(value.nodes) ||
    (!options.allowEmpty && value.nodes.length === 0)
  )
    throw new Error("Graph 需要非空 nodes 数组。");
  const ids = new Set<string>();
  for (const node of value.nodes) {
    if (!object(node) || !text(node.id) || !text(node.type))
      throw new Error("每个节点都需要非空 id 和 type。");
    if (ids.has(node.id)) throw new Error(`节点 id 重复：${node.id}`);
    ids.add(node.id);
    if (node.parameters !== undefined && !object(node.parameters))
      throw new Error(`节点 ${node.id} 的 parameters 必须是对象。`);
  }
  if (!Array.isArray(value.connections))
    throw new Error("Graph 需要 connections 数组。");
  for (const edge of value.connections) {
    if (
      !object(edge) ||
      !object(edge.from) ||
      !object(edge.to) ||
      !text(edge.from.node) ||
      !text(edge.from.port) ||
      !text(edge.to.node) ||
      !text(edge.to.port)
    ) {
      throw new Error("连接需要 from/to 的 node 和 port。");
    }
  }
  if (
    value.exports !== undefined &&
    (!Array.isArray(value.exports) ||
      value.exports.some(
        (item: unknown) =>
          !object(item) ||
          !text(item.name) ||
          !text(item.node) ||
          !text(item.port),
      ))
  ) {
    throw new Error("exports 必须是包含 name、node、port 的数组。");
  }
  return value as GraphDocument;
}

export function isTerminal(state: string): boolean {
  return state === "succeeded" || state === "failed" || state === "cancelled";
}
export function canCancel(state: string): boolean {
  return state === "queued" || state === "running";
}

// 同一任务的迟到响应不能令 UI 生命周期回退；终态允许同状态补充结果。
export function canAdvanceTaskState(previous: string, next: string): boolean {
  const known = [
    "queued",
    "running",
    "cancelling",
    "succeeded",
    "failed",
    "cancelled",
    "unknown",
  ];
  if (!known.includes(previous) || !known.includes(next)) return false;
  if (isTerminal(previous) || previous === "unknown") return previous === next;
  if (previous === "cancelling" && (next === "queued" || next === "running"))
    return false;
  if (previous === "running" && next === "queued") return false;
  return true;
}

export function buildTaskOptions(
  mode: Mode,
  values: OptionValues,
): Record<string, number | boolean> {
  if (mode === "offline") return {};
  if (mode !== "streaming" && mode !== "realtime")
    throw new Error("未知执行模式。");
  if (
    !Number.isInteger(values.blockFrames) ||
    values.blockFrames < 1 ||
    values.blockFrames > 65536
  ) {
    throw new Error("块大小必须是 1～65536 的整数。");
  }
  if (mode === "streaming") return { block_frames: values.blockFrames };
  if (
    !Number.isInteger(values.durationSeconds) ||
    values.durationSeconds < 1 ||
    values.durationSeconds > 3600
  ) {
    throw new Error("实时任务时长必须是 1～3600 秒的整数。");
  }
  if (typeof values.probe !== "boolean")
    throw new Error("probe 必须是布尔值。");
  return {
    block_frames: values.blockFrames,
    probe: values.probe,
    duration_seconds: values.durationSeconds,
  };
}

export function bindRealtimeDevices(
  graph: GraphDocument,
  inputId: string,
  outputId: string,
): GraphDocument {
  if (!text(inputId) || !text(outputId))
    throw new Error("请明确选择输入和输出设备。");
  if (
    graph.nodes.filter((node) => node.type === "realtime_input").length !== 1 ||
    graph.nodes.filter((node) => node.type === "realtime_output").length !== 1
  ) {
    throw new Error(
      "应用设备需要恰好一个 realtime_input 和一个 realtime_output 节点；当前图未修改。",
    );
  }
  return {
    ...graph,
    nodes: graph.nodes.map((node) => {
      if (node.type !== "realtime_input" && node.type !== "realtime_output")
        return node;
      return {
        ...node,
        parameters: {
          ...node.parameters,
          device_id: node.type === "realtime_input" ? inputId : outputId,
        },
      };
    }),
  };
}

export function isCurrentTaskResponse(
  responseEpoch: number,
  currentEpoch: number,
  responseTaskId: string,
  currentTaskId: string | null,
): boolean {
  return (
    responseEpoch === currentEpoch &&
    currentTaskId !== null &&
    responseTaskId === currentTaskId
  );
}

export function formatError(reason: unknown): string {
  if (reason instanceof Error) return reason.message;
  if (typeof reason === "string") {
    try {
      return formatError(JSON.parse(reason));
    } catch {
      return reason;
    }
  }
  if (Array.isArray(reason)) return reason.map(formatError).join("\n");
  if (object(reason)) {
    if (Array.isArray(reason.errors)) return formatError(reason.errors);
    const heading = [reason.code, reason.message]
      .filter((item) => typeof item === "string")
      .join("：");
    const location = ["node_id", "port_id", "parameter_id", "field_path"]
      .filter((key) => typeof reason[key] === "string" && reason[key] !== "")
      .map((key) => `${key}=${reason[key]}`)
      .join(" · ");
    if (heading || location)
      return [heading, location].filter(Boolean).join("\n");
    return JSON.stringify(reason, null, 2);
  }
  return String(reason ?? "发生未知错误");
}

export function createTemplate(kind: TemplateKind): {
  mode: Mode;
  graph: GraphDocument;
} {
  if (kind === "text")
    return {
      mode: "offline",
      graph: {
        schema_version: 1,
        nodes: [
          {
            id: "text",
            type: "text_input",
            parameters: { text: "你好，AudioProcess。" },
          },
        ],
        connections: [],
        exports: [{ name: "message", node: "text", port: "text" }],
      },
    };
  if (kind === "denoise")
    return {
      mode: "offline",
      graph: {
        schema_version: 1,
        nodes: [
          { id: "input", type: "wav_input", parameters: { path: "input.wav" } },
          { id: "mono", type: "audio_downmix_mono" },
          {
            id: "resample",
            type: "audio_resample",
            parameters: { sample_rate: 48000 },
          },
          { id: "denoise", type: "rnnoise_denoise" },
          {
            id: "output",
            type: "wav_output",
            parameters: { path: "denoised.wav" },
          },
        ],
        connections: [
          {
            from: { node: "input", port: "audio" },
            to: { node: "mono", port: "audio" },
          },
          {
            from: { node: "mono", port: "audio" },
            to: { node: "resample", port: "audio" },
          },
          {
            from: { node: "resample", port: "audio" },
            to: { node: "denoise", port: "audio" },
          },
          {
            from: { node: "denoise", port: "audio" },
            to: { node: "output", port: "audio" },
          },
        ],
        exports: [
          { name: "file", node: "output", port: "path" },
          { name: "clipped", node: "output", port: "clipped_samples" },
        ],
      },
    };
  if (kind === "realtime")
    return {
      mode: "realtime",
      graph: {
        schema_version: 1,
        nodes: [
          {
            id: "input",
            type: "realtime_input",
            parameters: { device_id: "请选择输入设备" },
          },
          { id: "gain", type: "realtime_gain", parameters: { gain_db: -6 } },
          {
            id: "output",
            type: "realtime_output",
            parameters: { device_id: "请选择输出设备" },
          },
        ],
        connections: [
          {
            from: { node: "input", port: "audio" },
            to: { node: "gain", port: "audio" },
          },
          {
            from: { node: "gain", port: "audio" },
            to: { node: "output", port: "audio" },
          },
        ],
      },
    };
  const stream = kind === "stream";
  return {
    mode: stream ? "streaming" : "offline",
    graph: {
      schema_version: 1,
      nodes: [
        {
          id: "input",
          type: stream ? "wav_stream_input" : "wav_input",
          parameters: { path: "input.wav" },
        },
        {
          id: "gain",
          type: stream ? "stream_gain" : "gain",
          parameters: { gain_db: -6 },
        },
        {
          id: "output",
          type: stream ? "wav_stream_output" : "wav_output",
          parameters: { path: "processed.wav" },
        },
      ],
      connections: [
        {
          from: { node: "input", port: "audio" },
          to: { node: "gain", port: "audio" },
        },
        {
          from: { node: "gain", port: "audio" },
          to: { node: "output", port: "audio" },
        },
      ],
      exports: [
        { name: "file", node: "output", port: "path" },
        { name: "clipped", node: "output", port: "clipped_samples" },
      ],
    },
  };
}
