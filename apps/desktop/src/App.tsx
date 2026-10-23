import { useRef, useState } from "react";
import {
  choosePluginDirectory,
  confirmDesktop,
  invokeDesktop,
} from "./api/desktop";
import { canCancel, formatError, parseGraph } from "./model";
import { stateLabels, type PageId } from "./presentation";
import { useAudioSession } from "./hooks/useAudioSession";
import { useGraphDraft } from "./hooks/useGraphDraft";
import { useAiSettings } from "./hooks/useAiSettings";
import { useAgentTools } from "./hooks/useAgentTools";
import { useRunRecords } from "./hooks/useRunRecords";
import { useWorkflowEditor } from "./hooks/useWorkflowEditor";
import { useWorkspaceBrowser } from "./hooks/useWorkspaceBrowser";
import { usePluginManager } from "./hooks/usePluginManager";
import { useSessionState } from "./hooks/useSessionState";
import { workspaceRelativeSavePath } from "./editor-source-model";
import type { BrowserSpace } from "./workspace-browser-model";
import AppShell from "./components/AppShell";
import WorkspacePanel from "./components/WorkspacePanel";
import WorkbenchPage from "./pages/WorkbenchPage";
import EditorPage, { type EditorSelection } from "./pages/EditorPage";
import RunRecordsPage from "./pages/RunRecordsPage";
import SettingsPage from "./pages/SettingsPage";
import WorkflowEditorPage from "./pages/WorkflowEditorPage";
import WorkspaceBrowserPage from "./pages/WorkspaceBrowserPage";

// Controllers live above page navigation. Views can unmount without losing a session,
// draft or in-flight request. No page owns or restarts the backend process.
export default function App() {
  const [page, setPage] = useState<PageId>("workbench");
  const [editorKind, setEditorKind] = useState<"graph" | "workflow">("graph");
  const workflowBusyRef = useRef<() => boolean>(() => false);
  const fileMutationRef = useRef<() => boolean>(() => false);
  const [workspaceOpen, setWorkspaceOpen] = useState(false);
  const [selection, setSelection] = useState<EditorSelection>({
    search: "",
  });
  const [viewBusy, setViewBusy] = useState(false);
  const viewBusyRef = useRef(false);
  const recoveryBlockedRef = useRef(true);
  const rememberedWorkspace = useRef<string | null>(null);
  const session = useAudioSession();
  const plugins = usePluginManager(
    {
      invoke: invokeDesktop,
      chooseDirectory: choosePluginDirectory,
      confirm: confirmDesktop,
    },
    session.desktop,
  );
  const draft = useGraphDraft(session.connection?.sessionId ?? null, session.connection?.workspace ?? "");
  const settings = useAiSettings(invokeDesktop);
  const agent = useAgentTools({
    connection: session.connection,
    settings,
    blocked: () =>
      recoveryBlockedRef.current ||
      fileMutationRef.current() ||
      workflowBusyRef.current() ||
      viewBusyRef.current ||
      !!session.readBusy() ||
      session.readTaskBlocked() ||
      !session.desktop,
  });
  const workflowEditor = useWorkflowEditor(
    session.connection,
    () =>
      recoveryBlockedRef.current ||
      fileMutationRef.current() ||
      viewBusyRef.current ||
      agent.readBusy() ||
      !!session.readBusy() ||
      session.readTaskBlocked() ||
      !session.desktop,
  );
  workflowBusyRef.current = workflowEditor.readBusy;
  const files = useWorkspaceBrowser(session.connection, page === "files", {
    enabled: session.desktop,
    blockedReason: () => {
      if (recoveryBlockedRef.current || viewBusyRef.current) return "当前编辑操作尚未完成，请稍后重试。";
      if (agent.readBusy() || workflowEditor.readBusy()) return "请先停止或等待 AI / Workflow 完成，再修改文件。";
      if (session.readBusy() || session.readTaskBlocked()) return "请先停止当前任务并完成收尾，再修改文件。";
      return null;
    },
    protectedPaths: () => {
      const paths: {space: BrowserSpace; path: string}[] = [];
      const graphSource = draft.exportSessionState().source;
      if (draft.modified && graphSource && session.connection) {
        try { paths.push({space: "user", path: workspaceRelativeSavePath(session.connection.workspace, graphSource.path)}); }
        catch { /* A stale source outside this workspace cannot be selected here. */ }
      }
      const workflowSource = workflowEditor.exportSessionState().source;
      if (workflowEditor.modified && workflowSource) paths.push({space: workflowSource.space, path: workflowSource.path});
      return paths;
    },
  });
  fileMutationRef.current = files.readMutating;
  const records = useRunRecords(
    session.connection?.sessionId,
    page === "tasks",
    session.taskActive ||
      session.cleanupBusy ||
      agent.busy ||
      workflowEditor.busy,
    `${session.task?.id ?? ""}:${session.task?.state ?? ""}:${session.task?.released ?? false}:${workflowEditor.lastRunId ?? ""}`,
  );
  if (session.connection) rememberedWorkspace.current = session.connection.workspace;
  const recovery = useSessionState(session.desktop, () => ({
    schema_version: 1,
    workspace: rememberedWorkspace.current,
    page, editorKind,
    permissions: { allowDevices: session.allowDevices, allowMonitor: session.allowMonitor },
    graph: draft.exportSessionState(),
    workflow: workflowEditor.exportSessionState(),
  }), async state => {
    rememberedWorkspace.current = state.workspace;
    draft.restoreSessionState(state.graph, state.workspace);
    workflowEditor.restoreSessionState(state.workflow, state.workspace);
    setPage(state.page); setEditorKind(state.editorKind);
    // Apply before connecting so the checkboxes and native session agree.
    // Permissions alone do not start a task or open an audio stream.
    await session.applyPermissions(state.permissions ?? { allowDevices: false, allowMonitor: false });
    if (state.workspace) {
      session.setWorkspace(state.workspace);
      try { await session.connectBackend(state.workspace); }
      catch (reason) { session.setError(formatError(reason)); }
    }
  });
  recoveryBlockedRef.current = !recovery.ready || recovery.closing;
  const goWorkbench = () => {
    setPage("workbench");
  };

  // Dialog/draft operations have their own guard; navigation and reading stay available.
  async function viewAction(action: () => Promise<unknown> | unknown) {
    if (recoveryBlockedRef.current || viewBusyRef.current || fileMutationRef.current()) return;
    viewBusyRef.current = true;
    setViewBusy(true);
    try {
      await action();
    } catch (reason) {
      session.setError(formatError(reason));
    } finally {
      viewBusyRef.current = false;
      setViewBusy(false);
    }
  }
  function sessionAction(action: () => Promise<unknown>) {
    if (recoveryBlockedRef.current || fileMutationRef.current()) return;
    void action().catch((reason) => session.setError(formatError(reason)));
  }
  async function validateDraft() {
    if (workflowEditor.readBusy()) throw new Error("请先结束 Workflow 操作。");
    if (agent.readBusy()) throw new Error("请先停止工具助手。");
    const id = session.connection?.sessionId;
    if (!id) return;
    const key = draft.validationKey;
    if (await session.validateSubmission(draft.buildSubmission()))
      draft.acceptValidation(id, key);
  }
  async function runDraft() {
    if (workflowEditor.readBusy()) throw new Error("请先结束 Workflow 操作。");
    if (agent.readBusy()) throw new Error("请先停止工具助手。");
    const id = await session.startSubmission(draft.buildSubmission(), {
      validationCurrent: draft.validationCurrent,
    });
    if (id) setPage("tasks");
  }
  const submittingLocked =
    files.mutating ||
    !!session.busy ||
    session.taskBlocked ||
    viewBusy ||
    agent.busy ||
    workflowEditor.busy;
  const cancelCurrent = () =>
    sessionAction(() =>
      session.cancelTask(session.task?.id, session.task?.sessionId),
    );

  if (!recovery.ready || recovery.closing) return <div className="banner info" role="status">{recovery.closing ? "正在保存编辑状态并退出…" : "正在恢复编辑状态…"}</div>;
  return (
    <AppShell
      page={page}
      onNavigate={setPage}
      workspace={session.connection?.workspace ?? null}
      onWorkspace={() => setWorkspaceOpen((open) => !open)}
      noticesExpanded={workspaceOpen}
      status={session.notice && <span className="topbar-status" role="status" title={session.notice}>{session.notice}</span>}
      notices={
        <>
          {recovery.error && <div className="banner warning" role="alert">{recovery.error}{recovery.disabled && <button onClick={() => void recovery.reset()}>重置恢复文件</button>}</div>}
          {!session.desktop && (
            <div className="banner preview">
              浏览器仅用于界面预览。文件、模型和任务操作需要桌面程序。
            </div>
          )}
          {workspaceOpen && (
            <WorkspacePanel
              session={session}
              aiBusy={viewBusy || agent.busy || workflowEditor.busy || files.mutating}
              onClose={() => setWorkspaceOpen(false)}
              onChoose={() => {
                if (!viewBusyRef.current && !agent.readBusy() && !workflowEditor.readBusy())
                  sessionAction(() => session.chooseWorkspace());
              }}
              onConnect={(path) =>
                sessionAction(async () => {
                  if (viewBusyRef.current || agent.readBusy() || workflowEditor.readBusy()) return;
                  await session.connectBackend(path);
                  setWorkspaceOpen(false);
                })
              }
              onDisconnect={() => {
                if (!viewBusyRef.current && !agent.readBusy() && !workflowEditor.readBusy())
                  sessionAction(() => session.disconnectBackend());
              }}
              onSettings={() => setPage("settings")}
            />
          )}
          {session.error && (
            <div className="banner error" role="alert">
              <strong>操作未完成</strong>
              <pre>{session.error}</pre>
              <button onClick={() => session.setError("")}>关闭</button>
            </div>
          )}
          {!workspaceOpen && session.recentWorkspaces.warning && (
            <div className="banner warning" role="alert">{session.recentWorkspaces.warning}</div>
          )}
          {session.forcedWarning && (
            <div className="banner warning" role="alert">
              {session.forcedWarning}
              <button onClick={() => session.setForcedWarning("")}>
                已知晓
              </button>
            </div>
          )}
          {session.cleanupError && (
            <div className="banner warning" role="alert">
              <span>{session.cleanupError}</span>
              <button
                disabled={
                  session.cleanupBusy ||
                  !!session.busy ||
                  agent.busy ||
                  workflowEditor.busy
                }
                onClick={() => sessionAction(() => session.retryCleanup())}
              >
                重试收尾
              </button>
            </div>
          )}
        </>
      }
      activity={
        (session.taskActive ||
          session.cleanupBusy ||
          agent.busy ||
          workflowEditor.busy || !!files.conversionState) && (
          <div className="global-activity" role="status">
            <span className="activity-pulse" />
            <span>
              {files.conversionState ? (files.conversionState.phase === "cancelling" ? "正在取消音频转换…" : "正在转换音频…") : agent.busy
                ? "AI 工具调用中"
                : workflowEditor.busy
                  ? workflowEditor.action === "stopping"
                    ? "Workflow 正在停止…"
                    : "Workflow 操作中"
                  : session.cleanupBusy
                    ? "正在保存结果并完成收尾…"
                    : session.taskActive && session.task
                      ? session.task.id +
                        " · " +
                        stateLabels[session.task.state]
                      : "处理中"}
            </span>
            <button
              onClick={
                files.conversionState ? () => setPage("files") : agent.busy
                  ? goWorkbench
                  : workflowEditor.busy
                    ? () => {
                        setEditorKind("workflow");
                        setPage("editor");
                      }
                    : session.taskActive || session.cleanupBusy
                      ? () => setPage("tasks")
                      : goWorkbench
              }
            >
              {files.conversionState ? "查看文件" : agent.busy
                ? "查看工具助手"
                : workflowEditor.busy
                  ? "查看 Workflow"
                  : session.taskActive || session.cleanupBusy
                    ? "查看任务"
                    : "查看工作台"}
            </button>
            {files.conversionState ? <button className="danger" disabled={files.conversionState.phase === "cancelling"} onClick={() => void files.cancelConversion()}>取消转换</button> : agent.busy
              ? agent.canStop && (
                  <button
                    className="danger"
                    disabled={agent.stopping}
                    onClick={() => void agent.stop()}
                  >
                    停止
                  </button>
                )
              : workflowEditor.busy
                ? ["running", "stopping"].includes(
                    workflowEditor.action ?? "",
                  ) && (
                    <button
                      className="danger"
                      disabled={workflowEditor.action === "stopping"}
                      onClick={() => void workflowEditor.stop()}
                    >
                      停止 Workflow
                    </button>
                  )
                : session.task &&
                  session.task.sessionId === session.connection?.sessionId &&
                  canCancel(session.task.state) && (
                    <button
                      className="danger"
                      disabled={!!session.busy}
                      onClick={cancelCurrent}
                    >
                      取消任务
                    </button>
                  )}
          </div>
        )
      }
    >
      {page === "workbench" && (
        <WorkbenchPage
          connected={!!session.connection}
          onOpenSettings={() => setPage("settings")}
          onOpenRun={(id) => {
            records.select(id);
            setPage("tasks");
          }}
          agent={agent}
          agentBlocked={
            files.mutating ||
            viewBusy ||
            workflowEditor.busy ||
            !!session.busy ||
            session.taskBlocked ||
            !session.desktop
          }
          onAgentSend={() => {
            try {
              void agent.send(
                agent.attachGraph ? draft.buildSubmission() : undefined,
              );
            } catch (reason) {
              agent.setError(formatError(reason));
            }
          }}
          onAgentApply={(graph) =>
            void viewAction(async () => {
              if (agent.busy) return;
              if (
                !(await confirmDesktop(
                  "将工具写出的 Graph 载入编辑器并替换当前草稿？不会执行。",
                  "载入 Graph",
                ))
              )
                return;
              draft.replaceGraph(JSON.stringify(graph, null, 2), "AI Graph 副本（未保存）");
              setEditorKind("graph");
              setPage("editor");
            })
          }
          onWorkflowApply={(text, label) =>
            void viewAction(async () => {
              if (agent.readBusy() || workflowEditor.readBusy()) return;
              const expected = workflowEditor.captureSnapshot();
              if (
                !(await confirmDesktop(
                  "将生成的 Workflow 载入编辑器并替换当前草稿？不会执行。",
                  "载入 Workflow",
                ))
              )
                return;
              if (
                !workflowEditor.isCurrentSnapshot(expected) ||
                !workflowEditor.applyText(text, label)
              )
                return;
              setEditorKind("workflow");
              setPage("editor");
            })
          }
        />
      )}
      {page === "editor" && (
        <div className="page-stack">
          <div className="section-tabs" role="group" aria-label="配置类型">
            <button
              type="button"
              className={editorKind === "graph" ? "active" : ""}
              aria-pressed={editorKind === "graph"}
              onClick={() => setEditorKind("graph")}
            >
              Graph
            </button>
            <button
              type="button"
              className={editorKind === "workflow" ? "active" : ""}
              aria-pressed={editorKind === "workflow"}
              onClick={() => setEditorKind("workflow")}
            >
              Workflow
            </button>
          </div>
          {editorKind === "workflow" ? (
            <WorkflowEditorPage
              editor={workflowEditor}
              connected={!!session.connection}
              locked={
                files.mutating ||
                viewBusy ||
                !!session.busy ||
                session.taskBlocked ||
                agent.busy ||
                !session.desktop
              }
              onOpenRun={(id) => {
                records.select(id);
                setPage("tasks");
              }}
            />
          ) : (
            <EditorPage
              draft={draft}
              session={session}
              selection={selection}
              onSelection={setSelection}
              submittingLocked={submittingLocked}
              fileBusy={viewBusy || files.mutating}
              onValidate={() => void viewAction(validateDraft)}
              onRun={() => void viewAction(runDraft)}
              onLoadTemplate={() => void viewAction(() => draft.loadTemplate())}
              onLoad={() =>
                void viewAction(
                  () =>
                    session.connection && draft.loadGraph(session.connection),
                )
              }
              onSave={() =>
                void viewAction(async () => {
                  if (!session.connection) return;
                  const saved = await draft.saveGraph(session.connection);
                  if (saved)
                    session.setNotice("Graph 已保存：" + saved);
                })
              }
              onSaveAs={() =>
                void viewAction(async () => {
                  if (!session.connection) return;
                  const saved = await draft.saveGraph(session.connection, true);
                  if (saved) session.setNotice("Graph 已另存为新文件：" + saved);
                })
              }
              onDevices={() => sessionAction(() => session.listDevices())}
              onTasks={() => setPage("tasks")}
            />
          )}
        </div>
      )}
      {page === "tasks" && (
        <RunRecordsPage
          records={records}
          connected={!!session.connection}
          currentTask={
            session.task &&
            session.task.sessionId === session.connection?.sessionId
              ? {
                  runId: session.task.runId,
                  state: session.task.state,
                  busy:
                    !!session.busy ||
                    session.cleanupBusy ||
                    agent.busy ||
                    workflowEditor.busy,
                  onStop: cancelCurrent,
                }
              : undefined
          }
        />
      )}
      {page === "files" && (
        <WorkspaceBrowserPage
          browser={files}
          connected={!!session.connection}
          onOpenGraph={(text, label) =>
            void viewAction(async () => {
              const selectedFile = files.capturePreview();
              const draftKey = draft.captureDraftKey();
              const graph = parseGraph(text, { allowEmpty: true });
              if (
                !(await confirmDesktop(
                  selectedFile?.space === "ai"
                    ? "将 AI 文件作为未保存的 Graph 副本载入编辑器？保存时需选择用户工作区的新路径。"
                    : "将此文件载入 Graph 编辑器并替换草稿？保存会更新这个用户工作区文件。",
                  "载入 Graph",
                ))
              )
                return;
              if (
                !files.isCurrentPreview(selectedFile) ||
                !draft.isCurrentDraftKey(draftKey)
              )
                return;
              if (!selectedFile || !session.connection) return;
              if (selectedFile.space === "user") {
                if (!(await draft.loadFromFile(session.connection, selectedFile.path, text, () => files.isCurrentPreview(selectedFile)))) return;
              } else {
                draft.replaceGraph(JSON.stringify(graph, null, 2), "AI Graph 副本（未保存）：" + label);
              }
              setEditorKind("graph");
              setPage("editor");
              session.setNotice("已载入配置：" + label);
            })
          }
          onOpenWorkflow={(text, label) =>
            void viewAction(async () => {
              if (workflowEditor.readBusy()) return;
              const selectedFile = files.capturePreview();
              const expected = workflowEditor.captureSnapshot();
              if (
                !(await confirmDesktop(
                  "将此文件载入 Workflow 编辑器并替换草稿？不会执行。",
                  "载入 Workflow",
                ))
              )
                return;
              if (
                !files.isCurrentPreview(selectedFile) ||
                !workflowEditor.isCurrentSnapshot(expected)
              )
                return;
              if (!selectedFile || !(await workflowEditor.loadFromFile(selectedFile.space, selectedFile.path, text, () => files.isCurrentPreview(selectedFile)))) return;
              setEditorKind("workflow");
              setPage("editor");
            })
          }
        />
      )}
      {page === "settings" && (
        <SettingsPage
          settings={settings}
          session={session}
          plugins={plugins}
          workflowBusy={viewBusy || agent.busy || workflowEditor.busy || files.mutating}
          onPermissions={(desired) => sessionAction(() => session.applyPermissions(desired, () => viewBusyRef.current || agent.readBusy() || workflowEditor.readBusy() || fileMutationRef.current()))}
        />
      )}
    </AppShell>
  );
}
