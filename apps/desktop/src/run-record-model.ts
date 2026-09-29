import type { RunSummary } from "./types/run-record";

export const runStateLabels: Record<string, string> = {
  running: "运行中",
  queued: "等待运行",
  cancelling: "取消中",
  succeeded: "已完成",
  failed: "失败",
  cancelled: "已取消",
  interrupted: "已中断（未恢复）",
  unknown: "状态未知",
  limited: "达到限制",
};
export const fileStateLabels: Record<string, string> = {
  available: "与记录一致",
  missing: "文件不可用",
  changed: "文件已变化",
  unverified: "无法验证",
  rejected: "路径不可访问",
  captured: "已保存指纹",
};

// If a bounded listing omits a parent, retain the child as a visible orphan.
// Never let incomplete/corrupt relationships hide an otherwise valid record.
export function groupRuns(records: RunSummary[]) {
  const roots: RunSummary[] = [];
  const children = new Map<string, RunSummary[]>();
  const workflows = new Set(
    records.filter((r) => r.kind === "workflow").map((r) => r.id),
  );
  for (const record of records) {
    if (
      record.kind === "graph" &&
      record.parent_id &&
      workflows.has(record.parent_id)
    ) {
      const group = children.get(record.parent_id) ?? [];
      group.push(record);
      children.set(record.parent_id, group);
    } else roots.push(record);
  }
  const newest = (a: RunSummary, b: RunSummary) =>
    b.started_at_ms - a.started_at_ms || a.id.localeCompare(b.id);
  roots.sort(newest);
  for (const group of children.values()) group.sort((a, b) => -newest(a, b));
  return { roots, children };
}

export function formatRunTime(value: number) {
  const date = new Date(value);
  return Number.isFinite(date.getTime())
    ? date.toLocaleString("zh-CN", { hour12: false })
    : "未知时间";
}
export function formatRunDuration(value: number | null) {
  if (value === null || !Number.isFinite(value)) return "—";
  return value < 1000 ? `${value} ms` : `${(value / 1000).toFixed(1)} 秒`;
}

export function matchesRunSelection(
  currentSession: string | undefined,
  currentId: string | null,
  requestedSession: string,
  requestedId: string,
) {
  return currentSession === requestedSession && currentId === requestedId;
}
