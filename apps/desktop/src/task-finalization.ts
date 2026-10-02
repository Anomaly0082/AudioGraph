import { canAdvanceTaskState, formatError, isTerminal } from "./model";
import type { TaskView } from "./types/desktop";

export type TaskIdentity = {
  taskId: string;
  sessionId: string;
  epoch: number;
};
export type TaskResponse = {
  task_id: string;
  run_id?: string;
  state: TaskView["state"];
  errors?: unknown[];
  result?: unknown;
};

type FinalizationServices = {
  readCurrent: (identity: TaskIdentity) => TaskView | null;
  update: (identity: TaskIdentity, patch: Partial<TaskView>) => void;
  result: (identity: TaskIdentity) => Promise<TaskResponse>;
  release: (identity: TaskIdentity) => Promise<{ task_id: string; released: boolean }>;
};

export function mergeTaskView(previous: TaskView | null, value: TaskView | null): TaskView | null {
  if (!previous || !value || previous.id !== value.id || previous.sessionId !== value.sessionId)
    return value;
  if (!canAdvanceTaskState(previous.state, value.state)) return previous;
  if (!isTerminal(previous.state)) return value;
  return {
    ...value,
    runId: value.runId ?? previous.runId,
    submission: previous.submission,
    result: value.result === undefined ? previous.result : value.result,
    errors: value.errors === undefined ? previous.errors : value.errors,
    resultRead: !!(previous.resultRead || value.resultRead),
    released: !!(previous.released || value.released),
    cleanupBusy: previous.released ? false : value.cleanupBusy ?? previous.cleanupBusy,
    cleanupError: previous.released ? undefined : Object.hasOwn(value, "cleanupError") ? value.cleanupError : previous.cleanupError,
  };
}

export function taskBlocksSubmission(
  task: TaskView | null,
  sessionId: string | undefined,
): boolean {
  return !!task && task.sessionId === sessionId && (!isTerminal(task.state) || !task.released);
}

// Status/cancel can observe the same terminal state concurrently. Share the
// entire result-and-release operation, including failures, until it settles.
export function createTaskFinalizer(services: FinalizationServices) {
  const pending = new Map<string, Promise<void>>();
  function finalize(identity: TaskIdentity, retry = false): Promise<void> {
    const key = JSON.stringify([identity.sessionId, identity.epoch, identity.taskId]);
    const existing = pending.get(key);
    if (existing) return existing;
    const current = services.readCurrent(identity);
    if (!current || current.released) return Promise.resolve();
    if (!isTerminal(current.state))
      return Promise.reject(new Error("任务尚未确认结束，不能收尾。"));
    if (current.cleanupError && !retry)
      return Promise.reject(new Error(current.cleanupError));

    const operation = Promise.resolve().then(async () => {
      let task = services.readCurrent(identity);
      if (!task || task.released) return;
      services.update(identity, { cleanupBusy: true, cleanupError: undefined });
      try {
        if (!task.resultRead) {
          const data = await services.result(identity);
          task = services.readCurrent(identity);
          if (!task) return;
          // Failed/cancelled outcomes may legitimately have no result payload.
          // Successful outcomes must include it; a terminal status alone is not
          // sufficient evidence that the complete result was retrieved.
          if (
            data.task_id !== identity.taskId ||
            !isTerminal(data.state) ||
            data.state !== task.state ||
            (data.state === "succeeded" && data.result == null)
          )
            throw new Error("终态任务结果不完整或不匹配；未释放任务，请重试收尾。");
          services.update(identity, {
            state: data.state,
            errors: data.errors,
            result: data.result,
            resultRead: true,
          });
        }
        if (!services.readCurrent(identity)) return;
        const released = await services.release(identity);
        if (!services.readCurrent(identity)) return;
        if (released.task_id !== identity.taskId || released.released !== true)
          throw new Error("后端未确认任务已收尾，请重试。");
        services.update(identity, {
          released: true,
          cleanupBusy: false,
          cleanupError: undefined,
        });
      } catch (reason) {
        if (!services.readCurrent(identity)) return;
        services.update(identity, {
          cleanupBusy: false,
          cleanupError: `任务收尾失败：${formatError(reason)}`,
        });
        throw reason;
      }
    });
    pending.set(key, operation);
    void operation.then(
      () => pending.delete(key),
      () => pending.delete(key),
    );
    return operation;
  }
  return { finalize };
}
