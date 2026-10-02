import type { WorkflowEditor } from "../hooks/useWorkflowEditor";
import {
  workflowRunStateLabels,
  type WorkflowSpace,
} from "../workflow-editor-model";

export type WorkflowEditorPageProps = {
  editor: WorkflowEditor;
  connected: boolean;
  locked?: boolean;
  onOpenRun: (runId: string) => void;
};

const actionLabels = {
  loading: "正在打开…",
  saving: "正在保存…",
  validating: "正在校验…",
  running: "正在运行…",
  stopping: "正在停止并清理…",
};

export default function WorkflowEditorPage({
  editor,
  connected,
  locked = false,
  onOpenRun,
}: WorkflowEditorPageProps) {
  const unavailable = !connected || locked || editor.busy;
  const fileEditing = editor.action === "loading" || editor.action === "saving";
  return (
    <section className="panel workflow-editor">
      <div className="section-heading">
        <h2>Workflow</h2>
        <span className={`badge ${editor.validationCurrent ? "good" : ""}`}>
          {editor.validationCurrent ? "已通过校验" : "待校验"}
        </span>
      </div>
      <div className="editor-toolbar">
        <label>
          文件工作区
          <select
            aria-label="Workflow 文件工作区"
            value={editor.sourceSpace}
            disabled={editor.busy}
            onChange={(event) =>
              editor.setSourceSpace(event.target.value as WorkflowSpace)
            }
          >
            <option value="user">用户工作区</option>
            <option value="ai">AI 工作区</option>
          </select>
        </label>
        <label>
          相对文件路径
          <input
            aria-label="Workflow 相对文件路径"
            placeholder="workflows/example.workflow.json"
            value={editor.path}
            disabled={editor.busy}
            onChange={(event) => editor.setPath(event.target.value)}
          />
        </label>
        <button
          disabled={unavailable || !editor.path.trim()}
          onClick={() => void editor.load()}
        >
          打开 JSON
        </button>
        <button
          disabled={unavailable || !editor.path.trim()}
          onClick={() => void editor.save()}
        >
          另存为新文件
        </button>
        <button disabled={editor.busy} onClick={() => void editor.resetBlank()}>
          恢复空白
        </button>
      </div>
      <p className="hint">内部 Graph 在 AI 工作区运行</p>
      <div className="file-caption">{editor.fileLabel}</div>
      <textarea
        aria-label="Workflow JSON"
        className="code-editor"
        spellCheck={false}
        value={editor.text}
        disabled={fileEditing}
        onChange={(event) => editor.editText(event.target.value)}
      />
      <div className="action-row">
        <button disabled={unavailable} onClick={() => void editor.validate()}>
          校验
        </button>
        <button
          className="primary"
          disabled={unavailable || !editor.validationCurrent}
          onClick={() => void editor.run()}
        >
          运行 Workflow
        </button>
        {(editor.action === "running" || editor.action === "stopping") && (
          <button
            disabled={editor.action === "stopping"}
            onClick={() => void editor.stop()}
          >
            停止
          </button>
        )}
        {editor.lastRunId && (
          <button onClick={() => onOpenRun(editor.lastRunId!)}>
            查看运行记录
          </button>
        )}
      </div>
      {editor.action && <p role="status">{actionLabels[editor.action]}</p>}
      {editor.lastState && (
        <p className="muted">
          上次运行：
          {workflowRunStateLabels[editor.lastState] ?? editor.lastState}
        </p>
      )}
      {editor.notice && <p role="status">{editor.notice}</p>}
      {editor.error && (
        <p className="inline-error" role="alert">
          {editor.error}
        </p>
      )}
    </section>
  );
}
