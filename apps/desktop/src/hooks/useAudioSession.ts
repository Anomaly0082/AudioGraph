import { useEffect, useRef, useState } from "react";
import {
  chooseWorkspaceDirectory,
  confirmDesktop,
  connectDesktop,
  controlRequest,
  desktopAvailable,
  disconnectDesktop,
  listenBackendDisconnected,
  requireSuccess,
} from "../api/desktop";
import {
  canAdvanceTaskState,
  canCancel,
  formatError,
  isCurrentTaskResponse,
  isTerminal,
} from "../model";
import type {
  AudioInspection,
  Connection,
  DeviceCatalog,
  GraphSubmission,
  TaskView,
} from "../types/desktop";
import { cloneSubmission } from "../workflow-model";

type TaskResponse = {
  task_id: string;
  state: TaskView["state"];
  errors?: unknown[];
  result?: unknown;
};
type StartOptions = {
  validationCurrent?: boolean;
  ownedFailedTaskId?: string | null;
};
const emptyDevices = (): DeviceCatalog => ({ inputs: [], outputs: [] });

export function useAudioSession() {
  const desktop = desktopAvailable();
  const [workspace, setWorkspace] = useState("");
  const [allowDevices, setAllowDevicesState] = useState(false);
  const [allowMonitor, setAllowMonitorState] = useState(false);
  const [connection, setConnectionState] = useState<Connection | null>(null);
  const connectionRef = useRef<Connection | null>(null);
  const epochRef = useRef(0);
  const deadSessions = useRef(new Map<string, boolean>());
  const ownedSessions = useRef(new Set<string>());
  const [task, setTaskState] = useState<TaskView | null>(null);
  const taskRef = useRef<TaskView | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const busyRef = useRef<string | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [forcedWarning, setForcedWarning] = useState("");
  const [devices, setDevices] = useState<DeviceCatalog>(emptyDevices);
  const taskActive =
    task !== null && !isTerminal(task.state) && task.state !== "unknown";

  function setAllowDevices(value: boolean) {
    setAllowDevicesState(value);
    if (!value) setAllowMonitorState(false);
  }
  function setAllowMonitor(value: boolean) {
    setAllowMonitorState(value && allowDevices);
  }
  function setConnected(value: Connection | null) {
    connectionRef.current = value;
    setConnectionState(value);
  }
  function setTask(value: TaskView | null) {
    const previous = taskRef.current;
    if (
      previous &&
      value &&
      previous.id === value.id &&
      previous.sessionId === value.sessionId &&
      !canAdvanceTaskState(previous.state, value.state)
    )
      return;
    const next =
      previous &&
      value &&
      previous.id === value.id &&
      previous.sessionId === value.sessionId &&
      isTerminal(previous.state) &&
      previous.state === value.state
        ? {
            ...value,
            submission: previous.submission,
            result: value.result === undefined ? previous.result : value.result,
            errors: value.errors === undefined ? previous.errors : value.errors,
          }
        : value;
    taskRef.current = next;
    setTaskState(next);
  }
  function disconnectState(sessionId: string, message: string) {
    if (connectionRef.current?.sessionId !== sessionId) return;
    epochRef.current++;
    setConnected(null);
    setDevices(emptyDevices());
    const previous = taskRef.current;
    if (previous && !isTerminal(previous.state))
      setTask({ ...previous, state: "unknown" });
    if (busyRef.current === "断开中") setNotice(message);
    else
      setError(
        `${message}\n连接已失效。任务可能已经开始，结果未知；不会自动重试提交。请检查输出后重新连接。`,
      );
  }
  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listenBackendDisconnected((event) => {
      deadSessions.current.set(event.sessionId, !!event.forced);
      if (deadSessions.current.size > 32)
        deadSessions.current.delete(deadSessions.current.keys().next().value!);
      if (event.forced && ownedSessions.current.has(event.sessionId)) {
        setForcedWarning(
          `旧后端会话已被强制结束，未完成输出文件可能保留。${event.message} 当前新会话不会自动重试旧任务。`,
        );
      }
      disconnectState(event.sessionId, event.message);
    })
      .then((remove) => {
        if (disposed) remove();
        else unlisten = remove;
      })
      .catch((reason) => {
        if (!disposed) setError(formatError(reason));
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [desktop]);

  async function guardedRpc<T>(
    request: Record<string, unknown>,
    target: Connection,
  ): Promise<T> {
    let reply;
    try {
      reply = await controlRequest<T>(target.sessionId, request);
    } catch (reason) {
      disconnectState(target.sessionId, formatError(reason));
      throw new Error(
        `后端连接不可用：${formatError(reason)}。未自动重试，请检查任务和输出后重新连接。`,
      );
    }
    if (connectionRef.current?.sessionId === target.sessionId && reply.record_warnings?.length)
      setForcedWarning(reply.record_warnings.join("\n"));
    return requireSuccess(reply);
  }
  async function perform<T>(
    label: string,
    action: () => Promise<T>,
  ): Promise<T> {
    if (busyRef.current) throw new Error(`请等待${busyRef.current}完成。`);
    busyRef.current = label;
    setBusy(label);
    setError("");
    setNotice("");
    try {
      return await action();
    } catch (reason) {
      setError(formatError(reason));
      throw reason;
    } finally {
      busyRef.current = null;
      setBusy(null);
    }
  }
  function ensureCurrent(target: Connection, epoch: number) {
    if (
      epoch !== epochRef.current ||
      connectionRef.current?.sessionId !== target.sessionId
    ) {
      throw new Error("连接已变化，迟到的响应已忽略。请检查任务与输出状态。");
    }
  }
  async function fetchTaskResult(
    id: string,
    target: Connection,
    epoch: number,
  ) {
    const data = await guardedRpc<TaskResponse>(
      { op: "tasks.result", task_id: id },
      target,
    );
    const previous = taskRef.current;
    if (
      !previous ||
      !isCurrentTaskResponse(epoch, epochRef.current, id, previous.id) ||
      previous.sessionId !== target.sessionId
    )
      return;
    setTask({
      ...previous,
      state: data.state,
      errors: data.errors,
      result: data.result,
    });
  }
  useEffect(() => {
    if (
      !connection ||
      !taskActive ||
      !task ||
      task.sessionId !== connection.sessionId
    )
      return;
    const target = connection,
      id = task.id,
      epoch = epochRef.current;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      try {
        const data = await guardedRpc<TaskResponse>(
          { op: "tasks.status", task_id: id },
          target,
        );
        const current = taskRef.current;
        if (
          disposed ||
          !current ||
          !isCurrentTaskResponse(epoch, epochRef.current, id, current.id) ||
          current.sessionId !== target.sessionId
        )
          return;
        if (isTerminal(data.state)) {
          await fetchTaskResult(id, target, epoch);
          return;
        }
        if (!["queued", "running", "cancelling"].includes(data.state))
          throw new Error("后端返回了未知任务状态。");
        setTask({ ...current, state: data.state, errors: data.errors });
        if (!disposed) timer = setTimeout(tick, 500);
      } catch (reason) {
        const current = taskRef.current;
        if (
          disposed ||
          !current ||
          !isCurrentTaskResponse(epoch, epochRef.current, id, current.id)
        )
          return;
        setError(formatError(reason));
        setTask({ ...current, state: "unknown" });
      }
    };
    timer = setTimeout(tick, 500);
    return () => {
      disposed = true;
      if (timer) clearTimeout(timer);
    };
  }, [connection?.sessionId, task?.id, task?.state, taskActive]);

  async function chooseWorkspace(): Promise<string | null> {
    if (connectionRef.current)
      throw new Error("请先断开当前工作区连接；活动任务断开前需要确认取消。");
    const selected = await chooseWorkspaceDirectory();
    if (selected) setWorkspace(selected);
    return selected;
  }
  async function connectBackend(): Promise<void> {
    await perform("连接中", async () => {
      if (connectionRef.current)
        throw new Error("请先断开当前工作区连接；活动任务断开前需要确认取消。");
      const epoch = ++epochRef.current;
      const value = await connectDesktop(
        workspace.trim(),
        allowDevices,
        allowMonitor,
      );
      if (
        epoch !== epochRef.current ||
        deadSessions.current.has(value.sessionId)
      ) {
        throw new Error("后端在连接期间已退出，请重新连接。");
      }
      ownedSessions.current.add(value.sessionId);
      if (ownedSessions.current.size > 32)
        ownedSessions.current.delete(
          ownedSessions.current.values().next().value!,
        );
      if (value.previousForcedDisconnect)
        setForcedWarning(
          "重新连接时，旧后端因未及时退出而被强制结束。请检查可能保留的部分输出文件；没有自动重试旧任务。",
        );
      setConnected(value);
      setWorkspace(value.workspace);
      setDevices(emptyDevices());
      setNotice("后端已连接。设备没有被自动打开，请先校验当前 Graph。");
    });
  }
  async function disconnectBackend(): Promise<void> {
    await perform("断开中", async () => {
      const target = connectionRef.current;
      if (!target) return;
      if (
        taskRef.current &&
        !isTerminal(taskRef.current.state) &&
        taskRef.current.state !== "unknown" &&
        !(await confirmDesktop(
          "断开会请求取消当前任务。未完成文件可能保留。继续？",
          "断开后端",
        ))
      )
        return;
      if (connectionRef.current?.sessionId !== target.sessionId) return;
      const report = await disconnectDesktop(target.sessionId);
      if (report.forced)
        setForcedWarning(
          `旧后端会话已强制结束，可能保留部分输出文件。${report.message}`,
        );
      if (connectionRef.current?.sessionId === target.sessionId) {
        epochRef.current++;
        setConnected(null);
        setDevices(emptyDevices());
        const previous = taskRef.current;
        if (previous && !isTerminal(previous.state))
          setTask({ ...previous, state: "unknown" });
      }
      setNotice(
        `${report.forced ? "已强制结束后端；未完成文件可能保留。" : "已断开后端。"} ${report.message}`,
      );
    });
  }
  async function validateSubmission(
    submission: GraphSubmission,
  ): Promise<boolean> {
    return perform("校验中", async () => {
      const target = connectionRef.current;
      if (!target) throw new Error("请先连接后端。");
      const epoch = epochRef.current;
      await guardedRpc(
        { op: "graph.validate", ...cloneSubmission(submission) },
        target,
      );
      if (epoch !== epochRef.current) return false;
      setNotice(
        "Graph 校验通过。校验不运行节点，也不验证设备在线或输入文件存在。",
      );
      return true;
    });
  }
  async function startSubmission(
    submission: GraphSubmission,
    options: StartOptions = {},
  ): Promise<string | null> {
    // Capture before any await, including the audible-output confirmation.
    const snapshot = cloneSubmission(submission);
    return perform("提交中", async () => {
      const target = connectionRef.current;
      if (!target) throw new Error("请先连接后端。");
      if (options.validationCurrent === false)
        throw new Error("请先校验当前 Graph。");
      const epoch = epochRef.current;
      const existing = taskRef.current;
      if (
        existing &&
        !(
          options.ownedFailedTaskId &&
          existing.id === options.ownedFailedTaskId &&
          existing.sessionId === target.sessionId &&
          existing.state === "failed"
        )
      ) {
        throw new Error(
          "请先释放上一次任务记录；AI 只可释放本会话内自己持有的失败任务。",
        );
      }
      if (snapshot.mode === "realtime" && !target.allowDevices)
        throw new Error("当前会话未允许音频设备访问。");
      if (snapshot.mode === "realtime" && snapshot.options.probe === false) {
        if (!target.allowMonitor) throw new Error("当前会话未允许有声输出。");
        if (
          !(await confirmDesktop(
            "这次运行会把麦克风声音送到选定输出。请佩戴耳机、调低音量，避免扬声器啸叫。确认开始？",
            "确认有声输出",
          ))
        )
          return null;
      }
      ensureCurrent(target, epoch);
      if (taskRef.current !== existing)
        throw new Error("任务已变化，未提交新任务。");
      if (existing) {
        await guardedRpc({ op: "tasks.release", task_id: existing.id }, target);
        ensureCurrent(target, epoch);
        if (taskRef.current !== existing)
          throw new Error("任务已变化，未提交新任务。");
        setTask(null);
      }
      const data = await guardedRpc<TaskResponse>(
        { op: "tasks.start", ...snapshot },
        target,
      );
      ensureCurrent(target, epoch);
      if (!data.task_id) {
        disconnectState(target.sessionId, "后端提交响应缺少任务 ID。");
        throw new Error(
          "后端返回了无效任务 ID；任务可能已开始，请检查输出后重新连接。",
        );
      }
      if (
        ![
          "queued",
          "running",
          "cancelling",
          "succeeded",
          "failed",
          "cancelled",
        ].includes(data.state)
      ) {
        setTask({
          id: data.task_id,
          sessionId: target.sessionId,
          state: "unknown",
          submission: snapshot,
        });
        throw new Error("后端返回了未知任务状态；请检查任务与输出后手动处理。");
      }
      setTask({
        id: data.task_id,
        sessionId: target.sessionId,
        state: data.state,
        errors: data.errors,
        submission: snapshot,
      });
      if (isTerminal(data.state))
        await fetchTaskResult(data.task_id, target, epoch);
      return data.task_id;
    });
  }
  async function cancelTask(expectedTaskId?: string): Promise<void> {
    await perform("请求取消", async () => {
      const current = taskRef.current,
        target = connectionRef.current;
      if (
        !current ||
        !target ||
        current.sessionId !== target.sessionId ||
        (expectedTaskId && current.id !== expectedTaskId)
      )
        throw new Error("任务已变化，未发送取消请求。");
      if (!canCancel(current.state)) return;
      const epoch = epochRef.current;
      const data = await guardedRpc<TaskResponse>(
        { op: "tasks.cancel", task_id: current.id },
        target,
      );
      if (
        !isCurrentTaskResponse(
          epoch,
          epochRef.current,
          current.id,
          taskRef.current?.id ?? null,
        )
      )
        return;
      if (isTerminal(data.state))
        await fetchTaskResult(current.id, target, epoch);
      else setTask({ ...current, state: data.state, errors: data.errors });
    });
  }
  async function releaseTask(): Promise<void> {
    await perform("释放记录", async () => {
      const current = taskRef.current,
        target = connectionRef.current;
      if (!current) return;
      if (!isTerminal(current.state) && current.state !== "unknown")
        throw new Error("活动任务尚未结束，不能释放记录。");
      if (
        target &&
        current.sessionId === target.sessionId &&
        isTerminal(current.state)
      ) {
        const epoch = epochRef.current;
        await guardedRpc({ op: "tasks.release", task_id: current.id }, target);
        if (epoch !== epochRef.current || taskRef.current?.id !== current.id)
          return;
      }
      setTask(null);
      setNotice(
        "记录已清除，可以运行新任务。输出文件未删除；已有文件仍不会被覆盖。",
      );
    });
  }
  async function listDevices(): Promise<DeviceCatalog> {
    return perform("枚举设备", async () => {
      const target = connectionRef.current;
      if (!target?.allowDevices) throw new Error("请断开后显式允许设备访问。");
      const epoch = epochRef.current;
      const catalog = await guardedRpc<DeviceCatalog>(
        { op: "devices.list" },
        target,
      );
      ensureCurrent(target, epoch);
      setDevices({
        inputs: catalog.inputs ?? [],
        outputs: catalog.outputs ?? [],
      });
      setNotice(
        "设备已枚举，尚未打开。选择后点击“应用设备到 Graph”才会修改图。",
      );
      return catalog;
    });
  }
  async function inspectAudio(path: string): Promise<AudioInspection> {
    const target = connectionRef.current;
    if (!target) throw new Error("请先连接后端。");
    const epoch = epochRef.current;
    const inspection = await guardedRpc<AudioInspection>(
      { op: "audio.inspect", path },
      target,
    );
    ensureCurrent(target, epoch);
    return inspection;
  }
  return {
    desktop,
    workspace,
    setWorkspace,
    allowDevices,
    setAllowDevices,
    allowMonitor,
    setAllowMonitor,
    connection,
    task,
    taskActive,
    busy,
    error,
    setError,
    notice,
    setNotice,
    forcedWarning,
    setForcedWarning,
    devices,
    chooseWorkspace,
    connectBackend,
    disconnectBackend,
    validateSubmission,
    startSubmission,
    cancelTask,
    releaseTask,
    listDevices,
    inspectAudio,
    readTask: () => taskRef.current,
    readBusy: () => busyRef.current,
  };
}
export type AudioSession = ReturnType<typeof useAudioSession>;
