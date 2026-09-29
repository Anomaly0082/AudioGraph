import { useRef, useState } from "react";
import { confirmDesktop, invokeDesktop } from "./api/desktop";
import { canCancel, formatError } from "./model";
import { preparePreset, stateLabels, type PageId } from "./presentation";
import { useAudioSession } from "./hooks/useAudioSession";
import { useGraphDraft } from "./hooks/useGraphDraft";
import { useAiSettings } from "./hooks/useAiSettings";
import { useAudioInput } from "./hooks/useAudioInput";
import { useAgentTools } from "./hooks/useAgentTools";
import { useRunRecords } from "./hooks/useRunRecords";
import AppShell from "./components/AppShell";
import WorkspacePanel from "./components/WorkspacePanel";
import WorkbenchPage, { type WorkbenchSetup } from "./pages/WorkbenchPage";
import EditorPage, { type EditorSelection } from "./pages/EditorPage";
import TasksPage from "./pages/TasksPage";
import RunRecordsPage from "./pages/RunRecordsPage";
import SettingsPage from "./pages/SettingsPage";

// Controllers live above page navigation. Views can unmount without losing a session,
// draft or in-flight request. No page owns or restarts the backend process.
export default function App() {
  const [page, setPage] = useState<PageId>("workbench");
  const [workspaceOpen, setWorkspaceOpen] = useState(false);
  const [inputPath, setInputPath] = useState("");
  const [setup, setSetup] = useState<WorkbenchSetup>({
    approach: "ai",
    preset: "denoise",
    output: "processed.wav",
    gain: "-6",
  });
  const [selection, setSelection] = useState<EditorSelection>({
    search: "",
  });
  const [viewBusy, setViewBusy] = useState(false);
  const viewBusyRef = useRef(false);
  const session = useAudioSession();
  const draft = useGraphDraft(session.connection?.sessionId ?? null);
  const settings = useAiSettings(invokeDesktop);
  const agent = useAgentTools({
    connection: session.connection,
    settings,
    blocked: () =>
      viewBusyRef.current ||
      !!session.readBusy() ||
      !!session.readTask() ||
      !session.desktop,
  });
  const records = useRunRecords(
    session.connection?.sessionId,
    page === "tasks",
    session.taskActive || agent.busy,
  );
  const input = useAudioInput({
    path: inputPath,
    setPath: setInputPath,
    connection: session.connection,
    inspect: session.inspectAudio,
  });
  const goWorkbench = () => {
    setSetup((current) => ({ ...current, approach: "ai" }));
    setPage("workbench");
  };

  // Dialog/draft operations have their own guard; navigation and reading stay available.
  async function viewAction(action: () => Promise<unknown> | unknown) {
    if (viewBusyRef.current) return;
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
    void action().catch((reason) => session.setError(formatError(reason)));
  }
  async function validateDraft() {
    if (agent.busy) throw new Error("请先停止工具助手。");
    const id = session.connection?.sessionId;
    if (!id) return;
    const key = draft.validationKey;
    if (await session.validateSubmission(draft.buildSubmission()))
      draft.acceptValidation(id, key);
  }
  async function runDraft() {
    if (agent.busy) throw new Error("请先停止工具助手。");
    const id = await session.startSubmission(draft.buildSubmission(), {
      validationCurrent: draft.validationCurrent,
    });
    if (id) setPage("tasks");
  }
  async function prepareTemplate() {
    const value = preparePreset(
      setup.preset,
      inputPath,
      setup.output,
      setup.gain,
    );
    if (
      !(await confirmDesktop(
        "将准备好的方案载入编辑器并替换当前草稿？不会立即执行。",
        "准备 Graph",
      ))
    )
      return;
    draft.applyProposal({
      mode: value.mode === "streaming" ? "streaming" : "offline",
      graph: value.graph,
      options: value.mode === "streaming" ? { block_frames: 256 } : {},
    });
    setPage("editor");
    session.setNotice("方案已准备，请查看节点和文件路径，再校验、提交任务。");
  }
  const submittingLocked =
    !!session.busy || session.taskActive || viewBusy || agent.busy;
  const cancelCurrent = () =>
    sessionAction(() => session.cancelTask(session.task?.id));

  return (
    <AppShell
      page={page}
      onNavigate={setPage}
      workspace={session.connection?.workspace ?? null}
      onWorkspace={() => setWorkspaceOpen((open) => !open)}
      notices={
        <>
          {!session.desktop && (
            <div className="banner preview">
              浏览器仅用于界面预览。文件、模型和任务操作需要桌面程序。
            </div>
          )}
          {workspaceOpen && (
            <WorkspacePanel
              session={session}
              aiBusy={viewBusy || agent.busy}
              onClose={() => setWorkspaceOpen(false)}
              onChoose={() => sessionAction(() => session.chooseWorkspace())}
              onConnect={() =>
                sessionAction(async () => {
                  await session.connectBackend();
                  setWorkspaceOpen(false);
                })
              }
              onDisconnect={() => {
                if (!agent.busy)
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
          {session.notice && (
            <div className="banner info" role="status">
              {session.notice}
            </div>
          )}
          {session.forcedWarning && (
            <div className="banner warning" role="alert">
              {session.forcedWarning}
              <button onClick={() => session.setForcedWarning("")}>
                已知晓
              </button>
            </div>
          )}
        </>
      }
      activity={
        (session.taskActive || agent.busy) && (
          <div className="global-activity" role="status">
            <span className="activity-pulse" />
            <span>
              {agent.busy
                ? "AI 工具调用中"
                : session.taskActive && session.task
                  ? session.task.id + " · " + stateLabels[session.task.state]
                  : "处理中"}
            </span>
            <button
              onClick={
                agent.busy
                  ? goWorkbench
                  : session.taskActive
                    ? () => setPage("tasks")
                    : goWorkbench
              }
            >
              {agent.busy
                ? "查看工具助手"
                : session.taskActive
                  ? "查看任务"
                  : "查看工作台"}
            </button>
            {agent.busy
              ? agent.canStop && (
                  <button
                    className="danger"
                    disabled={agent.stopping}
                    onClick={() => void agent.stop()}
                  >
                    停止
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
          input={input}
          connected={!!session.connection}
          desktop={session.desktop}
          setup={setup}
          onSetup={setSetup}
          onPrepare={() => void viewAction(prepareTemplate)}
          onOpenSettings={() => setPage("settings")}
          agent={agent}
          agentBlocked={
            viewBusy || !!session.busy || !!session.task || !session.desktop
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
              draft.editGraph(JSON.stringify(graph, null, 2));
              setPage("editor");
            })
          }
        />
      )}
      {page === "editor" && (
        <EditorPage
          draft={draft}
          session={session}
          selection={selection}
          onSelection={setSelection}
          submittingLocked={submittingLocked}
          fileBusy={viewBusy}
          onValidate={() => void viewAction(validateDraft)}
          onRun={() => void viewAction(runDraft)}
          onLoadTemplate={() => void viewAction(() => draft.loadTemplate())}
          onLoad={() =>
            void viewAction(
              () => session.connection && draft.loadGraph(session.connection),
            )
          }
          onSave={() =>
            void viewAction(async () => {
              if (!session.connection) return;
              const saved = await draft.saveGraph(session.connection);
              if (saved) session.setNotice("Graph 已另存为新文件：" + saved);
            })
          }
          onDevices={() => sessionAction(() => session.listDevices())}
          onTasks={() => setPage("tasks")}
        />
      )}
      {page === "tasks" && (
        <RunRecordsPage
          records={records}
          connected={!!session.connection}
          taskActive={session.taskActive}
          currentTask={
            session.task ? (
              <TasksPage
                session={session}
                aiBusy={agent.busy}
                onCancel={cancelCurrent}
                onRelease={() => {
                  if (!agent.busy) sessionAction(() => session.releaseTask());
                }}
                onWorkbench={goWorkbench}
                onEditor={() => setPage("editor")}
                onCopy={(value) =>
                  void viewAction(async () => {
                    await navigator.clipboard.writeText(value);
                    session.setNotice("输出路径已复制。");
                  })
                }
              />
            ) : undefined
          }
        />
      )}
      {page === "settings" && (
        <SettingsPage
          settings={settings}
          session={session}
          workflowBusy={agent.busy}
        />
      )}
    </AppShell>
  );
}
