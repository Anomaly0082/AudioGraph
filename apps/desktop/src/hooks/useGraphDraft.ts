import { useEffect, useMemo, useRef, useState } from "react";
import {
  chooseGraphFile,
  chooseNewGraphFile,
  confirmDesktop,
  loadGraphDesktop,
  saveGraphDesktop,
} from "../api/desktop";
import {
  buildTaskOptions,
  createTemplate,
  formatError,
  parseGraph,
  type Mode,
  type TemplateKind,
} from "../model";
import type { AiProposal } from "../ai-model";
import type { SessionState } from "../session-state-model";
import type { Connection, GraphSubmission } from "../types/desktop";
import { isCurrentValidationResponse } from "../workflow-model";
import { initialCanvasView, type CanvasView } from "../graph-canvas-layout";
import { createGraphHistory } from "../graph-edit-history";
import { copyGraphSelection, pasteGraphSelection } from "../graph-clipboard";
import { editorModified, editorWorkspaceContext, transitionEditorWorkspace, sameGraphContent, savedEditorSource, sourceForContext, type EditorSource, type EditorWorkspace } from "../editor-source-model";

const initialGraph = JSON.stringify(createTemplate("text").graph, null, 2);

export function useGraphDraft(sessionId: string | null, workspace = "") {
  const [mode, setModeState] = useState<Mode>("offline");
  const [template, setTemplate] = useState<TemplateKind>("text");
  const [graphText, setGraphText] = useState(initialGraph);
  // Layout is presentation-only: never serialize it into the executable Graph.
  const [canvasView, setCanvasViewState] = useState<CanvasView>(initialCanvasView);
  const history = useRef(createGraphHistory({ text: initialGraph, view: initialCanvasView() }));
  const clipboard = useRef<ReturnType<typeof copyGraphSelection> | null>(null);
  const [canPaste, setCanPaste] = useState(false);
  const pasteCount = useRef(0);
  const [fileLabel, setFileLabel] = useState("未保存的 Graph");
  const [, setSource] = useState<EditorSource | null>(null);
  const sourceRef = useRef<EditorSource | null>(null);
  const fileBusy = useRef(false);
  const contentRef = useRef(graphText);
  contentRef.current = graphText;
  const originEpoch = useRef(0);
  const sourceContext = JSON.stringify([sessionId, workspace]);
  const workspaceRef = useRef<EditorWorkspace>({ workspace: null, scope: "" });
  const transition = transitionEditorWorkspace(workspaceRef.current, sourceRef.current, sessionId === null ? null : workspace);
  workspaceRef.current = transition.owner;
  sourceRef.current = transition.source;
  const fileContext = editorWorkspaceContext(transition.owner);
  const contextRef = useRef(sourceContext);
  if (contextRef.current !== sourceContext) {
    contextRef.current = sourceContext;
    ++originEpoch.current;
  }
  function bindSource(value: EditorSource | null) {
    sourceRef.current = value;
    setSource(value);
  }
  function replaceGraph(value: string, label = "未保存的 Graph") {
    ++originEpoch.current;
    bindSource(null);
    applySnapshot(history.current.reset({ text: value, view: initialCanvasView() }));
    setFileLabel(label);
  }
  const [blockFrames, setBlockFramesState] = useState("256");
  const [duration, setDurationState] = useState("10");
  const [probe, setProbeState] = useState(true);
  const [validatedKey, setValidatedKey] = useState<string | null>(null);
  const sessionRef = useRef(sessionId);
  sessionRef.current = sessionId;
  const previousSessionRef = useRef(sourceContext);
  useEffect(() => {
    if (previousSessionRef.current === sourceContext) return;
    previousSessionRef.current = sourceContext;
    setProbeState(true);
    setValidatedKey(null);
    setSource(sourceRef.current);
  }, [sourceContext]);
  const previousFileContext = useRef(fileContext);
  useEffect(() => {
    if (previousFileContext.current === fileContext) return;
    previousFileContext.current = fileContext;
    setFileLabel("未保存的 Graph（工作区已变化）");
    applySnapshot(history.current.reset(history.current.current));
    clipboard.current = null; setCanPaste(false);
  }, [fileContext]);
  const validationKey = JSON.stringify([
    sessionId,
    originEpoch.current,
    mode,
    graphText,
    blockFrames,
    duration,
    probe,
  ]);
  const keyRef = useRef(validationKey);
  keyRef.current = validationKey;
  const validationCurrent =
    sessionId !== null && validatedKey === validationKey;
  const localGraph = useMemo(() => {
    try {
      return { graph: parseGraph(graphText, { allowEmpty: true }), error: "" };
    } catch (reason) {
      return { graph: null, error: formatError(reason) };
    }
  }, [graphText]);

  function applySnapshot(value: { text: string; view: CanvasView }) {
    contentRef.current = value.text;
    setGraphText(value.text);
    setCanvasViewState(value.view);
    setValidatedKey(null);
  }
  function editGraph(value: string, nextView?: CanvasView, group?: string) {
    applySnapshot(history.current.commit({ text: value, view: nextView ?? history.current.current.view }, group));
  }
  function setCanvasView(value: CanvasView, group?: string) {
    const current = history.current.current;
    if (JSON.stringify(current.view.positions) !== JSON.stringify(value.positions)) {
      const snapshot = history.current.commit({ text: current.text, view: value }, group);
      setCanvasViewState(snapshot.view);
    } else {
      setCanvasViewState(history.current.replaceView(value).view);
    }
  }
  function undo() { if (!fileBusy.current) applySnapshot(history.current.undo()); }
  function redo() { if (!fileBusy.current) applySnapshot(history.current.redo()); }
  function copyNodes(ids: string[], positions: CanvasView["positions"]) {
    const graph = parseGraph(contentRef.current, { allowEmpty: true });
    if (!ids.some(id => graph.nodes.some(node => node.id === id))) return;
    clipboard.current = copyGraphSelection(graph, ids, positions);
    pasteCount.current = 0;
    setCanPaste(true);
  }
  function pasteNodes(positions: CanvasView["positions"]): string[] {
    if (!clipboard.current || fileBusy.current) return [];
    const pasted = pasteGraphSelection(parseGraph(contentRef.current, { allowEmpty: true }), clipboard.current, positions, 32 * (pasteCount.current + 1));
    const text = JSON.stringify(pasted.graph, null, 2);
    parseGraph(text, { allowEmpty: true });
    editGraph(text, { ...history.current.current.view, positions: pasted.positions, selectedId: pasted.selectedIds[0] ?? "" });
    pasteCount.current++;
    return pasted.selectedIds;
  }
  function setMode(value: Mode) {
    setModeState(value);
    setValidatedKey(null);
  }
  function setBlockFrames(value: string) {
    setBlockFramesState(value);
    setValidatedKey(null);
  }
  function setDuration(value: string) {
    setDurationState(value);
    setValidatedKey(null);
  }
  function setProbe(value: boolean) {
    setProbeState(value);
    setValidatedKey(null);
  }
  function buildSubmission(): GraphSubmission {
    return {
      mode,
      graph: parseGraph(graphText),
      options: buildTaskOptions(mode, {
        blockFrames: Number(blockFrames),
        durationSeconds: Number(duration),
        probe,
      }),
    };
  }
  function acceptValidation(
    expectedSessionId: string,
    expectedKey: string,
  ): boolean {
    if (
      !isCurrentValidationResponse(
        expectedSessionId,
        expectedKey,
        sessionRef.current,
        keyRef.current,
      )
    )
      return false;
    setValidatedKey(expectedKey);
    return true;
  }
  async function loadTemplate(): Promise<boolean> {
    if (fileBusy.current) return false;
    const captured = contentRef.current;
    const epoch = originEpoch.current;
    if (
      !(await confirmDesktop(
        "载入模板会替换当前编辑内容。请先保存需要保留的 Graph。继续？",
        "载入模板",
      ))
    )
      return false;
    if (fileBusy.current || captured !== contentRef.current || epoch !== originEpoch.current) return false;
    const value = createTemplate(template);
    replaceGraph(JSON.stringify(value.graph, null, 2));
    setMode(value.mode);
    setProbe(true);
    setFileLabel("未保存的 Graph");
    return true;
  }
  async function loadGraph(connection: Connection): Promise<boolean> {
    if (sessionRef.current !== connection.sessionId) return false;
    if (fileBusy.current) return false;
    fileBusy.current = true;
    const captured = contentRef.current;
    const epoch = originEpoch.current;
    try {
    const selected = await chooseGraphFile(connection.workspace);
    if (!selected) return false;
    if (sessionRef.current !== connection.sessionId || epoch !== originEpoch.current) return false;
    const loaded = await loadGraphDesktop(connection.sessionId, selected);
    const content = JSON.stringify(loaded.graph, null, 2);
    parseGraph(content);
    if (
      !(await confirmDesktop(
        "文件已解析。是否替换当前编辑内容？执行模式请自行核对。",
        "打开 Graph",
        "info",
      ))
    )
      return false;
    if (sessionRef.current !== connection.sessionId || epoch !== originEpoch.current || contentRef.current !== captured)
      throw new Error("连接已变化，未载入旧工作区的 Graph。");
    replaceGraph(content, loaded.path);
    bindSource({ context: fileContext, path: loaded.path, revision: loaded.revision, content });
    return true;
    } finally { fileBusy.current = false; }
  }
  async function loadFromFile(connection: Connection, path: string, preview: string, stillCurrent: () => boolean): Promise<boolean> {
    if (sessionRef.current !== connection.sessionId) return false;
    if (fileBusy.current) return false;
    fileBusy.current = true;
    const captured = contentRef.current;
    const epoch = originEpoch.current;
    try {
      const loaded = await loadGraphDesktop(connection.sessionId, path);
      const content = JSON.stringify(loaded.graph, null, 2);
      if (!sameGraphContent(loaded.graph, parseGraph(preview, { allowEmpty: true })))
        throw new Error("文件已变化，请刷新预览后重新打开。");
      if (!stillCurrent() || sessionRef.current !== connection.sessionId || epoch !== originEpoch.current || contentRef.current !== captured) return false;
      replaceGraph(content, loaded.path);
      bindSource({ context: fileContext, path: loaded.path, revision: loaded.revision, content });
      return true;
    } finally { fileBusy.current = false; }
  }
  async function saveGraph(connection: Connection, saveAs = false): Promise<string | null> {
    if (sessionRef.current !== connection.sessionId) return null;
    if (fileBusy.current) return null;
    fileBusy.current = true;
    const content = contentRef.current;
    const epoch = originEpoch.current;
    const capturedContext = sourceContext;
    const origin = saveAs ? null : sourceForContext(sourceRef.current, fileContext);
    try {
    const graph = parseGraph(content);
    const selected = origin?.path ?? await chooseNewGraphFile(connection.workspace);
    if (!selected) return null;
    if (sessionRef.current !== connection.sessionId || epoch !== originEpoch.current)
      throw new Error("连接已变化，未保存旧工作区的 Graph。");
    const saved = await saveGraphDesktop(connection.sessionId, selected, graph, origin?.revision);
    if (sessionRef.current === connection.sessionId && epoch === originEpoch.current) {
      const accepted = savedEditorSource(capturedContext, contextRef.current, saved.path, saved.revision, content);
      bindSource(accepted && { ...accepted, context: fileContext });
      setFileLabel(saved.path);
    }
    return saved.path;
    } catch (reason) {
      throw new Error(`${formatError(reason)} 请重新打开文件或另存为新路径；草稿已保留。`);
    } finally { fileBusy.current = false; }
  }
  function applyProposal(proposal: AiProposal, label = "AI 提案（未保存）") {
    replaceGraph(JSON.stringify(proposal.graph, null, 2), label);
    setMode(proposal.mode);
    setProbe(true);
    setFileLabel(label);
    if (
      proposal.mode === "streaming" &&
      typeof proposal.options.block_frames === "number"
    ) {
      setBlockFrames(String(proposal.options.block_frames));
    }
  }
  return {
    exportSessionState: (): SessionState["graph"] => ({ text: graphText, mode, blockFrames, duration, canvasView, source: sourceRef.current && workspaceRef.current.workspace ? { path: sourceRef.current.path, revision: sourceRef.current.revision, content: sourceRef.current.content, workspace: workspaceRef.current.workspace, space: "user" } : null }),
    restoreSessionState(value: SessionState["graph"], workspace: string | null) {
      ++originEpoch.current;
      const owner = transitionEditorWorkspace({ workspace: null, scope: "" }, null, workspace).owner;
      workspaceRef.current = owner;
      previousFileContext.current = editorWorkspaceContext(owner);
      bindSource(value.source ? { path: value.source.path, revision: value.source.revision, content: value.source.content, context: editorWorkspaceContext(transitionEditorWorkspace({ workspace: null, scope: "" }, null, value.source.workspace).owner) } : null);
      applySnapshot(history.current.reset({ text: value.text, view: value.canvasView })); setModeState(value.mode); setBlockFramesState(value.blockFrames); setDurationState(value.duration); setProbeState(true); setValidatedKey(null);
      setFileLabel(value.source?.path ?? "恢复的 Graph 草稿");
    },
    mode,
    setMode,
    template,
    setTemplate,
    graphText,
    canvasView,
    setCanvasView,
    editGraph,
    undo, redo, canUndo: history.current.canUndo, canRedo: history.current.canRedo,
    copyNodes, pasteNodes, canPaste,
    replaceGraph,
    fileLabel,
    modified: editorModified(sourceForContext(sourceRef.current, fileContext), graphText),
    blockFrames,
    setBlockFrames,
    duration,
    setDuration,
    probe,
    setProbe,
    localGraph,
    validationKey,
    captureDraftKey: () => keyRef.current,
    isCurrentDraftKey: (key: string) => keyRef.current === key,
    validatedKey,
    setValidatedKey,
    validationCurrent,
    buildSubmission,
    acceptValidation,
    loadTemplate,
    loadGraph,
    loadFromFile,
    saveGraph,
    applyProposal,
  };
}
export type GraphDraft = ReturnType<typeof useGraphDraft>;
