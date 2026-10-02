import Disclosure from "../components/Disclosure";
import type { RunRecords } from "../hooks/useRunRecords";
import {
  fileStateLabels,
  formatRunDuration,
  formatRunTime,
  groupRuns,
  runStateLabels,
} from "../run-record-model";
import type { RunSummary } from "../types/run-record";

type Props = {
  records: RunRecords;
  connected: boolean;
  currentTask?: {
    runId?: string;
    state: string;
    busy: boolean;
    onStop: () => void;
  };
};
export default function RunRecordsPage({
  records: runs,
  connected,
  currentTask,
}: Props) {
  const grouped = groupRuns(runs.records);
  const record = runs.record;
  const isCurrent = (run: RunSummary) =>
    run.kind === "graph" &&
    run.origin === "manual" &&
    !!currentTask?.runId &&
    currentTask.runId === run.id;
  const shownState = (run: RunSummary) =>
    isCurrent(run) ? currentTask!.state : run.state;
  function entry(run: RunSummary) {
    const state = shownState(run);
    const canStop =
      isCurrent(run) && ["queued", "running", "cancelling"].includes(state);
    return (
      <div className="run-row-actions">
        <button
          type="button"
          className={`run-row ${runs.selectedId === run.id ? "selected" : ""}`}
          aria-pressed={runs.selectedId === run.id}
          onClick={() => runs.select(run.id)}
        >
          <span>
            <strong>{run.name}</strong>
            <small>
              {run.kind === "workflow" ? "Workflow" : "Graph"} ·{" "}
              {run.origin === "ai" ? "AI" : "手动"}
            </small>
          </span>
          <span>
            <span
              className={`badge ${state === "succeeded" ? "good" : ["failed", "limited", "interrupted", "unknown"].includes(state) ? "bad" : ""}`}
            >
              {runStateLabels[state] ?? state}
            </span>
            <small>{formatRunTime(run.started_at_ms)}</small>
          </span>
        </button>
        {canStop && (
          <button
            type="button"
            className="danger run-stop"
            aria-label={`停止 ${run.name}`}
            disabled={currentTask!.busy || state === "cancelling"}
            onClick={currentTask!.onStop}
          >
            {state === "cancelling" ? "正在停止…" : "停止"}
          </button>
        )}
      </div>
    );
  }
  return (
    <div className="page-stack">
      <section className="panel">
        <div className="section-heading">
          <h2>运行记录</h2>
          <div className="action-row">
            <button
              onClick={() => void runs.refresh()}
              disabled={!connected || runs.loading}
            >
              {runs.loading ? "刷新中…" : "刷新"}
            </button>
          </div>
        </div>
        {runs.error && (
          <div className="banner error" role="alert">
            {runs.error}
          </div>
        )}
        {runs.warnings.map((warning, i) => (
          <div className="banner warning" key={i}>
            {warning}
          </div>
        ))}
        {runs.truncated && (
          <p className="hint">当前仅显示部分记录；其余记录仍保留在本地。</p>
        )}
        {!connected ? (
          <p className="empty-state">打开工作区后查看该项目的记录。</p>
        ) : !runs.records.length ? (
          <p className="empty-state">
            {runs.loading ? "正在读取…" : "还没有运行记录。"}
          </p>
        ) : (
          <div className="run-layout">
            <div className="run-list" aria-label="运行列表">
              {grouped.roots.map((run) => (
                <div key={run.id}>
                  {entry(run)}
                  {!!grouped.children.get(run.id)?.length && (
                    <Disclosure
                      label={`子 Graph（${grouped.children.get(run.id)!.length}）`}
                    >
                      <div className="run-children">
                        {grouped.children.get(run.id)!.map((child) => (
                          <div key={child.id}>{entry(child)}</div>
                        ))}
                      </div>
                    </Disclosure>
                  )}
                </div>
              ))}
            </div>
            <section className="run-detail" aria-label="运行详情">
              {!record ? (
                <p className="empty-state">选择一条记录查看详情。</p>
              ) : (
                <>
                  <h3>{record.name}</h3>
                  <p>
                    {runStateLabels[shownState(record)] ?? shownState(record)} ·{" "}
                    {formatRunTime(record.started_at_ms)} · 耗时{" "}
                    {formatRunDuration(record.duration_ms)}
                  </p>
                  {record.parent_id && (
                    <p className="hint">
                      所属 Workflow：
                      <button onClick={() => runs.select(record.parent_id!)}>
                        查看父记录
                      </button>
                    </p>
                  )}
                  {record.error && (
                    <div className="banner error" role="alert">
                      {record.error}
                    </div>
                  )}
                  {record.recording_warning && (
                    <div className="banner warning">
                      {record.recording_warning}
                    </div>
                  )}
                  <h4>文件</h4>
                  {record.files.length ? (
                    <>
                      <button
                        disabled={
                          runs.checking ||
                          ["running", "queued", "cancelling"].includes(
                            record.state,
                          )
                        }
                        onClick={() => void runs.checkFiles()}
                      >
                        {runs.checking ? "检查中…" : "检查文件状态"}
                      </button>
                      <div className="run-files">
                        {record.files.map((file, index) => {
                          const check = runs.checks?.find(
                            (v) =>
                              v.space === file.space &&
                              v.path === file.path &&
                              v.role === file.role,
                          );
                          return (
                            <div
                              className="run-file"
                              key={`${file.space}:${file.path}:${index}`}
                            >
                              <small>
                                {file.role === "input"
                                  ? "输入"
                                  : file.role === "output"
                                    ? "输出"
                                    : "相关文件"}{" "}
                                ·{" "}
                                {file.space === "user"
                                  ? "用户工作区"
                                  : "AI 工作区"}
                              </small>
                              <code>{file.path}</code>
                              <small>
                                {file.size_bytes === null
                                  ? "大小未记录"
                                  : `${file.size_bytes.toLocaleString()} 字节`}{" "}
                                ·{" "}
                                {check
                                  ? (fileStateLabels[check.status] ??
                                    check.status)
                                  : "尚未检查当前文件"}
                              </small>
                              {(check?.message || file.message) && (
                                <small>{check?.message || file.message}</small>
                              )}
                            </div>
                          );
                        })}
                      </div>
                      <Disclosure label="文件指纹">
                        <pre className="result-json">
                          {JSON.stringify(record.files, null, 2)}
                        </pre>
                      </Disclosure>
                    </>
                  ) : (
                    <p className="hint">没有记录文件引用。</p>
                  )}
                  <Disclosure label="执行配置">
                    <pre className="result-json">
                      {JSON.stringify(record.configuration, null, 2)}
                    </pre>
                  </Disclosure>
                  <Disclosure label="执行详情">
                    <pre className="result-json">
                      {JSON.stringify(record.result, null, 2)}
                    </pre>
                  </Disclosure>
                </>
              )}
            </section>
          </div>
        )}
      </section>
    </div>
  );
}
