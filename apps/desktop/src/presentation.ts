import type { GraphDocument, TemplateKind } from "./model";

export type PageId =
  | "workbench"
  | "editor"
  | "files"
  | "tasks"
  | "settings";
export const pages: { id: PageId; label: string; description: string }[] = [
  {
    id: "workbench",
    label: "处理工作台",
    description: "通过 AI 助手查看节点、编排配置和处理音频。",
  },
  {
    id: "editor",
    label: "编辑器",
    description: "编辑 Graph 或 Workflow 配置，校验后运行。",
  },
  {
    id: "files",
    label: "文件",
    description: "浏览工作区文件并在本机预览。",
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
