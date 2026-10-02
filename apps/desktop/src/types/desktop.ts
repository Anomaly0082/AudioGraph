import type { GraphDocument, Mode, TaskState } from "../model";

export type NodeInfo = {
  typeId: string;
  displayName: string;
  description?: string;
  execution_domain: string;
  plugin?: {
    id: string;
    implementation_version: string;
    package_sha256: string;
    abi: { major: number; minor: number };
    capabilities: { id: string; version: number }[];
  };
  inputs: { id: string; type: string; required?: boolean }[];
  outputs: { id: string; type: string; required?: boolean }[];
  realtime_capabilities?: {
    format: {
      sample_rate: number;
      channels: number;
      sample_type: string;
      layout: string;
    };
    maximum_block_frames: number;
    supports_variable_blocks: boolean;
    offline_drivable: boolean;
  };
  parameters?: {
    id: string;
    type: string;
    description?: string;
    required?: boolean;
    default?: unknown;
    minimum?: number;
    maximum?: number;
    unit?: string;
    enum?: string[];
    integer_only?: boolean;
  }[];
};
export type Connection = {
  sessionId: string;
  workspace: string;
  allowDevices: boolean;
  allowMonitor: boolean;
  previousForcedDisconnect?: boolean;
  capabilities: {
    nodes: NodeInfo[];
    plugin_directory?: string;
    plugins?: {
      snapshot_id: string;
      available: {
        plugin_id: string;
        plugin_version: string;
        package_sha256: string;
      }[];
      errors: { package: string; code: string; message: string }[];
    };
  };
};
export type Device = { id: string; name: string; is_default: boolean };
export type DeviceCatalog = { inputs: Device[]; outputs: Device[] };
export type Reply<T = Record<string, unknown>> = {
  success: boolean;
  data?: T;
  errors?: unknown[];
  record_warnings?: string[];
};
export type GraphSubmission = {
  mode: Mode;
  graph: GraphDocument;
  options: Record<string, number | boolean>;
};
export type TaskView = {
  id: string;
  runId?: string;
  sessionId: string;
  state: TaskState | "unknown";
  errors?: unknown[];
  result?: unknown;
  resultRead?: boolean;
  released?: boolean;
  cleanupBusy?: boolean;
  cleanupError?: string;
  submission: GraphSubmission;
};
export type DisconnectedEvent = {
  sessionId: string;
  message: string;
  forced?: boolean;
};
export type DisconnectReport = { forced: boolean; message: string };
export type AudioInspection = {
  path: string;
  sample_rate: number;
  channels: number;
  frame_count: number;
  duration_seconds: number;
  encoding: "pcm_s16le";
};
