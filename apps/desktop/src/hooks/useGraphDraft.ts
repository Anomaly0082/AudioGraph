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
import type { Connection, GraphSubmission } from "../types/desktop";
import { isCurrentValidationResponse } from "../workflow-model";
import { initialCanvasView, type CanvasView } from "../graph-canvas-layout";

const initialGraph = JSON.stringify(createTemplate("text").graph, null, 2);

export function useGraphDraft(sessionId: string | null) {
  const [mode, setModeState] = useState<Mode>("offline");
  const [template, setTemplate] = useState<TemplateKind>("text");
  const [graphText, setGraphText] = useState(initialGraph);
  // Layout is presentation-only: never serialize it into the executable Graph.
  const [canvasView, setCanvasView] = useState<CanvasView>(initialCanvasView);
  const [fileLabel, setFileLabel] = useState("未保存的 Graph");
  const [blockFrames, setBlockFramesState] = useState("256");
  const [duration, setDurationState] = useState("10");
  const [probe, setProbeState] = useState(true);
  const [validatedKey, setValidatedKey] = useState<string | null>(null);
  const sessionRef = useRef(sessionId);
  sessionRef.current = sessionId;
  const previousSessionRef = useRef(sessionId);
  useEffect(() => {
    if (previousSessionRef.current === sessionId) return;
    previousSessionRef.current = sessionId;
    setProbeState(true);
    setValidatedKey(null);
  }, [sessionId]);
  const validationKey = JSON.stringify([
    sessionId,
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

  function editGraph(value: string) {
    setGraphText(value);
    setValidatedKey(null);
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
    if (
      !(await confirmDesktop(
        "载入模板会替换当前编辑内容。请先保存需要保留的 Graph。继续？",
        "载入模板",
      ))
    )
      return false;
    const value = createTemplate(template);
    setCanvasView(initialCanvasView());
    editGraph(JSON.stringify(value.graph, null, 2));
    setMode(value.mode);
    setProbe(true);
    setFileLabel("未保存的 Graph");
    return true;
  }
  async function loadGraph(connection: Connection): Promise<boolean> {
    const selected = await chooseGraphFile(connection.workspace);
    if (!selected) return false;
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
    if (sessionRef.current !== connection.sessionId)
      throw new Error("连接已变化，未载入旧工作区的 Graph。");
    editGraph(content);
    setCanvasView(initialCanvasView());
    setFileLabel(loaded.path);
    return true;
  }
  async function saveGraph(connection: Connection): Promise<string | null> {
    const graph = parseGraph(graphText);
    const draftKey = keyRef.current;
    const selected = await chooseNewGraphFile(connection.workspace);
    if (!selected) return null;
    if (sessionRef.current !== connection.sessionId)
      throw new Error("连接已变化，未保存旧工作区的 Graph。");
    const saved = await saveGraphDesktop(connection.sessionId, selected, graph);
    if (
      isCurrentValidationResponse(
        connection.sessionId,
        draftKey,
        sessionRef.current,
        keyRef.current,
      )
    ) {
      setFileLabel(saved.path);
    }
    return saved.path;
  }
  function applyProposal(proposal: AiProposal, label = "AI 提案（未保存）") {
    setCanvasView(initialCanvasView());
    editGraph(JSON.stringify(proposal.graph, null, 2));
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
    mode,
    setMode,
    template,
    setTemplate,
    graphText,
    canvasView,
    setCanvasView,
    editGraph,
    fileLabel,
    blockFrames,
    setBlockFrames,
    duration,
    setDuration,
    probe,
    setProbe,
    localGraph,
    validationKey,
    validatedKey,
    setValidatedKey,
    validationCurrent,
    buildSubmission,
    acceptValidation,
    loadTemplate,
    loadGraph,
    saveGraph,
    applyProposal,
  };
}
export type GraphDraft = ReturnType<typeof useGraphDraft>;
