import { createTemplate, type GraphDocument, type TemplateKind } from "./model";

export type PageId =
  | "workbench"
  | "editor"
  | "tasks"
  | "settings";
export const pages: { id: PageId; label: string; description: string }[] = [
  {
    id: "workbench",
    label: "处理工作台",
    description: "选择音频，用需求或模板准备处理方案。",
  },
  {
    id: "editor",
    label: "Graph 编辑器",
    description: "编辑独立草稿，查看节点约束，校验后再提交。",
  },
  {
    id: "tasks",
    label: "运行记录",
    description: "查看 Graph 和 Workflow 的运行历史。",
  },
  {
    id: "settings",
    label: "设置",
    description: "管理模型 API 与工作区的设备权限。",
  },
];
export const modeLabels = {
  offline: "整段离线",
  streaming: "离线分块",
  realtime: "实时设备",
};
export const stateLabels: Record<string, string> = {
  queued: "等待处理",
  running: "正在处理",
  cancelling: "正在取消",
  succeeded: "已完成",
  failed: "处理失败",
  cancelled: "已取消",
  unknown: "状态未知",
};
export const templateLabels: Record<TemplateKind, string> = {
  text: "文本测试 · 不输出文件",
  wav: "音量调整",
  denoise: "语音降噪 · 含格式适配",
  stream: "长音频音量调整 · 分块",
  realtime: "实时设备 · 高级",
};

export type QuickPreset = "wav" | "denoise" | "stream";
export function preparePreset(
  kind: QuickPreset,
  input: string,
  output: string,
  gainDb: string,
) {
  if (!input.trim() || !output.trim())
    throw new Error("请填写输入文件和新的输出文件名。");
  if (input.trim() === output.trim())
    throw new Error("输出不能与输入同名，请使用新的文件名。");
  const template = createTemplate(kind);
  const gain = Number(gainDb);
  if (
    kind !== "denoise" &&
    (!gainDb.trim() || !Number.isFinite(gain) || gain < -24 || gain > 12)
  ) {
    throw new Error("增益应在 -24 到 12 dB 之间。");
  }
  for (const node of template.graph.nodes) {
    if (node.id === "input") node.parameters = { path: input.trim() };
    if (node.id === "output") node.parameters = { path: output.trim() };
    if (node.id === "gain") node.parameters = { gain_db: gain };
  }
  return template;
}

function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
export function resultFields(
  result: unknown,
): { name: string; type: string; value: string; file: boolean }[] {
  if (!object(result) || !object(result.outputs)) return [];
  return Object.entries(result.outputs).flatMap(([name, output]) => {
    if (!object(output) || typeof output.type !== "string") return [];
    const value = output.value;
    if (!["string", "number", "boolean"].includes(typeof value)) return [];
    return [
      {
        name,
        type: output.type,
        value: String(value),
        file: output.type === "FilePath",
      },
    ];
  });
}

// Display links, not node-array order: a DAG is not necessarily a linear processing chain.
export function graphConnections(graph: GraphDocument): string[] {
  return graph.connections.map(
    ({ from, to }) => `${from.node}.${from.port} → ${to.node}.${to.port}`,
  );
}
