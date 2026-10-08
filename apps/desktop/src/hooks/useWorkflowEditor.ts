import { useEffect, useRef, useState } from "react";
import { chooseNewWorkflowFile, confirmDesktop, invokeDesktop } from "../api/desktop";
import { editorModified, editorWorkspaceContext, transitionEditorWorkspace, savedEditorSource, sourceForContext, workspaceRelativeSavePath, type EditorSource, type EditorWorkspace } from "../editor-source-model";
import { formatError } from "../model";
import type { Connection } from "../types/desktop";
import type { SessionState } from "../session-state-model";
import {
  blankWorkflowText,
  canRunWorkflow,
  createWorkflowSnapshot,
  isCurrentWorkflowSnapshot,
  sameWorkflowContext,
  workflowRelativePath,
  workflowRunStateLabels,
  workflowRunSucceeded,
  type WorkflowEditorAction,
  type WorkflowSnapshot,
  type WorkflowSpace,
} from "../workflow-editor-model";

type ActiveRun = {
  requestId: string;
  snapshot: WorkflowSnapshot;
  settled: boolean;
  stopPending: boolean;
  stopRequested: boolean;
};

export function useWorkflowEditor(
  connection: Connection | null,
  blocked: () => boolean,
  contextKey = "",
) {
  const [text, setText] = useState(blankWorkflowText);
  const [fileLabel, setFileLabel] = useState("未保存的 Workflow");
  const [, setSource] = useState<EditorSource | null>(null);
  const sourceRef = useRef<EditorSource | null>(null);
  const [sourceSpace, setSourceSpace] = useState<WorkflowSpace>("user");
  const [path, setPath] = useState("");
  const [action, setAction] = useState<WorkflowEditorAction | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [lastRunId, setLastRunId] = useState<string | null>(null);
  const [lastState, setLastState] = useState<string | null>(null);
  const [validated, setValidated] = useState<WorkflowSnapshot | null>(null);
  const draftRef = useRef({ text: blankWorkflowText, revision: 0 });
  const validatedRef = useRef<WorkflowSnapshot | null>(null);
  const actionRef = useRef<WorkflowEditorAction | null>(null);
  const runRef = useRef<ActiveRun | null>(null);
  const mountedRef = useRef(true);
  const blockedRef = useRef(blocked);
  blockedRef.current = blocked;
  const connectionRef = useRef(connection);
  connectionRef.current = connection;
  const identity = JSON.stringify([
    connection?.sessionId ?? null,
    connection?.workspace ?? null,
    contextKey,
  ]);
  const workspaceRef = useRef<EditorWorkspace>({ workspace: null, scope: contextKey });
  const transition = transitionEditorWorkspace(workspaceRef.current, sourceRef.current, connection?.workspace ?? null, contextKey);
  workspaceRef.current = transition.owner;
  sourceRef.current = transition.source;
  const fileContext = editorWorkspaceContext(transition.owner);
  const contextRef = useRef({ identity, epoch: 0 });
  if (contextRef.current.identity !== identity) {
    contextRef.current = { identity, epoch: contextRef.current.epoch + 1 };
    validatedRef.current = null;
  }

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      const active = runRef.current;
      if (active) void requestStop(active);
    };
  }, []);
  useEffect(() => {
    setSource(sourceRef.current);
    setValidated(null);
    setError("");
    setNotice("");
    setLastRunId(null);
    setLastState(null);
    const active = runRef.current;
    if (active && !sameWorkflowContext(active.snapshot, snapshot())) {
      void requestStop(active);
    }
  }, [identity]);
  const previousFileContext = useRef(fileContext);
  useEffect(() => {
    if (previousFileContext.current === fileContext) return;
    previousFileContext.current = fileContext;
    setFileLabel("未保存的 Workflow");
    setPath("");
  }, [fileContext]);

  function snapshot(): WorkflowSnapshot | null {
    const currentConnection = connectionRef.current;
    if (!currentConnection) return null;
    return createWorkflowSnapshot({
      sessionId: currentConnection.sessionId,
      contextKey: contextRef.current.identity,
      epoch: contextRef.current.epoch,
      ...draftRef.current,
    });
  }
  function current(expected: WorkflowSnapshot): boolean {
    return (
      mountedRef.current && isCurrentWorkflowSnapshot(expected, snapshot())
    );
  }
  function currentContext(expected: WorkflowSnapshot): boolean {
    return mountedRef.current && sameWorkflowContext(expected, snapshot());
  }
  function readBusy(): boolean {
    return actionRef.current !== null;
  }
  function setActionNow(value: WorkflowEditorAction | null) {
    actionRef.current = value;
    if (mountedRef.current) setAction(value);
  }
  function editText(value: string) {
    draftRef.current = { text: value, revision: draftRef.current.revision + 1 };
    validatedRef.current = null;
    setText(value);
    setValidated(null);
    setNotice("");
    setError("");
  }
  function applyText(value: string, label = "载入的 Workflow（未校验）") {
    if (readBusy()) return false;
    sourceRef.current = null;
    setSource(null);
    setPath("");
    editText(value);
    setFileLabel(label);
    return true;
  }
  function begin(value: WorkflowEditorAction): WorkflowSnapshot | null {
    if (readBusy() || blockedRef.current()) return null;
    const captured = snapshot();
    if (!captured) {
      setError("请先打开工作区。");
      return null;
    }
    setActionNow(value);
    setError("");
    setNotice("");
    return captured;
  }
  function fail(reason: unknown, expected: WorkflowSnapshot) {
    if (current(expected)) setError(formatError(reason));
  }
  function bindLoaded(loaded: { space: WorkflowSpace; path: string; text: string; revision: string }) {
    editText(loaded.text);
    sourceRef.current = { context: fileContext, space: loaded.space, path: loaded.path, revision: loaded.revision, content: loaded.text };
    setSource(sourceRef.current);
    setFileLabel(`${loaded.space}: ${loaded.path}`);
    setSourceSpace(loaded.space);
    setPath(loaded.path);
  }

  async function load(): Promise<boolean> {
    const captured = begin("loading");
    if (!captured) return false;
    try {
      const loaded = await invokeDesktop<{
        space: WorkflowSpace;
        path: string;
        text: string;
        revision: string;
      }>("workflow_editor_load", {
        sessionId: captured.sessionId,
        space: sourceSpace,
        path: workflowRelativePath(path),
      });
      if (!current(captured)) return false;
      if (
        !(await confirmDesktop(
          "打开文件会替换当前 Workflow 草稿。继续？",
          "打开 Workflow",
        ))
      )
        return false;
      if (!current(captured)) return false;
      bindLoaded(loaded);
      setNotice("文件已载入，待校验。");
      return true;
    } catch (reason) {
      fail(reason, captured);
      return false;
    } finally {
      setActionNow(null);
    }
  }

  async function loadFromFile(space: WorkflowSpace, path: string, preview: string, stillCurrent: () => boolean): Promise<boolean> {
    const captured = begin("loading");
    if (!captured) return false;
    try {
      const loaded = await invokeDesktop<{ space: WorkflowSpace; path: string; text: string; revision: string }>("workflow_editor_load", {
        sessionId: captured.sessionId, space, path: workflowRelativePath(path),
      });
      if (loaded.text !== preview) throw new Error("文件已变化，请刷新预览后重新打开。");
      if (!current(captured) || !stillCurrent()) return false;
      bindLoaded(loaded);
      setNotice("文件已载入，待校验。");
      return true;
    } catch (reason) { fail(reason, captured); return false; }
    finally { setActionNow(null); }
  }

  async function save(saveAs = false): Promise<boolean> {
    const captured = begin("saving");
    if (!captured) return false;
    const origin = saveAs ? null : sourceForContext(sourceRef.current, fileContext);
    try {
      const targetSpace = origin?.space ?? sourceSpace;
      let targetPath = origin?.path ?? path;
      if (!origin && ((!targetPath.trim()) || (saveAs && targetSpace === "user"))) {
        if (targetSpace === "ai") throw new Error("请在相对文件路径中填写 AI 工作区的新文件路径，再保存或另存为。");
        const workspace = connectionRef.current!.workspace;
        const selected = await chooseNewWorkflowFile(workspace);
        if (!selected) return false;
        targetPath = workspaceRelativeSavePath(workspace, selected);
      }
      if (!currentContext(captured)) return false;
      const saved = await invokeDesktop<{ space: WorkflowSpace; path: string; revision: string }>(
        "workflow_editor_save",
        {
          sessionId: captured.sessionId,
          space: targetSpace,
          path: workflowRelativePath(targetPath),
          text: captured.text,
          expectedRevision: origin?.revision,
        },
      );
      if (currentContext(captured)) {
        const accepted = savedEditorSource(captured.contextKey, contextRef.current.identity, saved.path, saved.revision, captured.text, saved.space);
        sourceRef.current = accepted && { ...accepted, context: fileContext };
        setSource(sourceRef.current);
        setFileLabel(`${saved.space}: ${saved.path}`);
        setSourceSpace(saved.space);
        setPath(saved.path);
        setNotice(origin ? "已保存。" : "已另存为新文件。");
      }
      return true;
    } catch (reason) {
      if (currentContext(captured)) setError(`${formatError(reason)} 草稿已保留；文件冲突时请重新打开或另存为新路径。`);
      return false;
    } finally {
      setActionNow(null);
    }
  }

  async function validate(): Promise<boolean> {
    const captured = begin("validating");
    if (!captured) return false;
    validatedRef.current = null;
    setValidated(null);
    try {
      const reply = await invokeDesktop<{
        valid: boolean;
        error?: unknown;
        errors?: unknown;
      }>("workflow_editor_validate", {
        sessionId: captured.sessionId,
        text: captured.text,
      });
      if (!current(captured)) return false;
      if (reply.valid !== true)
        throw new Error(
          formatError(reply.error ?? reply.errors ?? "Workflow 校验未通过。"),
        );
      validatedRef.current = captured;
      setValidated(captured);
      setNotice("当前 Workflow 已通过校验。");
      return true;
    } catch (reason) {
      fail(reason, captured);
      return false;
    } finally {
      setActionNow(null);
    }
  }

  function finishRun(active: ActiveRun) {
    // Cancellation may resolve before the actual execution and cleanup finish.
    if (runRef.current !== active || !active.settled || active.stopPending)
      return;
    runRef.current = null;
    setActionNow(null);
  }
  async function run(): Promise<boolean> {
    const candidate = snapshot();
    if (
      !canRunWorkflow(
        candidate,
        validatedRef.current,
        readBusy(),
        blockedRef.current(),
      )
    ) {
      if (!readBusy() && !blockedRef.current())
        setError("请先校验当前 Workflow。");
      return false;
    }
    const captured = begin("running");
    if (!captured) return false;
    const active: ActiveRun = {
      requestId: crypto.randomUUID(),
      snapshot: captured,
      settled: false,
      stopPending: false,
      stopRequested: false,
    };
    runRef.current = active;
    try {
      const report = await invokeDesktop<{
        state: string;
        run_id: string;
        error?: unknown;
        [key: string]: unknown;
      }>("workflow_editor_run", {
        sessionId: captured.sessionId,
        requestId: active.requestId,
        text: captured.text,
      });
      if (currentContext(captured)) {
        setLastRunId(report.run_id ?? null);
        setLastState(report.state ?? "unknown");
        // A completed run never grants validation to an edited next draft.
        setNotice(
          `Workflow 运行${workflowRunStateLabels[report.state] ?? "已结束"}。`,
        );
        if (report.state === "failed" && report.error)
          setError(formatError(report.error));
      }
      return workflowRunSucceeded(report.state);
    } catch (reason) {
      if (currentContext(captured)) setError(formatError(reason));
      return false;
    } finally {
      active.settled = true;
      finishRun(active);
    }
  }

  async function requestStop(active: ActiveRun): Promise<boolean> {
    if (runRef.current !== active || active.settled || active.stopRequested)
      return false;
    active.stopPending = true;
    active.stopRequested = true;
    setActionNow("stopping");
    try {
      await invokeDesktop("agent_cancel", { requestId: active.requestId });
      if (currentContext(active.snapshot) && !active.settled)
        setNotice("正在停止并清理运行…");
      return true;
    } catch (reason) {
      active.stopRequested = false;
      if (currentContext(active.snapshot)) setError(formatError(reason));
      return false;
    } finally {
      active.stopPending = false;
      if (!active.settled && !active.stopRequested) setActionNow("running");
      finishRun(active);
    }
  }

  async function stop(): Promise<boolean> {
    const active = runRef.current;
    return active ? requestStop(active) : false;
  }

  async function resetBlank(): Promise<boolean> {
    if (readBusy()) return false;
    const expected = draftRef.current;
    const expectedContext = contextRef.current;
    if (
      expected.text !== blankWorkflowText &&
      !(await confirmDesktop(
        "恢复空白会替换当前 Workflow 草稿。继续？",
        "恢复空白 Workflow",
      ))
    )
      return false;
    if (
      readBusy() ||
      expected !== draftRef.current ||
      expectedContext !== contextRef.current
    )
      return false;
    applyText(blankWorkflowText, "未保存的 Workflow");
    return true;
  }

  return {
    exportSessionState: (): SessionState["workflow"] => ({ text, sourceSpace, path, source: sourceRef.current && workspaceRef.current.workspace ? { path: sourceRef.current.path, revision: sourceRef.current.revision, content: sourceRef.current.content, workspace: workspaceRef.current.workspace, space: sourceRef.current.space ?? "user" } : null }),
    restoreSessionState(value: SessionState["workflow"], workspace: string | null) {
      contextRef.current.epoch++;
      const owner = transitionEditorWorkspace({ workspace: null, scope: contextKey }, null, workspace, contextKey).owner;
      workspaceRef.current = owner;
      previousFileContext.current = editorWorkspaceContext(owner);
      editText(value.text);
      sourceRef.current = value.source ? { path: value.source.path, revision: value.source.revision, content: value.source.content, space: value.source.space, context: editorWorkspaceContext(transitionEditorWorkspace({ workspace: null, scope: contextKey }, null, value.source.workspace, contextKey).owner) } : null;
      setSource(sourceRef.current); setSourceSpace(value.sourceSpace); setPath(value.path); setFileLabel(value.source ? `${value.source.space}: ${value.source.path}` : "恢复的 Workflow 草稿");
      validatedRef.current = null; setValidated(null);
    },
    text,
    editText,
    applyText,
    fileLabel,
    modified: editorModified(sourceForContext(sourceRef.current, fileContext), text),
    sourceSpace,
    setSourceSpace,
    path,
    setPath,
    action,
    busy: action !== null,
    readBusy,
    error,
    notice,
    lastRunId,
    lastState,
    captureSnapshot: snapshot,
    isCurrentSnapshot: (expected: WorkflowSnapshot | null) =>
      !!expected && current(expected),
    validationCurrent:
      !!validated && isCurrentWorkflowSnapshot(validated, snapshot()),
    load,
    loadFromFile,
    save,
    saveAs: () => save(true),
    validate,
    run,
    stop,
    resetBlank,
  };
}

export type WorkflowEditor = ReturnType<typeof useWorkflowEditor>;
