import { useEffect, useRef, useState } from "react";
import { confirmDesktop, invokeDesktop } from "../api/desktop";
import { formatError } from "../model";
import type { Connection } from "../types/desktop";
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

  async function load(): Promise<boolean> {
    const captured = begin("loading");
    if (!captured) return false;
    try {
      const loaded = await invokeDesktop<{
        space: WorkflowSpace;
        path: string;
        text: string;
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
      editText(loaded.text);
      setFileLabel(`${loaded.space}: ${loaded.path}`);
      setNotice("文件已载入，待校验。");
      return true;
    } catch (reason) {
      fail(reason, captured);
      return false;
    } finally {
      setActionNow(null);
    }
  }

  async function save(): Promise<boolean> {
    const captured = begin("saving");
    if (!captured) return false;
    try {
      const saved = await invokeDesktop<{ space: WorkflowSpace; path: string }>(
        "workflow_editor_save",
        {
          sessionId: captured.sessionId,
          space: sourceSpace,
          path: workflowRelativePath(path),
          text: captured.text,
        },
      );
      if (current(captured)) {
        setFileLabel(`${saved.space}: ${saved.path}`);
        setNotice("已另存为新文件。");
      }
      return true;
    } catch (reason) {
      fail(reason, captured);
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
    text,
    editText,
    applyText,
    fileLabel,
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
    save,
    validate,
    run,
    stop,
    resetBlank,
  };
}

export type WorkflowEditor = ReturnType<typeof useWorkflowEditor>;
