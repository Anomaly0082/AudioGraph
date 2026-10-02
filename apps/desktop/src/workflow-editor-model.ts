export type WorkflowSpace = "user" | "ai";
export type WorkflowEditorAction =
  | "loading"
  | "saving"
  | "validating"
  | "running"
  | "stopping";

export const blankWorkflowText = JSON.stringify(
  { schema_version: 1, inputs: {}, steps: [], outputs: {} },
  null,
  2,
);

export const workflowRunStateLabels: Record<string, string> = {
  succeeded: "成功",
  failed: "失败",
  cancelled: "已停止",
  limited: "达到运行限额",
  interrupted: "已中断",
  unknown: "状态未知",
};

export function workflowRunSucceeded(state: string): boolean {
  return state === "succeeded";
}

export type WorkflowSnapshot = Readonly<{
  sessionId: string;
  contextKey: string;
  epoch: number;
  revision: number;
  text: string;
}>;

export function createWorkflowSnapshot(
  value: WorkflowSnapshot,
): WorkflowSnapshot {
  return Object.freeze({ ...value });
}

export function sameWorkflowContext(
  expected: WorkflowSnapshot,
  current: WorkflowSnapshot | null,
): boolean {
  return (
    !!current &&
    expected.sessionId === current.sessionId &&
    expected.contextKey === current.contextKey &&
    expected.epoch === current.epoch
  );
}

export function isCurrentWorkflowSnapshot(
  expected: WorkflowSnapshot,
  current: WorkflowSnapshot | null,
): boolean {
  return (
    sameWorkflowContext(expected, current) &&
    expected.revision === current!.revision &&
    expected.text === current!.text
  );
}

export function canRunWorkflow(
  current: WorkflowSnapshot | null,
  validated: WorkflowSnapshot | null,
  busy: boolean,
  blocked: boolean,
): boolean {
  return (
    !busy &&
    !blocked &&
    !!validated &&
    isCurrentWorkflowSnapshot(validated, current)
  );
}

export function workflowRelativePath(value: string): string {
  const path = value.trim().replace(/\\/g, "/");
  if (
    !path ||
    path.startsWith("/") ||
    /^[a-zA-Z]:/.test(path) ||
    path.split("/").some((part) => part === ".." || part === "") ||
    path.includes(":")
  ) {
    throw new Error("请输入工作区内的相对文件路径，不可使用绝对路径或 ..。");
  }
  return path;
}
