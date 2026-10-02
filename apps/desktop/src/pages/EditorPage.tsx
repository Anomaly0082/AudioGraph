import { useEffect, useState } from "react";
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
  onDevices,
  onTasks,
}: Props) {
  const { connection, task, busy } = session;
  const nodes = connection?.capabilities.nodes ?? [];
  const graph = draft.localGraph.graph;
  const view = draft.canvasView ?? initialCanvasView();
  const [error, setError] = useState("");
  const [deleteId, setDeleteId] = useState<string | null>(null);
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
    <div className="visual-editor">
      <section className="panel">
        <div className="section-heading">
          <h2>Graph</h2>
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
            载入模板
          </button>
          <button
            disabled={!session.desktop || !connection || !!busy || fileBusy}
            onClick={onLoad}
          >
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
            另存为新文件
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
              校验
            </button>
            <button
              className="primary"
              disabled={
                !connection ||
                submittingLocked ||
                session.taskBlocked ||
                !draft.validationCurrent ||
                !canSubmitGraph
              }
              onClick={onRun}
            >
              提交任务
            </button>
            {task && <button onClick={onTasks}>查看运行记录</button>}
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
          {graph && (
            <GraphCanvas
              graph={graph}
              nodes={nodes}
              view={view}
              onView={draft.setCanvasView}
              disabled={fileBusy}
              connectionEditReason={
                !connection ? "打开工作区后可编辑连线" : undefined
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
                        change((value) => removeGraphNode(value, selected.id));
                        draft.setCanvasView({ ...view, selectedId: "" });
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
              <h2 id="node-palette-title">添加节点</h2>
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
                      draft.editGraph(JSON.stringify(added.graph, null, 2));
                      draft.setCanvasView({
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
                  <span>＋ {item.displayName}</span>
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
        <div className="file-caption">{draft.fileLabel}</div>
        <textarea
          id="graph-editor"
          aria-label="Graph JSON"
          className="code-editor"
          disabled={fileBusy}
          spellCheck={false}
          value={draft.graphText}
          onChange={(event) => draft.editGraph(event.target.value)}
        />
      </Disclosure>
    </div>
  );
}
