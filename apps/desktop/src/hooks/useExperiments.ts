import { useEffect, useRef, useState } from "react";
import { createAiRequestId, normalizeAiConfig } from "../ai-model";
import { confirmDesktop, invokeDesktop } from "../api/desktop";
import {
  appendExperimentRound,
  buildCandidateSubmission,
  normalizeExperimentRecord,
  updateExperimentCandidate,
  validateExperimentProposal,
  validateExperimentSpec,
} from "../experiment-model";
import {
  readAuthoritativeBusy,
  readAuthoritativeTask,
  runExperimentBatch,
} from "../experiment-runner";
import { formatError, isTerminal } from "../model";
import type { GraphSubmission, TaskView } from "../types/desktop";
import type {
  CandidateFeedback,
  ExperimentAiReply,
  ExperimentProposal,
  ExperimentRecord,
  ExperimentSpec,
  ExperimentSummary,
} from "../types/experiment";
import type { AiSettings } from "./useAiSettings";
import type { AudioSession } from "./useAudioSession";

type Options = {
  session: AudioSession;
  settings: AiSettings;
  blocked: () => boolean;
  onRestore: (submission: GraphSubmission) => Promise<boolean>;
};

export function useExperiments({
  session,
  settings,
  blocked,
  onRestore,
}: Options) {
  const [record, setRecordState] = useState<ExperimentRecord | null>(null);
  const recordRef = useRef<ExperimentRecord | null>(null);
  const [history, setHistory] = useState<ExperimentSummary[]>([]);
  const [busy, setBusy] = useState(false);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [pendingProposal, setPendingProposalState] =
    useState<ExperimentProposal | null>(null);
  const pendingRef = useRef<ExperimentProposal | null>(null);
  const pendingBindingRef = useRef<{
    sessionId: string;
    recordId: string;
  } | null>(null);
  const lockRef = useRef(false);
  const runRef = useRef(false);
  const stopRef = useRef(false);
  const ownedTaskRef = useRef<string | null>(null);
  const cancelIssuedRef = useRef<string | null>(null);
  const cancelRetryAtRef = useRef(0);
  const requestIdRef = useRef<string | null>(null);
  const mountedRef = useRef(true);
  const sessionRef = useRef(session);
  const blockedRef = useRef(blocked);
  const restoreRef = useRef(onRestore);
  sessionRef.current = session;
  blockedRef.current = blocked;
  restoreRef.current = onRestore;

  function putRecord(value: ExperimentRecord | null) {
    recordRef.current = value;
    if (mountedRef.current) setRecordState(value);
  }
  function putPending(
    value: ExperimentProposal | null,
    binding: { sessionId: string; recordId: string } | null,
  ) {
    pendingRef.current = value;
    pendingBindingRef.current = binding;
    if (mountedRef.current) setPendingProposalState(value);
  }
  function currentSessionId(): string {
    const id = sessionRef.current.connection?.sessionId;
    if (!id) throw new Error("请先连接后端。");
    return id;
  }
  function currentTask(): TaskView | null {
    return readAuthoritativeTask(sessionRef.current);
  }
  function currentBusy(): string | null {
    return readAuthoritativeBusy(sessionRef.current);
  }
  function ensureBound(sessionId: string, recordId?: string) {
    if (
      !mountedRef.current ||
      sessionRef.current.connection?.sessionId !== sessionId ||
      (recordId && recordRef.current?.id !== recordId)
    )
      throw new Error("会话或实验已变化，已停止本次操作。");
  }
  function claim(): boolean {
    if (lockRef.current || runRef.current) return false;
    lockRef.current = true;
    setBusy(true);
    setError("");
    setNotice("");
    return true;
  }
  function release() {
    lockRef.current = false;
    if (mountedRef.current) setBusy(false);
  }
  async function guarded(action: () => Promise<void>): Promise<void> {
    if (!claim()) return;
    try {
      if (blockedRef.current())
        throw new Error("当前操作尚未结束，请稍后再试。");
      await action();
    } catch (reason) {
      if (mountedRef.current) setError(formatError(reason));
    } finally {
      release();
    }
  }
  async function save(value: ExperimentRecord, sessionId: string) {
    const normalized = normalizeExperimentRecord(value, false);
    ensureBound(sessionId, value.id);
    await invokeDesktop<void>("experiment_save", {
      sessionId,
      record: normalized.record,
    });
    ensureBound(sessionId, value.id);
    putRecord(normalized.record);
  }
  async function create(spec: ExperimentSpec): Promise<void> {
    await guarded(async () => {
      const sessionId = currentSessionId();
      if (currentTask()) throw new Error("请先释放当前任务记录。");
      const safe = validateExperimentSpec(
        spec,
        sessionRef.current.connection?.capabilities.nodes,
      );
      if (!(await sessionRef.current.validateSubmission(safe.base)))
        throw new Error("Graph 校验未完成。");
      ensureBound(sessionId);
      const value = await invokeDesktop<unknown>("experiment_create", {
        sessionId,
        spec: safe,
      });
      ensureBound(sessionId);
      const normalized = normalizeExperimentRecord(value);
      if (normalized.interrupted)
        throw new Error("新实验记录包含运行中的候选。");
      putRecord(normalized.record);
      putPending(null, null);
      setNotice("实验已创建，输入音频已复制为固定样本。");
      await refreshInternal(sessionId);
    });
  }
  async function refreshInternal(sessionId: string): Promise<void> {
    const list = await invokeDesktop<ExperimentSummary[]>("experiment_list", {
      sessionId,
    });
    ensureBound(sessionId);
    if (
      !Array.isArray(list) ||
      list.some(
        (item) =>
          !item ||
          typeof item.id !== "string" ||
          typeof item.goal !== "string" ||
          typeof item.created_at !== "number",
      )
    )
      throw new Error("实验列表格式无效。");
    setHistory(list);
  }
  async function refresh(): Promise<void> {
    await guarded(async () => refreshInternal(currentSessionId()));
  }
  async function load(id: string): Promise<void> {
    await guarded(async () => {
      const sessionId = currentSessionId();
      if (!id) throw new Error("请选择实验。");
      const value = await invokeDesktop<unknown>("experiment_load", {
        sessionId,
        id,
      });
      ensureBound(sessionId);
      const normalized = normalizeExperimentRecord(value);
      if (normalized.record.id !== id)
        throw new Error("载入的实验 ID 与请求不符。");
      putRecord(normalized.record);
      putPending(null, null);
      if (normalized.interrupted) {
        await invokeDesktop<void>("experiment_save", {
          sessionId,
          record: normalized.record,
        });
        ensureBound(sessionId, id);
        setNotice("上次未结束的候选已标为中断；不会自动重试。");
      }
    });
  }
  async function propose(): Promise<void> {
    await guarded(async () => {
      stopRef.current = false;
      const sessionId = currentSessionId();
      const active = recordRef.current;
      if (!active) throw new Error("请先创建或载入实验。");
      if (currentTask()) throw new Error("请先释放当前任务记录。");
      if (settings.settingsLoading || settings.settingsBusy)
        throw new Error("请等待模型配置读取完成。");
      const config = normalizeAiConfig(settings.config);
      const requestId = createAiRequestId();
      requestIdRef.current = requestId;
      putPending(null, null);
      try {
        const reply = await invokeDesktop<ExperimentAiReply>(
          "ai_experiment_candidates",
          {
            sessionId,
            requestId,
            config,
            record: active,
          },
        );
        if (stopRef.current || requestIdRef.current !== requestId) return;
        ensureBound(sessionId, active.id);
        if (reply.request_id !== requestId)
          throw new Error("模型响应的请求 ID 不匹配。");
        if (!reply.proposal)
          throw new Error(reply.text || "模型没有返回候选。");
        const proposal = validateExperimentProposal(
          reply.proposal,
          active.parameters,
        );
        putPending(proposal, { sessionId, recordId: active.id });
        setNotice(reply.text || "候选已生成，请检查整批参数后保存。");
      } finally {
        if (requestIdRef.current === requestId) requestIdRef.current = null;
        stopRef.current = false;
      }
    });
  }
  async function append(proposal: ExperimentProposal): Promise<void> {
    await guarded(async () => {
      const sessionId = currentSessionId();
      const active = recordRef.current;
      if (!active) throw new Error("请先创建或载入实验。");
      if (currentTask()) throw new Error("请先释放当前任务记录。");
      const next = appendExperimentRound(active, proposal);
      await save(next, sessionId);
      putPending(null, null);
      setNotice("候选批次已保存，请检查输出路径并确认运行。");
    });
  }
  async function addManual(candidates: ExperimentProposal): Promise<void> {
    await append(candidates);
  }
  async function acceptProposal(): Promise<void> {
    const proposal = pendingRef.current;
    const binding = pendingBindingRef.current;
    if (
      !proposal ||
      !binding ||
      binding.sessionId !== sessionRef.current.connection?.sessionId ||
      binding.recordId !== recordRef.current?.id
    ) {
      setError("AI 候选已失效，请重新生成。");
      return;
    }
    await append(proposal);
  }
  function rejectProposal() {
    if (!lockRef.current) putPending(null, null);
  }

  async function waitTerminal(
    sessionId: string,
    taskId: string,
  ): Promise<TaskView> {
    let terminalWithoutResultSince: number | null = null;
    for (;;) {
      if (
        stopRef.current &&
        cancelIssuedRef.current !== taskId &&
        Date.now() >= cancelRetryAtRef.current
      ) {
        const current = currentTask();
        if (
          current?.id === taskId &&
          !isTerminal(current.state) &&
          current.state !== "unknown"
        ) {
          cancelIssuedRef.current = taskId;
          try {
            await sessionRef.current.cancelTask(taskId);
          } catch (reason) {
            cancelIssuedRef.current = null;
            cancelRetryAtRef.current = Date.now() + 1000;
            if (mountedRef.current)
              setError(`停止请求失败：${formatError(reason)}`);
          }
        }
      }
      ensureBound(sessionId);
      const task = currentTask();
      if (!task || task.id !== taskId || task.sessionId !== sessionId)
        throw new Error("任务记录已变化，实验停止等待；请检查输出。");
      if (task.state === "unknown")
        throw new Error("任务状态未知；请检查输出，不会自动重试。");
      if (isTerminal(task.state)) {
        if (task.state === "succeeded" && task.result === undefined) {
          terminalWithoutResultSince ??= Date.now();
          if (Date.now() - terminalWithoutResultSince > 10000)
            throw new Error(
              "任务已结束，但成功结果未返回；未将候选标记为成功。",
            );
          await new Promise<void>((resolve) => setTimeout(resolve, 200));
          continue;
        }
        return task;
      }
      await new Promise<void>((resolve) => setTimeout(resolve, 200));
    }
  }
  async function runRound(roundId: string): Promise<void> {
    if (!claim()) return;
    runRef.current = true;
    stopRef.current = false;
    cancelIssuedRef.current = null;
    setRunning(true);
    try {
      if (blockedRef.current())
        throw new Error("当前操作尚未结束，请稍后再试。");
      const sessionId = currentSessionId();
      const initial = recordRef.current;
      if (!initial) throw new Error("请先创建或载入实验。");
      const recordId = initial.id;
      const round = initial.rounds.find((item) => item.id === roundId);
      if (!round) throw new Error("候选批次不存在。");
      if (round.candidates.some((item) => item.state !== "planned"))
        throw new Error("这个批次已执行或中断，不能再次运行。");
      if (currentTask() || currentBusy())
        throw new Error("请先完成或释放当前任务记录。");
      const summary = round.candidates
        .map(
          (item) =>
            `${item.label}: ${item.values.join(", ")} → ${item.output_path}`,
        )
        .join("\n");
      const approved = await confirmDesktop(
        `将顺序运行 ${round.candidates.length} 个候选：\n${summary}\n\n确认开始？`,
        "运行实验批次",
      );
      if (!approved || stopRef.current) return;
      ensureBound(sessionId, recordId);
      await runExperimentBatch(roundId, {
        readRecord: () => recordRef.current!,
        persist: (value) => save(value, sessionId),
        readTask: currentTask,
        ensureBound: () => ensureBound(sessionId, recordId),
        stopped: () => stopRef.current,
        hasTask: () => !!currentTask() || !!currentBusy(),
        validate: (submission) =>
          sessionRef.current.validateSubmission(submission),
        start: (submission) => sessionRef.current.startSubmission(submission),
        waitTerminal: (taskId) => waitTerminal(sessionId, taskId),
        cancel: async (taskId) => {
          if (cancelIssuedRef.current === taskId) return;
          cancelIssuedRef.current = taskId;
          try {
            await sessionRef.current.cancelTask(taskId);
          } catch (reason) {
            cancelIssuedRef.current = null;
            throw reason;
          }
        },
        release: () => sessionRef.current.releaseTask(),
        ownTask: (taskId) => {
          ownedTaskRef.current = taskId;
        },
      });
      if (!stopRef.current) setNotice("这一批候选已执行完成。");
      else setNotice("实验已停止，未启动后续候选。");
    } catch (reason) {
      if (mountedRef.current) setError(formatError(reason));
    } finally {
      ownedTaskRef.current = null;
      cancelIssuedRef.current = null;
      runRef.current = false;
      stopRef.current = false;
      if (mountedRef.current) setRunning(false);
      release();
    }
  }
  async function stop(): Promise<void> {
    if (!runRef.current && !requestIdRef.current) return;
    stopRef.current = true;
    const requestId = requestIdRef.current;
    const taskId = ownedTaskRef.current;
    try {
      if (requestId) await invokeDesktop("ai_cancel_request", { requestId });
      const task = currentTask();
      if (
        taskId &&
        task?.id === taskId &&
        !isTerminal(task.state) &&
        task.state !== "unknown" &&
        cancelIssuedRef.current !== taskId
      ) {
        cancelIssuedRef.current = taskId;
        try {
          await sessionRef.current.cancelTask(taskId);
        } catch (reason) {
          cancelIssuedRef.current = null;
          cancelRetryAtRef.current = Date.now() + 1000;
          throw reason;
        }
      }
    } catch (reason) {
      if (mountedRef.current) setError(`停止请求失败：${formatError(reason)}`);
    }
  }
  async function rate(
    roundId: string,
    candidateId: string,
    feedback: CandidateFeedback,
  ): Promise<void> {
    await guarded(async () => {
      const active = recordRef.current;
      if (!active) throw new Error("请先载入实验。");
      if (
        !["preferred", "acceptable", "rejected"].includes(feedback.rating) ||
        typeof feedback.note !== "string" ||
        new TextEncoder().encode(feedback.note).length > 4096
      )
        throw new Error("评价格式无效。");
      const round = active.rounds.find((item) => item.id === roundId);
      const candidate = round?.candidates.find(
        (item) => item.id === candidateId,
      );
      if (!candidate || candidate.state !== "succeeded")
        throw new Error("只能评价成功候选。");
      const updated = updateExperimentCandidate(
        active,
        roundId,
        candidateId,
        (item) => ({ ...item, feedback: { ...feedback } }),
      );
      await save(updated, currentSessionId());
      setNotice("评价已保存。");
    });
  }
  async function restore(roundId: string, candidateId: string): Promise<void> {
    try {
      if (lockRef.current || blockedRef.current())
        throw new Error("实验操作正在进行，请稍后恢复参数。");
      const active = recordRef.current;
      const round = active?.rounds.find((item) => item.id === roundId);
      const candidate = round?.candidates.find(
        (item) => item.id === candidateId,
      );
      if (!active || !round || !candidate || candidate.state !== "succeeded")
        throw new Error("只能恢复成功候选的参数。");
      const submission = structuredClone(
        buildCandidateSubmission(active, round, candidate),
      );
      const output = submission.graph.nodes.find(
        (node) => node.id === active.output_node_id,
      )!;
      const freshPath = `experiment-selected-${active.id}-${createAiRequestId()}.wav`;
      output.parameters = { ...output.parameters, path: freshPath };
      if (await restoreRef.current(submission))
        setNotice("候选参数已恢复为编辑器草稿，输出路径已换新；尚未运行。");
    } catch (reason) {
      setError(formatError(reason));
    }
  }

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      stopRef.current = true;
      const requestId = requestIdRef.current;
      if (requestId)
        void invokeDesktop("ai_cancel_request", { requestId }).catch(
          () => undefined,
        );
      const taskId = ownedTaskRef.current;
      if (taskId)
        void sessionRef.current.cancelTask(taskId).catch(() => undefined);
    };
  }, []);
  useEffect(() => {
    stopRef.current = true;
    const requestId = requestIdRef.current;
    if (requestId)
      void invokeDesktop("ai_cancel_request", { requestId }).catch(
        () => undefined,
      );
    const taskId = ownedTaskRef.current;
    if (taskId)
      void sessionRef.current.cancelTask(taskId).catch(() => undefined);
    putPending(null, null);
    putRecord(null);
    setHistory([]);
    if (session.connection?.sessionId)
      void refreshInternal(session.connection.sessionId).catch((reason) =>
        setError(formatError(reason)),
      );
  }, [session.connection?.sessionId]);

  const canStop = running || requestIdRef.current !== null;
  return {
    record,
    history,
    busy,
    running,
    canStop,
    error,
    setError,
    notice,
    create,
    refresh,
    load,
    propose,
    stop,
    addManual,
    runRound,
    rate,
    restore,
    pendingProposal,
    acceptProposal,
    rejectProposal,
  };
}

export type Experiments = ReturnType<typeof useExperiments>;
