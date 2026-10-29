import { FileJson, FolderOpen, Save, SaveAll, ShieldCheck, Play, History, LayoutTemplate, Plus, Undo2, Redo2, Copy, ClipboardPaste } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { GraphDraft } from "../hooks/useGraphDraft";
import type { AudioSession } from "../hooks/useAudioSession";
import {
  formatError,
  type GraphDocument,
  type Mode,
  type TemplateKind,
} from "../model";
import { modeLabels, templateLabels } from "../presentation";
import {
  addGraphNode,
  rewireGraphPorts,
  isNodeAvailableInMode,
  removeGraphConnection,
  removeGraphExport,
  removeGraphNode,
  setGraphExport,
  setNodeParameter,
  unavailableGraphNodes,
} from "../graph-editor-model";
import {
  initialCanvasView,
  positionKey,
  resolvedPositions,
  nodePorts,
  HEADER_HEIGHT,
  PORT_HEIGHT,
} from "../graph-canvas-layout";
import GraphCanvas from "../components/GraphCanvas";
import NodeInspector from "../components/NodeInspector";
import Disclosure from "../components/Disclosure";

export type EditorSelection = {
  search: string;
};
type Props = {
  draft: GraphDraft;
  session: AudioSession;
  selection: EditorSelection;
  onSelection: (value: EditorSelection) => void;
  submittingLocked: boolean;
  fileBusy?: boolean;
  onValidate: () => void;
  onRun: () => void;
  onLoadTemplate: () => void;
  onLoad: () => void;
  onSave: () => void;
  onSaveAs: () => void;
  onDevices: () => void;
  onTasks: () => void;
};

export default function EditorPage({
  draft,
  session,
  selection,
  onSelection,
  submittingLocked,
  fileBusy,
  onValidate,
  onRun,
  onLoadTemplate,
  onLoad,
  onSave,
  onSaveAs,
  onDevices,
  onTasks,
}: Props) {
  const { connection, task, busy } = session;
  const nodes = connection?.capabilities.nodes ?? [];
  const graph = draft.localGraph.graph;
  const view = draft.canvasView ?? initialCanvasView();
  const [error, setError] = useState("");
  const [deleteId, setDeleteId] = useState<string | null>(null);
  const [multiple, setMultiple] = useState<string[]>([]);
  const jsonEdit = useRef(0);
  const selectedIds = (multiple.includes(view.selectedId) ? multiple : [view.selectedId])
    .filter(id => graph?.nodes.some(node => node.id === id));
  function selectNode(id: string, extend = false) {
    const ids = extend ? selectedIds.includes(id) ? selectedIds.filter(value => value !== id) : [...selectedIds, id] : id ? [id] : [];
    setMultiple(ids);
    draft.setCanvasView({ ...view, selectedId: ids.includes(id) ? id : ids.at(-1) ?? "" });
  }
  function copy() {
    if (!graph || fileBusy) return;
    draft.copyNodes(selectedIds, resolvedPositions(graph, nodes, view.positions));
  }
  function paste() {
    if (!graph || fileBusy) return;
    try { setMultiple(draft.pasteNodes(resolvedPositions(graph, nodes, view.positions))); setError(""); }
    catch (reason) { setError(formatError(reason)); }
  }
  useEffect(() => {
    setError("");
    setDeleteId(null);
  }, [draft.graphText]);
  const selected = graph?.nodes.find((node) => node.id === view.selectedId);
  const descriptor = nodes.find((item) => item.typeId === selected?.type);
  const available = nodes.filter((item) =>
    isNodeAvailableInMode(item, draft.mode),
  );
  const filtered = available.filter((item) =>
    (item.typeId + " " + item.displayName)
      .toLowerCase()
      .includes(selection.search.toLowerCase()),
  );
  const mismatch = graph?.nodes.filter((node) => {
    const info = nodes.find((item) => item.typeId === node.type);
    return info && !isNodeAvailableInMode(info, draft.mode);
  });
  const canSubmitGraph = !!graph?.nodes.length && !draft.localGraph.error;
  const missingNodes =
    connection && graph ? unavailableGraphNodes(graph, nodes) : [];
  function change(action: (value: GraphDocument) => GraphDocument) {
    if (!graph || fileBusy) return;
    try {
      const updated = action(graph);
      if (updated === graph) return;
      draft.editGraph(JSON.stringify(updated, null, 2));
      setError("");
    } catch (reason) {
      setError(formatError(reason));
    }
  }
  return (
    <div className="visual-editor" onKeyDown={(event) => {
      if (event.nativeEvent.isComposing || event.altKey || fileBusy ||
          (event.target as HTMLElement).closest("input, textarea, select, [contenteditable='true']")) return;
      if (!(event.ctrlKey || event.metaKey)) return;
      const key = event.key.toLowerCase();
      if (key === "z" || key === "y") {
        event.preventDefault();
        if (key === "y" || event.shiftKey) draft.redo(); else draft.undo();
      } else if (key === "c" && selectedIds.length) { event.preventDefault(); copy(); }
      else if (key === "v" && draft.canPaste) { event.preventDefault(); paste(); }
      else if (key === "a" && (event.target as HTMLElement).closest(".canvas-viewport") && graph) {
        event.preventDefault(); setMultiple(graph.nodes.map(node => node.id));
        draft.setCanvasView({ ...view, selectedId: graph.nodes[0]?.id ?? "" });
      }
    }}>
      <section className="panel">
        <div className="section-heading">
          <h2><FileJson className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />Graph</h2>
          <span className={`badge ${draft.validationCurrent ? "good" : ""}`}>
            {draft.validationCurrent ? "已通过校验" : "待校验"}
          </span>
        </div>
        <div className="editor-toolbar">
          <label>
            模板
            <select
              aria-label="Graph 模板"
              value={draft.template}
              onChange={(event) =>
                draft.setTemplate(event.target.value as TemplateKind)
              }
            >
              {Object.entries(templateLabels).map(([key, value]) => (
                <option key={key} value={key}>
                  {value}
                </option>
              ))}
            </select>
          </label>
          <button disabled={fileBusy} onClick={onLoadTemplate}>
            <LayoutTemplate className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
            载入模板
          </button>
          <button
            disabled={!session.desktop || !connection || !!busy || fileBusy}
            onClick={onLoad}
          >
            <FolderOpen className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
            打开 JSON
          </button>
          <button
            disabled={
              !session.desktop ||
              !connection ||
              !!busy ||
              fileBusy ||
              !canSubmitGraph
            }
            onClick={onSave}
          >
            <Save className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
            保存
          </button>
          <button
            disabled={!session.desktop || !connection || !!busy || fileBusy || !canSubmitGraph}
            onClick={onSaveAs}
          >
            <SaveAll className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
            另存为
          </button>
        </div>
        <div className="execution-bar">
          <label>
            执行模式
            <select
              value={draft.mode}
              disabled={fileBusy}
              onChange={(event) => draft.setMode(event.target.value as Mode)}
            >
              {Object.entries(modeLabels).map(([key, value]) => (
                <option key={key} value={key}>
                  {value}
                </option>
              ))}
            </select>
          </label>
          {draft.mode !== "offline" && (
            <label>
              每块帧数
              <input
                type="number"
                min={1}
                max={65536}
                value={draft.blockFrames}
                onChange={(event) => draft.setBlockFrames(event.target.value)}
              />
            </label>
          )}
          {draft.mode === "realtime" && (
            <>
              <label>
                持续秒数
                <input
                  type="number"
                  min={1}
                  max={3600}
                  value={draft.duration}
                  onChange={(event) => draft.setDuration(event.target.value)}
                />
              </label>
              <label className="checkbox-label">
                <input
                  type="checkbox"
                  checked={draft.probe}
                  disabled={!connection?.allowMonitor}
                  onChange={(event) => draft.setProbe(event.target.checked)}
                />
                静音探测
              </label>
            </>
          )}
          <div className="action-row">
            <button
              disabled={!connection || !!busy || fileBusy || !canSubmitGraph}
              onClick={onValidate}
            >
              <ShieldCheck className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
              校验
            </button>
            <button
              className="primary"
              disabled={
                !connection ||
                submittingLocked ||
                session.taskBlocked ||
                missingNodes.length > 0 ||
                !draft.validationCurrent ||
                !canSubmitGraph
              }
              onClick={onRun}
            >
              <Play className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
              提交任务
            </button>
            {task && <button onClick={onTasks}><History className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />查看运行记录</button>}
          </div>
        </div>
        {draft.mode === "realtime" && (
          <p className="hint">有声输出需授权并再次确认，建议戴耳机。</p>
        )}
        {!!mismatch?.length && (
          <p className="inline-error" role="alert">
            以下节点不支持当前执行模式：
            {mismatch.map((node) => node.id).join("、")}
          </p>
        )}
      </section>
      {missingNodes.length > 0 && (
        <section className="banner error" role="alert" aria-label="不可用节点">
          <p>
            以下节点类型不可用，暂不能运行。原有参数、连接和导出已保留，可继续编辑并另存。
          </p>
          <ul>
            {missingNodes.map((node) => (
              <li key={node.id}>
                <code>{node.id}</code>：<code>{node.type}</code>
              </li>
            ))}
          </ul>
          <p>若来自插件，请在设置中安装或启用兼容插件，重启后重新校验。</p>
        </section>
      )}
      {error && (
        <div className="banner error" role="alert">
          {error}
          <button onClick={() => setError("")}>关闭</button>
        </div>
      )}
      {draft.localGraph.error && (
        <div className="banner error" role="alert">
          {draft.localGraph.error} 请在下方 JSON 中修正。
        </div>
      )}
      <div className="graph-workspace">
        <div>
          <div className="action-row" role="toolbar" aria-label="节点编辑操作">
            <button disabled={fileBusy || !draft.canUndo} onClick={() => draft.undo()} title="撤销（Ctrl+Z）"><Undo2 className="ui-icon" size={16} aria-hidden="true" />撤销</button>
            <button disabled={fileBusy || !draft.canRedo} onClick={() => draft.redo()} title="重做（Ctrl+Shift+Z / Ctrl+Y）"><Redo2 className="ui-icon" size={16} aria-hidden="true" />重做</button>
            <button disabled={fileBusy || !selectedIds.length} onClick={copy} title="复制节点（Ctrl+C；Shift+单击多选）"><Copy className="ui-icon" size={16} aria-hidden="true" />复制</button>
            <button disabled={fileBusy || !graph || !draft.canPaste} onClick={paste} title="粘贴节点（Ctrl+V）"><ClipboardPaste className="ui-icon" size={16} aria-hidden="true" />粘贴</button>
          </div>
          {graph && (
            <GraphCanvas
              graph={graph}
              nodes={nodes}
              catalogAvailable={!!connection}
              view={view}
              onView={draft.setCanvasView}
              selectedIds={selectedIds}
              onSelect={selectNode}
              disabled={fileBusy}
              connectionEditReason={
                !connection
                  ? "打开工作区后可编辑连线"
                  : missingNodes.length
                    ? "节点类型不可用，连线暂为只读"
                    : undefined
              }
              onConnect={(from, to, originalIndex) =>
                connection &&
                change((value) =>
                  rewireGraphPorts(value, nodes, from, to, originalIndex),
                )
              }
              onDisconnect={(index) =>
                connection &&
                change((value) => removeGraphConnection(value, index))
              }
            />
          )}
        </div>
        <aside className="graph-sidebar">
          {selected && graph ? (
            <>
              {deleteId === selected.id && (
                <div className="panel">
                  <p>删除 {selected.id} 及其连接和导出？</p>
                  <div className="action-row">
                    <button onClick={() => setDeleteId(null)}>取消</button>
                    <button
                      className="danger"
                      disabled={fileBusy}
                      onClick={() => {
                        draft.editGraph(JSON.stringify(removeGraphNode(graph, selected.id), null, 2), { ...view, selectedId: "" });
                        setDeleteId(null);
                      }}
                    >
                      确认删除
                    </button>
                  </div>
                </div>
              )}
              <fieldset className="inspector-fieldset" disabled={fileBusy}>
                <NodeInspector
                  key={selected.id + ":" + selected.type}
                  node={selected}
                  descriptor={descriptor}
                  catalogAvailable={!!connection}
                  devices={session.devices}
                  canListDevices={!!connection?.allowDevices && !busy}
                  onListDevices={onDevices}
                  onParameter={(id, value) =>
                    change((current) =>
                      setNodeParameter(current, selected.id, id, value),
                    )
                  }
                  onDelete={() => setDeleteId(selected.id)}
                  exports={graph.exports ?? []}
                  onExport={(port, name) =>
                    change((current) =>
                      setGraphExport(current, selected.id, port, name),
                    )
                  }
                  onRemoveExport={(index) =>
                    change((current) => removeGraphExport(current, index))
                  }
                />
              </fieldset>
            </>
          ) : (
            <div className="panel">
              <p className="muted">选择节点以编辑参数</p>
            </div>
          )}
          <section className="panel" aria-labelledby="node-palette-title">
            <div className="section-heading">
              <h2 id="node-palette-title"><Plus className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />添加节点</h2>
              <span className="badge">{available.length}</span>
            </div>
            <input
              aria-label="搜索节点"
              placeholder="搜索节点"
              value={selection.search}
              onChange={(event) =>
                onSelection({ ...selection, search: event.target.value })
              }
            />
            <div className="node-picker">
              {filtered.map((item) => (
                <button
                  key={item.typeId}
                  title={item.description}
                  disabled={!graph || fileBusy}
                  aria-label={`添加节点 ${item.displayName}`}
                  onClick={() => {
                    if (!graph || fileBusy) return;
                    try {
                      const added = addGraphNode(graph, item);
                      const positions = resolvedPositions(
                        graph,
                        nodes,
                        view.positions,
                      );
                      const x = 36;
                      const y = Math.max(
                        36,
                        ...graph.nodes.map((node) => {
                          const ports = nodePorts(
                            graph,
                            node,
                            nodes.find((info) => info.typeId === node.type),
                          );
                          return (
                            positions[positionKey(node)].y +
                            HEADER_HEIGHT +
                            Math.max(
                              ports.inputs.length,
                              ports.outputs.length,
                              1,
                            ) *
                              PORT_HEIGHT +
                            72
                          );
                        }),
                      );
                      positions[
                        positionKey(
                          added.graph.nodes[added.graph.nodes.length - 1],
                        )
                      ] = { x, y };
                      draft.editGraph(JSON.stringify(added.graph, null, 2), {
                        ...view,
                        positions,
                        selectedId: added.nodeId,
                      });
                      setError("");
                    } catch (reason) {
                      setError(formatError(reason));
                    }
                  }}
                >
                  <span><Plus className="ui-icon" size={16} strokeWidth={1.65} aria-hidden="true" />{item.displayName}</span>
                  <code>{item.typeId}</code>
                </button>
              ))}
            </div>
            {!connection && <p className="hint">打开工作区以读取节点目录。</p>}
            {connection && !filtered.length && (
              <p className="hint">当前模式下没有匹配节点。</p>
            )}
          </section>
        </aside>
      </div>
      <Disclosure
        label=" Graph JSON"
        className="panel editor-json"
        open={!!draft.localGraph.error}
      >
        <div className="file-caption">{draft.fileLabel}{draft.modified ? " · 未保存修改" : ""}</div>
        <textarea
          id="graph-editor"
          aria-label="Graph JSON"
          className="code-editor"
          disabled={fileBusy}
          spellCheck={false}
          value={draft.graphText}
          onFocus={() => { jsonEdit.current++; }}
          onChange={(event) => draft.editGraph(event.target.value, undefined, `json-${jsonEdit.current}`)}
        />
      </Disclosure>
    </div>
  );
}
