import {
  buildCandidateSubmission,
  updateExperimentCandidate,
} from "./experiment-model";
import { formatError, isTerminal } from "./model";
import type { GraphSubmission, TaskView } from "./types/desktop";
import type { ExperimentRecord } from "./types/experiment";

export type ExperimentRunnerServices = {
  readRecord: () => ExperimentRecord;
  persist: (record: ExperimentRecord) => Promise<void>;
  readTask: () => TaskView | null;
  ensureBound: () => void;
  stopped: () => boolean;
  hasTask: () => boolean;
  validate: (submission: GraphSubmission) => Promise<boolean>;
  start: (submission: GraphSubmission) => Promise<string | null>;
  waitTerminal: (taskId: string) => Promise<TaskView>;
  cancel: (taskId: string) => Promise<void>;
  release: () => Promise<void>;
  ownTask: (taskId: string | null) => void;
};

export function readAuthoritativeTask(session: {
  readTask?: () => TaskView | null;
  task: TaskView | null;
}): TaskView | null {
  return session.readTask ? session.readTask() : session.task;
}

export function readAuthoritativeBusy(session: {
  readBusy?: () => string | null;
  busy: string | null;
}): string | null {
  return session.readBusy ? session.readBusy() : session.busy;
}

// This is the actual batch path used by the hook. All effects pass through the
// session and persistence functions supplied by the hook; tests inject the same
// contract to exercise ordering and failure behavior.
export async function runExperimentBatch(
  roundId: string,
  services: ExperimentRunnerServices,
): Promise<void> {
  const initial = services.readRecord();
  const round = initial.rounds.find((item) => item.id === roundId);
  if (!round) throw new Error("候选批次不存在。");
  if (round.candidates.some((item) => item.state !== "planned"))
    throw new Error("这个批次已执行或中断，不能再次运行。");
  let ownedTaskId: string | null = null;
  let activeCandidateId: string | null = null;
  let checkpointFailed = false;
  async function checkpoint(value: ExperimentRecord) {
    try {
      await services.persist(value);
    } catch (reason) {
      checkpointFailed = true;
      throw reason;
    }
  }
  async function markInterrupted(candidateId: string) {
    await checkpoint(
      updateExperimentCandidate(
        services.readRecord(),
        roundId,
        candidateId,
        (entry) => ({ ...entry, state: "interrupted" }),
      ),
    );
  }
  try {
    for (const candidate of round.candidates) {
      if (services.stopped()) break;
      activeCandidateId = candidate.id;
      services.ensureBound();
      if (services.hasTask())
        throw new Error("任务记录已被其他操作占用，停止后续候选。");
      const submission = buildCandidateSubmission(
        services.readRecord(),
        round,
        candidate,
      );
      await checkpoint(
        updateExperimentCandidate(
          services.readRecord(),
          roundId,
          candidate.id,
          (entry) => ({ ...entry, state: "starting" }),
        ),
      );
      if (services.stopped()) {
        await markInterrupted(candidate.id);
        break;
      }
      if (!(await services.validate(submission)))
        throw new Error("候选 Graph 校验未完成，停止后续候选。");
      if (services.stopped()) {
        await markInterrupted(candidate.id);
        break;
      }
      const taskId = await services.start(submission);
      if (!taskId) throw new Error("候选任务没有启动。");
      ownedTaskId = taskId;
      services.ownTask(taskId);
      services.ensureBound();
      await checkpoint(
        updateExperimentCandidate(
          services.readRecord(),
          roundId,
          candidate.id,
          (entry) => ({ ...entry, state: "running", task_id: taskId }),
        ),
      );
      const task = await services.waitTerminal(taskId);
      if (
        task.id !== taskId ||
        !isTerminal(task.state) ||
        (task.state === "succeeded" && task.result === undefined)
      )
        throw new Error("任务终态或结果不完整，停止后续候选。");
      await checkpoint(
        updateExperimentCandidate(
          services.readRecord(),
          roundId,
          candidate.id,
          (entry) => ({
            ...entry,
            state: task.state as "succeeded" | "failed" | "cancelled",
            task_id: taskId,
            ...(task.result === undefined ? {} : { result: task.result }),
            ...(task.errors === undefined ? {} : { errors: task.errors }),
          }),
        ),
      );
      if (task.state === "failed")
        throw new Error("候选运行失败，已停止后续候选；失败任务保留在任务页。");
      if (task.state === "cancelled")
        throw new Error("候选已取消，后续候选未运行。");
      if (services.stopped()) break;
      await services.release();
      ownedTaskId = null;
      services.ownTask(null);
      activeCandidateId = null;
    }
  } catch (original) {
    let reason: unknown = original;
    if (ownedTaskId) {
      const task = services.readTask();
      if (
        task?.id === ownedTaskId &&
        !isTerminal(task.state) &&
        task.state !== "unknown"
      ) {
        try {
          await services.cancel(ownedTaskId);
        } catch (cancelError) {
          reason = new Error(
            `${formatError(reason)}；取消任务也失败：${formatError(cancelError)}`,
          );
        }
      }
    }
    if (!checkpointFailed && activeCandidateId) {
      try {
        services.ensureBound();
        const current = services
          .readRecord()
          .rounds.find((item) => item.id === roundId)
          ?.candidates.find((item) => item.id === activeCandidateId);
        if (
          current &&
          (current.state === "starting" || current.state === "running")
        ) {
          const task = services.readTask();
          const terminal = task?.id === ownedTaskId && isTerminal(task.state);
          const state =
            terminal && task.state === "succeeded" && task.result !== undefined
              ? "succeeded"
              : terminal &&
                  (task.state === "failed" || task.state === "cancelled")
                ? task.state
                : "interrupted";
          await services.persist(
            updateExperimentCandidate(
              services.readRecord(),
              roundId,
              activeCandidateId,
              (entry) => ({
                ...entry,
                state,
                ...(task?.id === ownedTaskId && task.result !== undefined
                  ? { result: task.result }
                  : {}),
                errors:
                  task?.id === ownedTaskId && task.errors !== undefined
                    ? task.errors
                    : [{ message: formatError(reason) }],
              }),
            ),
          );
        }
      } catch (saveError) {
        reason = new Error(
          `${formatError(reason)}；候选状态保存失败：${formatError(saveError)}`,
        );
      }
    }
    throw reason;
  }
}
