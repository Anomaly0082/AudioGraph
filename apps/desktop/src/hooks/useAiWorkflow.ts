import { useEffect, useRef, useState } from "react";
import {
  createAiRequestId,
  isCurrentAiResponse,
  normalizeAiConfig,
  normalizeAiProposal,
  runApprovedProposal,
  sameAiConfig,
  type AiConfig,
  type AiGenerateResponse,
  type AiProposal,
  type AiRepairContext,
  type AiSummaryResponse,
  type AiTaskSnapshot,
  type AudioInspection,
} from "../ai-model";
import { formatError, isTerminal } from "../model";
import type { AiInvoke, AiSettings } from "./useAiSettings";

export type AiPhase =
  | "idle"
  | "generating"
  | "stopping"
  | "proposal"
  | "executing"
  | "summarizing"
  | "done";
export type AiWorkflowOptions = {
  sessionId: string | null;
  disabled: boolean;
  hasTask: boolean;
  task: AiTaskSnapshot | null;
  invokeAi: AiInvoke;
  settings: AiSettings;
  onStartProposal: (
    proposal: AiProposal,
    ownedFailedTaskId: string | null,
  ) => Promise<string>;
  onCancelTask: (taskId: string) => Promise<void>;
};

export function useAiWorkflow({
  sessionId,
  disabled,
  hasTask,
  task,
  invokeAi,
  settings,
  onStartProposal,
  onCancelTask,
}: AiWorkflowOptions) {
  const { config, savedConfig, settingsLoading, settingsBusy, saveSettings } =
    settings;
  const [prompt, setPrompt] = useState("");
  const [inputPath, setInputPathState] = useState("");
  const [phase, setPhase] = useState<AiPhase>("idle");
  const [calls, setCalls] = useState(0);
  const [round, setRound] = useState(0);
  const [assistantText, setAssistantText] = useState("");
  const [summary, setSummary] = useState("");
  const [proposal, setProposal] = useState<AiProposal | null>(null);
  const [inspection, setInspection] = useState<AudioInspection | null>(null);
  const [error, setError] = useState("");
  const [aiTaskId, setAiTaskId] = useState<string | null>(null);
  const [repairContext, setRepairContext] = useState<AiRepairContext | null>(
    null,
  );
  const [repairBefore, setRepairBefore] = useState<unknown>(null);
  const [failureSnapshot, setFailureSnapshot] = useState<AiTaskSnapshot | null>(
    null,
  );
  const [ownedFailedTaskId, setOwnedFailedTaskId] = useState<string | null>(
    null,
  );
  const originRef = useRef<{ prompt: string; inputPath: string | null } | null>(
    null,
  );
  const repairPendingRef = useRef(false);
  const flowRevisionRef = useRef(0);
  const activeModelStageRef = useRef<"generating" | "summarizing">(
    "generating",
  );
  const requestIdRef = useRef<string | null>(null);
  const stoppingRequestIdRef = useRef<string | null>(null);
  const summaryStartedRef = useRef(false);
  const submitStartedRef = useRef(false);
  const mountedRef = useRef(true);
  const sessionIdRef = useRef(sessionId);
  const cancelTaskRef = useRef(onCancelTask);
  const hasTaskRef = useRef(hasTask);
  const taskRef = useRef(task);
  const disabledRef = useRef(disabled);
  cancelTaskRef.current = onCancelTask;
  hasTaskRef.current = hasTask;
  taskRef.current = task;
  disabledRef.current = disabled;
  const workflowBusy =
    phase === "generating" ||
    phase === "stopping" ||
    phase === "proposal" ||
    phase === "executing" ||
    phase === "summarizing";
  const busy = workflowBusy || settingsBusy;

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);
  useEffect(() => {
    const previous = sessionIdRef.current;
    sessionIdRef.current = sessionId;
    if (previous === sessionId) return;
    const requestId = requestIdRef.current;
    flowRevisionRef.current++;
    repairPendingRef.current = false;
    requestIdRef.current = null;
    stoppingRequestIdRef.current = null;
    submitStartedRef.current = false;
    summaryStartedRef.current = false;
    if (requestId)
      void invokeAi("ai_cancel_request", { requestId }).catch(() => undefined);
    originRef.current = null;
    setProposal(null);
    setInspection(null);
    setAiTaskId(null);
    setSummary("");
    setCalls(0);
    setRound(0);
    setPhase("idle");
    setRepairContext(null);
    setRepairBefore(null);
    setFailureSnapshot(null);
    setOwnedFailedTaskId(null);
    if (previous) setError("后端会话已变化，旧会话的 AI 请求或提案已失效。");
  }, [invokeAi, sessionId]);

  async function getReadyConfig(): Promise<AiConfig | null> {
    let safeConfig: AiConfig;
    try {
      safeConfig = normalizeAiConfig(config);
    } catch (reason) {
      setError(formatError(reason));
      return null;
    }
    if (
      !sameAiConfig(safeConfig, savedConfig) &&
      !(await saveSettings(safeConfig))
    ) {
      setError("本机模型配置保存失败。请到设置页查看原因后重试。");
      return null;
    }
    return safeConfig;
  }

  async function generate() {
    if (
      !sessionId ||
      disabled ||
      hasTask ||
      workflowBusy ||
      repairPendingRef.current ||
      requestIdRef.current ||
      settingsLoading ||
      settingsBusy ||
      !prompt.trim()
    )
      return;
    repairPendingRef.current = true;
    const revision = flowRevisionRef.current;
    const original = {
      prompt: prompt.trim(),
      inputPath: inputPath.trim() || null,
    };
    const safeConfig = await getReadyConfig();
    if (!safeConfig) {
      repairPendingRef.current = false;
      return;
    }
    if (
      !mountedRef.current ||
      sessionIdRef.current !== sessionId ||
      hasTaskRef.current ||
      disabledRef.current ||
      flowRevisionRef.current !== revision
    ) {
      repairPendingRef.current = false;
      return;
    }
    repairPendingRef.current = false;
    originRef.current = original;
    const requestId = createAiRequestId();
    requestIdRef.current = requestId;
    stoppingRequestIdRef.current = null;
    submitStartedRef.current = false;
    summaryStartedRef.current = false;
    activeModelStageRef.current = "generating";
    setCalls(1);
    setRound(1);
    setProposal(null);
    setInspection(null);
    setAssistantText("");
    setSummary("");
    setAiTaskId(null);
    setRepairContext(null);
    setRepairBefore(null);
    setFailureSnapshot(null);
    setOwnedFailedTaskId(null);
    setError("");
    setPhase("generating");
    try {
      const response = await invokeAi<AiGenerateResponse>("ai_generate", {
        sessionId,
        requestId,
        config: safeConfig,
        prompt: original.prompt,
        inputPath: original.inputPath,
      });
      if (stoppingRequestIdRef.current === requestId) {
        stoppingRequestIdRef.current = null;
        requestIdRef.current = null;
        setError("已停止 AI 请求。");
        setPhase("idle");
        return;
      }
      if (
        !mountedRef.current ||
        sessionIdRef.current !== sessionId ||
        !isCurrentAiResponse(response.requestId, requestIdRef.current)
      )
        return;
      setAssistantText(response.text || "模型没有返回说明。");
      setInspection(response.inspection ?? null);
      if (response.proposal === undefined) {
        setRepairContext(response.repairContext ?? null);
        setRepairBefore(response.repairContext?.proposal ?? null);
        requestIdRef.current = null;
        setPhase("done");
        return;
      }
      setProposal(normalizeAiProposal(response.proposal));
      requestIdRef.current = null;
      setPhase("proposal");
    } catch (reason) {
      if (!mountedRef.current || requestIdRef.current !== requestId) return;
      requestIdRef.current = null;
      setError(
        stoppingRequestIdRef.current === requestId
          ? "已停止 AI 请求。"
          : formatError(reason),
      );
      stoppingRequestIdRef.current = null;
      setPhase("idle");
    }
  }

  async function repair() {
    const context = repairContext,
      original = originRef.current;
    if (
      !sessionId ||
      !context ||
      !original ||
      workflowBusy ||
      repairPendingRef.current ||
      requestIdRef.current ||
      settingsLoading ||
      settingsBusy ||
      disabledRef.current
    )
      return;
    const currentTask = taskRef.current;
    if (
      currentTask &&
      (currentTask.id !== ownedFailedTaskId ||
        currentTask.sessionId !== sessionId ||
        currentTask.state !== "failed")
    )
      return;
    repairPendingRef.current = true;
    const revision = flowRevisionRef.current;
    const safeConfig = await getReadyConfig();
    if (!safeConfig) {
      repairPendingRef.current = false;
      return;
    }
    const latestTask = taskRef.current;
    if (
      !mountedRef.current ||
      sessionIdRef.current !== sessionId ||
      flowRevisionRef.current !== revision ||
      disabledRef.current ||
      (latestTask &&
        (latestTask.id !== ownedFailedTaskId ||
          latestTask.sessionId !== sessionId ||
          latestTask.state !== "failed"))
    ) {
      repairPendingRef.current = false;
      return;
    }
    repairPendingRef.current = false;
    const requestId = createAiRequestId();
    requestIdRef.current = requestId;
    stoppingRequestIdRef.current = null;
    activeModelStageRef.current = "generating";
    submitStartedRef.current = false;
    summaryStartedRef.current = false;
    setRound((value) => value + 1);
    setCalls(1);
    setProposal(null);
    setSummary("");
    setAiTaskId(null);
    setAssistantText("");
    setError("");
    setPhase("generating");
    try {
      const response = await invokeAi<AiGenerateResponse>("ai_repair", {
        sessionId,
        requestId,
        config: safeConfig,
        prompt: original.prompt,
        inputPath: original.inputPath,
        context,
      });
      if (stoppingRequestIdRef.current === requestId) {
        stoppingRequestIdRef.current = null;
        requestIdRef.current = null;
        setError("已停止 AI 修正请求。");
        setPhase("done");
        return;
      }
      if (
        !mountedRef.current ||
        sessionIdRef.current !== sessionId ||
        !isCurrentAiResponse(response.requestId, requestIdRef.current)
      )
        return;
      setAssistantText(response.text || "模型没有返回说明。");
      setInspection(response.inspection ?? null);
      setRepairBefore(context.proposal);
      if (response.proposal === undefined) {
        setRepairContext(response.repairContext ?? null);
        requestIdRef.current = null;
        setPhase("done");
        return;
      }
      setProposal(normalizeAiProposal(response.proposal));
      requestIdRef.current = null;
      setPhase("proposal");
    } catch (reason) {
      if (!mountedRef.current || requestIdRef.current !== requestId) return;
      requestIdRef.current = null;
      const stopped = stoppingRequestIdRef.current === requestId;
      stoppingRequestIdRef.current = null;
      setError(stopped ? "已停止 AI 修正请求。" : formatError(reason));
      setPhase("done");
    }
  }

  async function stopAiRequest() {
    const requestId = requestIdRef.current;
    if (
      !requestId ||
      (phase !== "generating" &&
        phase !== "summarizing" &&
        phase !== "stopping")
    )
      return;
    const expectedSessionId = sessionIdRef.current;
    setPhase("stopping");
    stoppingRequestIdRef.current = requestId;
    try {
      await invokeAi<{ cancelled: boolean }>("ai_cancel_request", {
        requestId,
      });
    } catch (reason) {
      if (
        mountedRef.current &&
        requestIdRef.current === requestId &&
        stoppingRequestIdRef.current === requestId &&
        sessionIdRef.current === expectedSessionId
      ) {
        stoppingRequestIdRef.current = null;
        setError(`停止请求失败：${formatError(reason)}`);
        setPhase(activeModelStageRef.current);
      }
    }
  }

  async function confirmAndRun() {
    if (!proposal || !sessionId || phase !== "proposal") return;
    if (hasTask && !ownedFailedTaskId) {
      setError("请先释放当前任务记录，再确认 AI 提案。");
      return;
    }
    const expectedSessionId = sessionId;
    setError("");
    setPhase("executing");
    try {
      const taskId = await runApprovedProposal(
        {
          approved: true,
          expectedSessionId,
          currentSessionId: sessionIdRef.current,
          startedRef: submitStartedRef,
        },
        proposal,
        () => undefined,
        (value) => onStartProposal(value, ownedFailedTaskId),
      );
      if (!taskId) return;
      if (sessionIdRef.current !== expectedSessionId) {
        submitStartedRef.current = false;
        setProposal(null);
        setError("后端会话已变化；旧会话的任务响应不会继续驱动 AI 流程。");
        setPhase("done");
        return;
      }
      setAiTaskId(taskId);
      setRepairContext(null);
    } catch (reason) {
      if (sessionIdRef.current !== expectedSessionId) {
        submitStartedRef.current = false;
        setProposal(null);
        setError("后端会话已变化，旧会话的 AI 提案已失效。");
        setPhase("done");
        return;
      }
      setError(formatError(reason));
      setPhase("proposal");
    }
  }

  async function cancelExecution() {
    if (!aiTaskId || phase !== "executing") return;
    try {
      await onCancelTask(aiTaskId);
    } catch (reason) {
      setError(formatError(reason));
    }
  }

  useEffect(() => {
    if (
      phase !== "executing" ||
      !sessionId ||
      !proposal ||
      !aiTaskId ||
      !task ||
      task.id !== aiTaskId ||
      task.sessionId !== sessionId ||
      !isTerminal(task.state) ||
      summaryStartedRef.current
    )
      return;
    summaryStartedRef.current = true;
    if (task.state === "failed") {
      const snapshot = { ...task, errors: task.errors ?? [] };
      setFailureSnapshot(snapshot);
      setOwnedFailedTaskId(task.id);
      setRepairContext({
        proposal,
        errors: task.errors?.length
          ? task.errors
          : [{ message: "任务执行失败，未返回错误详情。" }],
      });
      setRepairBefore(proposal);
    }
    if (task.state === "cancelled") {
      setError("真实任务已取消；未发送第二个模型请求。AI 解释不能改变此结果。");
      setPhase("done");
      return;
    }
    const requestId = createAiRequestId();
    requestIdRef.current = requestId;
    activeModelStageRef.current = "summarizing";
    setCalls(2);
    setPhase("summarizing");
    setError("");
    let safeConfig: AiConfig;
    try {
      safeConfig = normalizeAiConfig(config);
    } catch (reason) {
      requestIdRef.current = null;
      setError(`任务已结束，但无法请求 AI 解释：${formatError(reason)}`);
      setPhase("done");
      return;
    }
    invokeAi<AiSummaryResponse>("ai_summarize", {
      sessionId,
      requestId,
      config: safeConfig,
      prompt: originRef.current?.prompt ?? prompt.trim(),
      proposal,
      result: { state: task.state, result: task.result, errors: task.errors },
    })
      .then((response) => {
        if (stoppingRequestIdRef.current === requestId) {
          stoppingRequestIdRef.current = null;
          requestIdRef.current = null;
          setError(`真实任务状态为“${task.state}”；已停止 AI 解释请求。`);
          setPhase("done");
          return;
        }
        if (
          !mountedRef.current ||
          sessionIdRef.current !== sessionId ||
          !isCurrentAiResponse(response.requestId, requestIdRef.current)
        )
          return;
        requestIdRef.current = null;
        setSummary(response.text || "模型没有返回结果说明。");
        setPhase("done");
      })
      .catch((reason) => {
        if (!mountedRef.current || requestIdRef.current !== requestId) return;
        requestIdRef.current = null;
        const stopped = stoppingRequestIdRef.current === requestId;
        stoppingRequestIdRef.current = null;
        setError(
          stopped
            ? `真实任务状态为“${task.state}”；已停止 AI 解释请求。`
            : `真实任务状态为“${task.state}”，但 AI 解释失败：${formatError(reason)}`,
        );
        setPhase("done");
      });
  }, [aiTaskId, config, invokeAi, phase, prompt, proposal, sessionId, task]);

  useEffect(() => {
    if (
      phase !== "executing" ||
      !aiTaskId ||
      !task ||
      task.id !== aiTaskId ||
      task.state !== "unknown"
    )
      return;
    summaryStartedRef.current = true;
    setError(
      "真实任务状态未知，AI 流程已停止等待；不会请求结果解释。请先检查输出，再决定是否断开或清除显示记录。",
    );
    setPhase("done");
  }, [aiTaskId, phase, task]);

  useEffect(() => {
    if (phase !== "executing" || !aiTaskId) return;
    const timer = setTimeout(
      () => {
        setError(
          "AI 任务等待已达到 10 分钟上限，正在请求取消；真实状态仍以任务区为准。",
        );
        void cancelTaskRef
          .current(aiTaskId)
          .catch((reason) =>
            setError(`等待超时，取消请求失败：${formatError(reason)}`),
          );
      },
      10 * 60 * 1000,
    );
    return () => clearTimeout(timer);
  }, [aiTaskId, phase]);

  function rejectProposal() {
    if (phase !== "proposal") return;
    submitStartedRef.current = false;
    setProposal(null);
    setPhase("done");
  }

  function reset() {
    if (workflowBusy || settingsBusy) return;
    flowRevisionRef.current++;
    originRef.current = null;
    repairPendingRef.current = false;
    requestIdRef.current = null;
    summaryStartedRef.current = false;
    stoppingRequestIdRef.current = null;
    submitStartedRef.current = false;
    setPhase("idle");
    setCalls(0);
    setRound(0);
    setAssistantText("");
    setSummary("");
    setProposal(null);
    setInspection(null);
    setRepairContext(null);
    setRepairBefore(null);
    setFailureSnapshot(null);
    setOwnedFailedTaskId(null);
    setError("");
    setAiTaskId(null);
  }

  function editRequest(kind: "prompt" | "inputPath", value: string) {
    if (workflowBusy || settingsBusy) return;
    flowRevisionRef.current++;
    originRef.current = null;
    const requestId = requestIdRef.current;
    requestIdRef.current = null;
    if (requestId)
      void invokeAi("ai_cancel_request", { requestId }).catch(() => undefined);
    setRepairContext(null);
    setRepairBefore(null);
    setFailureSnapshot(null);
    setOwnedFailedTaskId(null);
    setProposal(null);
    setAiTaskId(null);
    setSummary("");
    setAssistantText("");
    setCalls(0);
    setRound(0);
    setError("");
    setPhase("idle");
    if (kind === "prompt") setPrompt(value);
    else setInputPathState(value);
  }

  const canRepair =
    !!repairContext &&
    !!originRef.current &&
    !!sessionId &&
    (phase === "done" || phase === "idle") &&
    !disabled &&
    !settingsLoading &&
    !settingsBusy &&
    (!task ||
      (!!ownedFailedTaskId &&
        task.id === ownedFailedTaskId &&
        task.sessionId === sessionId &&
        task.state === "failed"));
  const unavailable = !sessionId || disabled || hasTask;
  return {
    prompt,
    inputPath,
    phase,
    calls,
    round,
    assistantText,
    summary,
    proposal,
    inspection,
    error,
    aiTaskId,
    repairContext,
    repairBefore,
    failureSnapshot,
    ownedFailedTaskId,
    workflowBusy,
    busy,
    canRepair,
    unavailable,
    repairPending: repairPendingRef.current,
    sessionId,
    hasTask,
    task,
    disabled,
    editPrompt: (value: string) => editRequest("prompt", value),
    setInputPath: (value: string) => editRequest("inputPath", value),
    generate,
    repair,
    stopAiRequest,
    confirmAndRun,
    cancelExecution,
    rejectProposal,
    reset,
  };
}

export type AiWorkflow = ReturnType<typeof useAiWorkflow>;
