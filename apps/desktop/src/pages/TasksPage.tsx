import type { AudioSession } from "../hooks/useAudioSession";
import { canCancel, formatError, isTerminal } from "../model";
import { modeLabels, resultFields, stateLabels } from "../presentation";
import GraphOverview from "../components/GraphOverview";
import Disclosure from "../components/Disclosure";

type Props = {
  session: AudioSession;
  aiBusy: boolean;
  onCancel: () => void;
  onRelease: () => void;
  onWorkbench: () => void;
  onEditor: () => void;
  onCopy: (value: string) => void;
  aiExplanation?: string;
  aiExplaining?: boolean;
  aiError?: string;
  isAiTask?: boolean;
};

export default function TasksPage({
  session,
  aiBusy,
  onCancel,
  onRelease,
  onWorkbench,
  onEditor,
  onCopy,
  aiExplanation,
  aiExplaining,
  aiError,
  isAiTask,
}: Props) {
  const { task, connection, busy, taskActive } = session;
  if (!task)
    return (
      <section className="panel empty-task">
        <span className="eyebrow">任务与结果</span>
        <h2>还没有任务记录</h2>
        <p>
          从工作台准备处理方案，或在编辑器校验后提交。这个页面只展示真实任务，不生成模拟结果。
        </p>
        <div className="action-row">
          <button className="primary" onClick={onWorkbench}>
            准备处理方案
          </button>
          <button onClick={onEditor}>打开 Graph 编辑器</button>
        </div>
      </section>
    );
  const fields = resultFields(task.result);
  const current = task.sessionId === connection?.sessionId;
  return (
    <div className="page-stack">
      <section className="panel" aria-labelledby="task-title">
        <div className="section-heading">
          <div>
            <span className="step">任务</span>
            <h2 id="task-title">{stateLabels[task.state] ?? task.state}</h2>
          </div>
          <span
            className={`badge ${task.state === "succeeded" ? "good" : ["failed", "unknown"].includes(task.state) ? "bad" : ""}`}
          >
            {task.state}
          </span>
        </div>
        <div className="task-identity">
          <code>{task.id}</code>
          <span>{modeLabels[task.submission.mode]}</span>
          <span>{current ? "当前工作区会话" : "已断开的旧会话"}</span>
        </div>
        {taskActive && (
          <div className="activity-line">
            <span className="activity-pulse" />
            {task.state === "cancelling"
              ? "已请求取消，正在等待后端结束。"
              : "后端正在处理。你可以切换页面查看或编辑下一份草稿。"}
          </div>
        )}
        {task.state === "unknown" && (
          <div className="banner warning">
            状态未知不等于失败或成功。请先检查输出文件，再重新连接；程序不会自动重试。
          </div>
        )}
        {task.errors?.length ? (
          <div className="task-errors" role="alert">
            <strong>处理未完成</strong>
            <pre>{formatError(task.errors)}</pre>
          </div>
        ) : null}
        <div className="action-row">
          <button
            className="danger"
            disabled={!current || !!busy || !canCancel(task.state)}
            onClick={onCancel}
          >
            {task.state === "cancelling" ? "等待取消完成" : "取消任务"}
          </button>
          {task.state === "failed" && (
            <button onClick={isAiTask ? onWorkbench : onEditor}>
              {isAiTask ? "回工作台查看 / 修正" : "回编辑器修改草稿"}
            </button>
          )}
          <button disabled={!!busy || aiBusy || taskActive} onClick={onRelease}>
            {current && isTerminal(task.state)
              ? "释放记录，准备新任务"
              : "清除显示记录"}
          </button>
        </div>
        <p className="hint">
          释放当前任务不会删除输出或运行历史。取消或失败可能留下部分文件。
        </p>
      </section>
      <section className="panel">
        <div className="section-heading">
          <h2>输出与指标</h2>
          <span className="badge">以后端结果为准</span>
        </div>
        {fields.length ? (
          <div className="output-list">
            {fields.map((field) => (
              <div className="output-item" key={field.name}>
                <div>
                  <small>{field.file ? "输出文件" : field.name}</small>
                  <code>{field.value}</code>
                </div>
                {field.file && (
                  <button onClick={() => onCopy(field.value)}>复制路径</button>
                )}
              </div>
            ))}
          </div>
        ) : (
          <p className="empty-state">
            {taskActive
              ? "任务结束后显示输出。"
              : "没有可展示的文件或数值输出。"}
          </p>
        )}
        <Disclosure label="完整结果 JSON">
          <pre className="result-json">
            {JSON.stringify(task.result ?? null, null, 2)}
          </pre>
        </Disclosure>
        <p className="hint">
          输出文件保存在工作区。本版没有内置播放器，可复制路径后使用自己的播放器试听。
        </p>
      </section>
      {(aiExplanation || aiExplaining || aiError) && (
        <section className="panel">
          <div className="section-heading">
            <h2>AI 结果解释</h2>
            <span className="badge">辅助说明 · 不改变任务状态</span>
          </div>
          {aiExplanation && <p className="summary-text">{aiExplanation}</p>}
          {aiExplaining && (
            <p className="hint">实际结果已保留，正在请求辅助解释。</p>
          )}
          {aiError && <p className="inline-error">{aiError}</p>}
          <div className="action-row">
            <button onClick={onWorkbench}>回工作台查看 AI 流程</button>
          </div>
        </section>
      )}
      <section className="panel">
        <div className="section-heading">
          <h2>提交时的方案</h2>
          <span className="badge">固定快照 · 不随草稿变化</span>
        </div>
        <GraphOverview
          graph={task.submission.graph}
          nodes={connection?.capabilities.nodes}
        />
        <Disclosure label="任务 Graph 与执行选项">
          <pre className="result-json">
            {JSON.stringify(task.submission, null, 2)}
          </pre>
        </Disclosure>
      </section>
    </div>
  );
}
