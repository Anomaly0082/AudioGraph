import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import { formatError, type GraphDocument } from "../model";
import type {
  Connection,
  DisconnectedEvent,
  DisconnectReport,
  Reply,
} from "../types/desktop";

export const desktopAvailable = isTauri;
export const invokeDesktop = <T>(
  command: string,
  args: Record<string, unknown> = {},
): Promise<T> => invoke<T>(command, args);
export function listenBackendDisconnected(
  callback: (event: DisconnectedEvent) => void,
): Promise<() => void> {
  return listen<DisconnectedEvent>("backend-disconnected", (event) =>
    callback(event.payload),
  );
}
export function connectDesktop(
  workspace: string,
  allowDevices: boolean,
  allowMonitor: boolean,
): Promise<Connection> {
  return invokeDesktop("connect", { workspace, allowDevices, allowMonitor });
}
export function disconnectDesktop(
  sessionId: string,
): Promise<DisconnectReport> {
  return invokeDesktop("disconnect", { sessionId });
}
export function controlRequest<T = Record<string, unknown>>(
  sessionId: string,
  request: Record<string, unknown>,
): Promise<Reply<T>> {
  return invokeDesktop("control_request", { sessionId, request });
}
export function requireSuccess<T>(reply: Reply<T>): T {
  if (!reply.success) throw new Error(formatError(reply.errors ?? reply));
  return reply.data ?? ({} as T);
}
export function loadGraphDesktop(
  sessionId: string,
  path: string,
): Promise<{ path: string; graph: unknown }> {
  return invokeDesktop("load_graph", { sessionId, path });
}
export function saveGraphDesktop(
  sessionId: string,
  path: string,
  graph: GraphDocument,
): Promise<{ path: string }> {
  return invokeDesktop("save_graph", { sessionId, path, graph });
}
export async function chooseWorkspaceDirectory(): Promise<string | null> {
  const selected = await open({
    directory: true,
    multiple: false,
    title: "选择音频项目工作目录",
  });
  return typeof selected === "string" ? selected : null;
}
export async function chooseGraphFile(
  workspace: string,
): Promise<string | null> {
  const selected = await open({
    multiple: false,
    defaultPath: workspace,
    filters: [{ name: "Graph JSON", extensions: ["json"] }],
  });
  return typeof selected === "string" ? selected : null;
}
export async function chooseAudioFile(
  workspace?: string,
): Promise<string | null> {
  const selected = await open({
    multiple: false,
    defaultPath: workspace,
    filters: [{ name: "WAV 音频", extensions: ["wav"] }],
  });
  return typeof selected === "string" ? selected : null;
}
export function chooseNewGraphFile(workspace: string): Promise<string | null> {
  return save({
    defaultPath: `${workspace}/graph-new.json`,
    filters: [{ name: "Graph JSON", extensions: ["json"] }],
  });
}
export function confirmDesktop(
  message: string,
  title: string,
  kind: "warning" | "info" = "warning",
): Promise<boolean> {
  return isTauri()
    ? confirm(message, { title, kind })
    : Promise.resolve(window.confirm(message));
}
