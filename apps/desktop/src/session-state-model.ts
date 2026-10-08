import type { CanvasView } from "./graph-canvas-layout";
import type { Mode } from "./model";
import type { PageId } from "./presentation";
import { editorWorkspaceContext, transitionEditorWorkspace } from "./editor-source-model";

export type SessionSource = { workspace: string; path: string; revision: string; content: string; space: "user" | "ai" };
export type SessionState = {
  schema_version: 1; workspace: string | null; page: PageId; editorKind: "graph" | "workflow";
  // Absent in older recovery records; both permissions then default to false.
  permissions?: { allowDevices: boolean; allowMonitor: boolean };
  graph: { text: string; mode: Mode; blockFrames: string; duration: string; canvasView: CanvasView; source: SessionSource | null };
  workflow: { text: string; sourceSpace: "user" | "ai"; path: string; source: SessionSource | null };
};
const bytes = (text: string) => new TextEncoder().encode(text).length;
export function validateSessionState(value: unknown): SessionState {
  const state = value as SessionState;
  const fail = () => { throw new Error("恢复文件格式无效；自动保存已暂停，请重置恢复文件。"); };
  const keys = (obj: unknown, allowed: string[]) => { if (!obj || typeof obj !== "object" || Array.isArray(obj) || Object.keys(obj).length !== allowed.length || Object.keys(obj).some(key => !allowed.includes(key))) fail(); };
  const text = (s: unknown, limit: number) => { if (typeof s !== "string" || bytes(s) > limit) fail(); };
  const point = (p: { x: number; y: number }) => { if (!p || !Number.isFinite(p.x) || !Number.isFinite(p.y) || Math.abs(p.x) > 1e7 || Math.abs(p.y) > 1e7) fail(); };
  if (!state || state.schema_version !== 1 || !["workbench", "editor", "files", "tasks", "settings"].includes(state.page) || !["graph", "workflow"].includes(state.editorKind)) fail();
  const hasPermissions = Object.prototype.hasOwnProperty.call(state, "permissions");
  keys(state, ["schema_version", "workspace", "page", "editorKind", "graph", "workflow", ...(hasPermissions ? ["permissions"] : [])]);
  if (hasPermissions) {
    keys(state.permissions, ["allowDevices", "allowMonitor"]);
    const permissions = state.permissions!;
    if (typeof permissions.allowDevices !== "boolean" || typeof permissions.allowMonitor !== "boolean" || (permissions.allowMonitor && !permissions.allowDevices)) fail();
  }
  if (state.workspace !== null) text(state.workspace, 4096);
  if (!state.graph || !state.workflow) fail();
  keys(state.graph, ["text", "mode", "blockFrames", "duration", "canvasView", "source"]);
  keys(state.workflow, ["text", "sourceSpace", "path", "source"]);
  text(state.graph.text, 4 * 1024 * 1024); text(state.graph.blockFrames, 32); text(state.graph.duration, 32);
  if (!["offline", "streaming", "realtime"].includes(state.graph.mode)) fail();
  text(state.workflow.text, 64 * 1024); text(state.workflow.path, 4096);
  if (!["user", "ai"].includes(state.workflow.sourceSpace)) fail();
  for (const [source, limit, graph] of [[state.graph.source, 4 * 1024 * 1024, true], [state.workflow.source, 64 * 1024, false]] as const) {
    if (source === null) continue;
    if (!source || !["user", "ai"].includes(source.space) || (graph && source.space !== "user")) fail();
    keys(source, ["workspace", "path", "revision", "content", "space"]);
    text(source.workspace, 4096); text(source.path, 4096); text(source.revision, 64); text(source.content, limit);
    const identity = (workspace: string | null) => editorWorkspaceContext(transitionEditorWorkspace({ workspace: null, scope: "" }, null, workspace).owner);
    if (!/^[0-9a-f]{64}$/.test(source.revision) || identity(source.workspace) !== identity(state.workspace)) fail();
  }
  const canvas = state.graph.canvasView;
  keys(canvas, ["positions", "selectedId", "zoom", "pan"]);
  if (!canvas || !canvas.positions || typeof canvas.positions !== "object" || Array.isArray(canvas.positions) || Object.keys(canvas.positions).length > 4096 || !Number.isFinite(canvas.zoom) || canvas.zoom < .25 || canvas.zoom > 4) fail();
  text(canvas.selectedId, 1024); point(canvas.pan);
  for (const [key, position] of Object.entries(canvas.positions)) { text(key, 1024); point(position); }
  if (bytes(JSON.stringify(state)) > 20 * 1024 * 1024) fail();
  return state;
}

/** One writer; a newer pending snapshot supersedes the older pending snapshot. */
export function createSessionWriter(save: (state: SessionState) => Promise<void>) {
  let pending: SessionState | null = null;
  let running: Promise<void> | null = null;
  function flush(): Promise<void> {
    if (running) return running.then(flush);
    if (!pending) return Promise.resolve();
    const state = pending; pending = null;
    running = save(state).finally(() => { running = null; });
    return running.then(flush);
  }
  return { enqueue(state: SessionState) { pending = validateSessionState(state); }, flush };
}
