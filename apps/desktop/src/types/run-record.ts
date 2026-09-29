export type RunSummary = {
  schema_version?: number;
  id: string;
  kind: "graph" | "workflow";
  origin: "manual" | "ai";
  parent_id: string | null;
  name: string;
  started_at_ms: number;
  finished_at_ms: number | null;
  duration_ms: number | null;
  state: string;
};

export type RunFile = {
  space: "user" | "ai";
  path: string;
  role: string;
  size_bytes: number | null;
  sha256: string | null;
  capture_status: string;
  message?: string | null;
};

export type RunRecord = RunSummary & {
  configuration: unknown;
  result: unknown;
  error: string | null;
  recording_warning?: string | null;
  files: RunFile[];
};

export type RunFileCheck = {
  space: "user" | "ai";
  path: string;
  role: string;
  status: string;
  message?: string | null;
};

export type RunList = {
  records: RunSummary[];
  warnings: string[];
  truncated: boolean;
};
