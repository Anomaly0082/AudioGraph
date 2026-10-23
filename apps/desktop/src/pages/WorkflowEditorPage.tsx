import { Workflow, FolderOpen, Save, SaveAll, RotateCcw, ShieldCheck, Play, Square, History } from "lucide-react";
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
        <h2><Workflow className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />Workflow</h2>
        <span className={`badge ${editor.validationCurrent ? "good" : ""}`}>
          {editor.validationCurrent ? "已通过校验" : "待校验"}
        </span>
      </div>
      <div className="editor-toolbar">
        <label>
          打开 / 另存为工作区
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
          打开 / 另存为路径
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
          <FolderOpen className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
          打开 JSON
        </button>
        <button
          disabled={unavailable}
          onClick={() => void editor.save()}
        >
          <Save className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
          保存
        </button>
        <button disabled={unavailable} onClick={() => void editor.saveAs()}>
          <SaveAll className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
          另存为
        </button>
        <button disabled={editor.busy} onClick={() => void editor.resetBlank()}>
          <RotateCcw className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
          恢复空白
        </button>
      </div>
      <p className="hint">保存更新原文件；另存为不覆盖已有文件，AI 工作区需填写新路径。内部 Graph 在 AI 工作区运行。</p>
      <div className="file-caption">{editor.fileLabel}{editor.modified ? " · 未保存修改" : ""}</div>
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
          <ShieldCheck className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
          校验
        </button>
        <button
          className="primary"
          disabled={unavailable || !editor.validationCurrent}
          onClick={() => void editor.run()}
        >
          <Play className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
          运行 Workflow
        </button>
        {(editor.action === "running" || editor.action === "stopping") && (
          <button
            disabled={editor.action === "stopping"}
            onClick={() => void editor.stop()}
          >
            <Square className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
            停止
          </button>
        )}
        {editor.lastRunId && (
          <button onClick={() => onOpenRun(editor.lastRunId!)}>
            <History className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
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
