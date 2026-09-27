import { useEffect, useRef, useState } from "react";
import {
  createAiRequestId, getFileParameters, isCurrentAiResponse, loadedAiDraft, normalizeAiConfig,
  sameAiConfig, type AiSettingsLoad, type AiSettingsWrite,
  normalizeAiProposal, runApprovedProposal, type AiConfig, type AiGenerateResponse, type AiNodeInfo,
  type AiProposal, type AiSummaryResponse, type AiTaskSnapshot, type AudioInspection,
} from "./ai-model";
import { formatError, isTerminal } from "./model";

type AiInvoke = <T>(command: string, args: Record<string, unknown>) => Promise<T>;
type Phase = "idle" | "generating" | "stopping" | "proposal" | "executing" | "summarizing" | "done";
const phaseLabels: Record<Phase, string> = {
  idle: "待命", generating: "生成提案", stopping: "正在停止", proposal: "等待确认",
  executing: "执行任务", summarizing: "解释结果", done: "本轮结束",
};

export type AiPanelProps = {
  sessionId: string | null;
  nodes: AiNodeInfo[];
  disabled: boolean;
  hasTask: boolean;
  task: AiTaskSnapshot | null;
  invokeAi: AiInvoke;
  onBusyChange: (busy: boolean) => void;
  onApplyProposal: (proposal: AiProposal) => void;
  onStartProposal: (proposal: AiProposal) => Promise<string>;
  onCancelTask: (taskId: string) => Promise<void>;
};

const initialConfig: AiConfig = { baseUrl: "https://api.openai.com/v1", model: "", apiKey: "" };

export default function AiPanel({ sessionId, nodes, disabled, hasTask, task, invokeAi, onBusyChange,
  onApplyProposal, onStartProposal, onCancelTask }: AiPanelProps) {
  const [config, setConfig] = useState(initialConfig);
  const [savedConfig, setSavedConfig] = useState<AiConfig | null>(null);
  const [settingsPath, setSettingsPath] = useState("");
  const [settingsLoading, setSettingsLoading] = useState(true);
  const [settingsBusy, setSettingsBusy] = useState(false);
  const [settingsError, setSettingsError] = useState("");
  const [settingsNotice, setSettingsNotice] = useState("");
  const [prompt, setPrompt] = useState("");
  const [inputPath, setInputPath] = useState("");
  const [phase, setPhase] = useState<Phase>("idle");
  const [calls, setCalls] = useState(0);
  const [assistantText, setAssistantText] = useState("");
  const [summary, setSummary] = useState("");
  const [proposal, setProposal] = useState<AiProposal | null>(null);
  const [inspection, setInspection] = useState<AudioInspection | null>(null);
  const [error, setError] = useState("");
  const [aiTaskId, setAiTaskId] = useState<string | null>(null);
  const requestIdRef = useRef<string | null>(null);
  const stoppingRequestIdRef = useRef<string | null>(null);
  const summaryStartedRef = useRef(false);
  const submitStartedRef = useRef(false);
  const mountedRef = useRef(true);
  const sessionIdRef = useRef(sessionId);
  const cancelTaskRef = useRef(onCancelTask);
  const hasTaskRef = useRef(hasTask);
  const disabledRef = useRef(disabled);
  const settingsBusyRef = useRef(false);
  const settingsEditRevisionRef = useRef(0);
  const settingsMutationRef = useRef(0);
  cancelTaskRef.current = onCancelTask;
  hasTaskRef.current = hasTask;
  disabledRef.current = disabled;
  const workflowBusy = phase === "generating" || phase === "stopping" || phase === "proposal" ||
    phase === "executing" || phase === "summarizing";

  useEffect(() => { mountedRef.current = true; return () => { mountedRef.current = false; }; }, []);
  useEffect(() => {
    let active = true;
    const editRevision = settingsEditRevisionRef.current;
    const mutation = settingsMutationRef.current;
    void invokeAi<AiSettingsLoad>("ai_load_settings", {}).then((loaded) => {
      if (!active || settingsMutationRef.current !== mutation) return;
      const loadedConfig = loaded.config === null ? null : normalizeAiConfig(loaded.config);
      setSettingsPath(loaded.path);
      setSavedConfig(loadedConfig);
      setConfig((current) => loadedAiDraft(current, loadedConfig,
        settingsEditRevisionRef.current !== editRevision));
      setSettingsError("");
    }).catch((reason) => {
      if (active && settingsMutationRef.current === mutation) setSettingsError(`读取本机配置失败：${formatError(reason)}`);
    }).finally(() => { if (active) setSettingsLoading(false); });
    return () => { active = false; };
  }, [invokeAi]);
  useEffect(() => { onBusyChange(workflowBusy || settingsBusy); }, [workflowBusy, settingsBusy, onBusyChange]);
  useEffect(() => () => { onBusyChange(false); }, [onBusyChange]);
  useEffect(() => {
    const previous = sessionIdRef.current;
    sessionIdRef.current = sessionId;
    if (previous === sessionId) return;
    const requestId = requestIdRef.current;
    requestIdRef.current = null; stoppingRequestIdRef.current = null;
    submitStartedRef.current = false; summaryStartedRef.current = false;
    if (requestId) void invokeAi("ai_cancel_request", { requestId }).catch(() => undefined);
    setProposal(null); setInspection(null); setAiTaskId(null); setSummary(""); setCalls(0); setPhase("idle");
    if (previous) setError("后端会话已变化，旧会话的 AI 请求或提案已失效。");
  }, [invokeAi, sessionId]);

  function editConfig(change: (current: AiConfig) => AiConfig) {
    settingsEditRevisionRef.current++;
    setConfig(change);
    setSettingsNotice("");
  }

  async function saveSettings(safeConfig: AiConfig): Promise<boolean> {
    if (settingsBusyRef.current || settingsLoading) return false;
    settingsBusyRef.current = true;
    settingsMutationRef.current++;
    setSettingsBusy(true); setSettingsError(""); setSettingsNotice("");
    const editRevision = settingsEditRevisionRef.current;
    try {
      const result = await invokeAi<AiSettingsWrite>("ai_save_settings", { config: safeConfig });
      if (!mountedRef.current) return false;
      setSettingsPath(result.path);
      setSettingsLoading(false);
      setSavedConfig(safeConfig);
      if (settingsEditRevisionRef.current === editRevision) setConfig(safeConfig);
      setSettingsNotice("配置已保存到本机。");
      return true;
    } catch (reason) {
      if (mountedRef.current) setSettingsError(`保存本机配置失败：${formatError(reason)}`);
      return false;
    } finally {
      settingsBusyRef.current = false;
      if (mountedRef.current) setSettingsBusy(false);
    }
  }

  function saveCurrentSettings() {
    if (settingsBusyRef.current || settingsLoading || workflowBusy) return;
    let safeConfig: AiConfig;
    try { safeConfig = normalizeAiConfig(config); }
    catch (reason) { setSettingsError(`保存本机配置失败：${formatError(reason)}`); return; }
    void saveSettings(safeConfig);
  }

  async function clearSavedSettings() {
    if (settingsBusyRef.current || settingsLoading || workflowBusy) return;
    settingsBusyRef.current = true;
    settingsMutationRef.current++;
    setSettingsBusy(true); setSettingsError(""); setSettingsNotice("");
    try {
      const result = await invokeAi<AiSettingsWrite>("ai_clear_settings", {});
      if (!mountedRef.current) return;
      setSettingsPath(result.path);
      setSettingsLoading(false);
      setSavedConfig(null);
      settingsEditRevisionRef.current++;
      setConfig((current) => ({ ...current, apiKey: "" }));
      setSettingsNotice("已删除本机保存的配置，并清除当前输入的 API Key。");
    } catch (reason) {
      if (mountedRef.current) setSettingsError(`清除本机配置失败：${formatError(reason)}`);
    } finally {
      settingsBusyRef.current = false;
      if (mountedRef.current) setSettingsBusy(false);
    }
  }

  async function generate() {
    if (!sessionId || disabled || hasTask || workflowBusy || settingsLoading || settingsBusyRef.current || !prompt.trim()) return;
    let safeConfig: AiConfig;
    try { safeConfig = normalizeAiConfig(config); }
    catch (reason) { setError(formatError(reason)); return; }
    if (!sameAiConfig(safeConfig, savedConfig) && !await saveSettings(safeConfig)) return;
    if (!mountedRef.current || sessionIdRef.current !== sessionId || hasTaskRef.current || disabledRef.current) return;
    const requestId = createAiRequestId();
    requestIdRef.current = requestId;
    stoppingRequestIdRef.current = null;
    submitStartedRef.current = false;
    summaryStartedRef.current = false;
    setCalls(1); setProposal(null); setInspection(null); setAssistantText(""); setSummary(""); setAiTaskId(null);
    setError(""); setPhase("generating");
    try {
      const response = await invokeAi<AiGenerateResponse>("ai_generate", {
        sessionId, requestId, config: safeConfig, prompt: prompt.trim(), inputPath: inputPath.trim() || null,
      });
      if (stoppingRequestIdRef.current === requestId) {
        stoppingRequestIdRef.current = null; requestIdRef.current = null;
        setError("已停止 AI 请求。"); setPhase("idle"); return;
      }
      if (!mountedRef.current || sessionIdRef.current !== sessionId ||
          !isCurrentAiResponse(response.requestId, requestIdRef.current)) return;
      setAssistantText(response.text || "模型没有返回说明。");
      setInspection(response.inspection ?? null);
      if (response.proposal === undefined) { requestIdRef.current = null; setPhase("done"); return; }
      setProposal(normalizeAiProposal(response.proposal));
      requestIdRef.current = null;
      setPhase("proposal");
    } catch (reason) {
      if (!mountedRef.current || requestIdRef.current !== requestId) return;
      requestIdRef.current = null;
      setError(stoppingRequestIdRef.current === requestId ? "已停止 AI 请求。" : formatError(reason));
      stoppingRequestIdRef.current = null;
      setPhase("idle");
    }
  }

  async function stopAiRequest() {
    const requestId = requestIdRef.current;
    if (!requestId || (phase !== "generating" && phase !== "summarizing" && phase !== "stopping")) return;
    const expectedSessionId = sessionIdRef.current;
    setPhase("stopping");
    stoppingRequestIdRef.current = requestId;
    try { await invokeAi<{ cancelled: boolean }>("ai_cancel_request", { requestId }); }
    catch (reason) {
      if (mountedRef.current && requestIdRef.current === requestId &&
          stoppingRequestIdRef.current === requestId && sessionIdRef.current === expectedSessionId) {
        stoppingRequestIdRef.current = null;
        setError(`停止请求失败：${formatError(reason)}`);
        setPhase(calls === 1 ? "generating" : "summarizing");
      }
    }
  }

  async function confirmAndRun() {
    if (!proposal || !sessionId || phase !== "proposal") return;
    const expectedSessionId = sessionId;
    setError(""); setPhase("executing");
    try {
      const taskId = await runApprovedProposal({ approved: true, expectedSessionId,
        currentSessionId: sessionIdRef.current, startedRef: submitStartedRef }, proposal,
        onApplyProposal, onStartProposal);
      if (!taskId) return;
      if (sessionIdRef.current !== expectedSessionId) {
        submitStartedRef.current = false; setProposal(null);
        setError("后端会话已变化；旧会话的任务响应不会继续驱动 AI 流程。"); setPhase("done"); return;
      }
      setAiTaskId(taskId);
    } catch (reason) {
      if (sessionIdRef.current !== expectedSessionId) {
        submitStartedRef.current = false; setProposal(null);
        setError("后端会话已变化，旧会话的 AI 提案已失效。"); setPhase("done"); return;
      }
      setError(formatError(reason));
      setPhase("proposal");
    }
  }

  async function cancelExecution() {
    if (!aiTaskId || phase !== "executing") return;
    try { await onCancelTask(aiTaskId); }
    catch (reason) { setError(formatError(reason)); }
  }

  useEffect(() => {
    if (!sessionId || !proposal || !aiTaskId || !task || task.id !== aiTaskId ||
        task.sessionId !== sessionId || !isTerminal(task.state) || summaryStartedRef.current) return;
    summaryStartedRef.current = true;
    if (task.state === "cancelled") {
      setError("真实任务已取消；未发送第二个模型请求。AI 解释不能改变此结果。");
      setPhase("done"); return;
    }
    const requestId = createAiRequestId();
    requestIdRef.current = requestId;
    setCalls(2); setPhase("summarizing"); setError("");
    let safeConfig: AiConfig;
    try { safeConfig = normalizeAiConfig(config); }
    catch (reason) { setError(`任务已结束，但无法请求 AI 解释：${formatError(reason)}`); setPhase("done"); return; }
    invokeAi<AiSummaryResponse>("ai_summarize", {
      sessionId, requestId, config: safeConfig, prompt: prompt.trim(), proposal,
      result: { state: task.state, result: task.result, errors: task.errors },
    }).then((response) => {
      if (stoppingRequestIdRef.current === requestId) {
        stoppingRequestIdRef.current = null; requestIdRef.current = null;
        setError(`真实任务状态为“${task.state}”；已停止 AI 解释请求。`); setPhase("done"); return;
      }
      if (!mountedRef.current || sessionIdRef.current !== sessionId ||
          !isCurrentAiResponse(response.requestId, requestIdRef.current)) return;
      requestIdRef.current = null;
      setSummary(response.text || "模型没有返回结果说明。"); setPhase("done");
    }).catch((reason) => {
      if (!mountedRef.current || requestIdRef.current !== requestId) return;
      requestIdRef.current = null;
      const stopped = stoppingRequestIdRef.current === requestId;
      stoppingRequestIdRef.current = null;
      setError(stopped ? `真实任务状态为“${task.state}”；已停止 AI 解释请求。` :
        `真实任务状态为“${task.state}”，但 AI 解释失败：${formatError(reason)}`);
      setPhase("done");
    });
  }, [aiTaskId, config, invokeAi, prompt, proposal, sessionId, task]);

  useEffect(() => {
    if (phase !== "executing" || !aiTaskId || !task || task.id !== aiTaskId || task.state !== "unknown") return;
    summaryStartedRef.current = true;
    setError("真实任务状态未知，AI 流程已停止等待；不会请求结果解释。请先检查输出，再决定是否断开或清除显示记录。");
    setPhase("done");
  }, [aiTaskId, phase, task]);

  useEffect(() => {
    if (phase !== "executing" || !aiTaskId) return;
    const timer = setTimeout(() => {
      setError("AI 任务等待已达到 10 分钟上限，正在请求取消；真实状态仍以下方任务区为准。");
      void cancelTaskRef.current(aiTaskId).catch((reason) => setError(`等待超时，取消请求失败：${formatError(reason)}`));
    }, 10 * 60 * 1000);
    return () => clearTimeout(timer);
  }, [aiTaskId, phase]);

  function rejectProposal() {
    if (phase !== "proposal") return;
    submitStartedRef.current = false; setProposal(null); setPhase("done");
  }

  function reset() {
    if (workflowBusy || settingsBusyRef.current) return;
    requestIdRef.current = null; summaryStartedRef.current = false;
    stoppingRequestIdRef.current = null;
    submitStartedRef.current = false;
    setPhase("idle"); setCalls(0); setAssistantText(""); setSummary(""); setProposal(null); setInspection(null);
    setError(""); setAiTaskId(null);
  }

  const files = proposal ? getFileParameters(proposal, nodes) : [];
  const unavailable = !sessionId || disabled || hasTask;
  return <section className="panel ai-panel" aria-labelledby="ai-title">
    <div className="section-heading"><div><span className="step">AI</span><h2 id="ai-title">AI Graph 助手</h2></div><span className="badge">{phaseLabels[phase]} · 模型阶段 {calls}/2</span></div>
    <p className="hint ai-disclosure">需求文字、节点目录、可选输入 WAV 的路径与真实音频元数据、提案中的文件名 / Graph，以及任务结果摘要会发送到你配置的模型服务；音频文件本体不会上传。每次流程最多发出 2 个 HTTP 请求，但这不代表供应商费用上限。模型文字仅供显示，不会作为代码或 Graph 自动执行。</p>
    <div className="ai-config-grid">
      <label>OpenAI 兼容地址<input value={config.baseUrl} disabled={workflowBusy || settingsBusy} placeholder="https://api.openai.com/v1" onChange={(event) => editConfig((current) => ({ ...current, baseUrl: event.target.value, apiKey: "" }))} /></label>
      <label>模型<input value={config.model} disabled={workflowBusy || settingsBusy} placeholder="请填写模型名称" onChange={(event) => editConfig((current) => ({ ...current, model: event.target.value }))} /></label>
      <label>API Key<div className="key-row"><input type="password" autoComplete="off" value={config.apiKey} disabled={workflowBusy || settingsBusy} placeholder="本地 HTTP 服务可留空" onChange={(event) => editConfig((current) => ({ ...current, apiKey: event.target.value }))} /><button type="button" disabled={workflowBusy || settingsBusy || !config.apiKey} onClick={() => editConfig((current) => ({ ...current, apiKey: "" }))}>清除当前输入</button></div></label>
    </div>
    <p className="hint">地址、模型和 API Key 保存在本机明文 JSON 文件中，不写入仓库；需求文字、音频路径和会话不会保存。生成提案前会自动保存有效配置。</p>
    <div className="action-row">
      <button type="button" disabled={workflowBusy || settingsLoading || settingsBusy} onClick={saveCurrentSettings}>保存配置</button>
      <button type="button" disabled={workflowBusy || settingsLoading || settingsBusy} onClick={clearSavedSettings}>清除已保存配置</button>
      <span className="hint" role="status">{settingsLoading ? "正在读取本机配置…" : savedConfig === null ? sameAiConfig(config, initialConfig) ? "本机尚无已保存配置" : "有未保存的修改" : sameAiConfig(config, savedConfig) ? "配置已保存" : "有未保存的修改"}</span>
    </div>
    {settingsPath && <p className="hint">保存位置：<code>{settingsPath}</code></p>}
    {settingsError && <div className="banner error ai-message" role="alert"><strong>本机配置提示</strong><pre>{settingsError}</pre></div>}
    {settingsNotice && <p className="hint" role="status">{settingsNotice}</p>}
    <label className="ai-request-label">输入 WAV 路径（可选）<input value={inputPath} disabled={workflowBusy || settingsBusy} placeholder="例如：input.wav（仅检查工作区内现存 PCM16 WAV）" onChange={(event) => setInputPath(event.target.value)} /></label>
    <label className="ai-request-label">处理需求<textarea value={prompt} disabled={workflowBusy || settingsBusy} placeholder="例如：读取 input.wav，降低 6 dB 后写入 output.wav" onChange={(event) => setPrompt(event.target.value)} /></label>
    <div className="action-row">
      <button className="primary" disabled={unavailable || workflowBusy || settingsLoading || settingsBusy || !prompt.trim()} onClick={generate}>{phase === "generating" ? "正在生成…" : "生成 Graph 提案"}</button>
      {(phase === "generating" || phase === "summarizing" || phase === "stopping") && <button className="danger" disabled={phase === "stopping"} onClick={stopAiRequest}>{phase === "stopping" ? "正在停止…" : "停止 AI 请求"}</button>}
      {phase === "executing" && <button className="danger" disabled={!task || task.id !== aiTaskId || !["queued", "running"].includes(task.state)} onClick={cancelExecution}>停止执行</button>}
      {(phase === "idle" || phase === "done") && calls > 0 && <button disabled={settingsBusy} onClick={reset}>新需求</button>}
      <span className="hint">{!sessionId ? "请先连接后端。" : hasTask && !aiTaskId ? "请先释放已有任务记录。" : phase === "proposal" ? "等待你的明确确认，不会自动执行。" : phase === "executing" ? "真实任务由下方任务区权威跟踪；等待上限 10 分钟。" : phase === "summarizing" ? "真实结果已保留，正在请求辅助解释。" : "最多一次提案、一次结果解释。"}</span>
    </div>
    {error && <div className="banner error ai-message" role="alert"><strong>AI 流程提示</strong><pre>{error}</pre></div>}
    {assistantText && <div className="ai-message"><h3>模型回复</h3><p>{assistantText}</p></div>}
    {inspection && <div className="ai-message"><h3>输入音频检查</h3><p><code>{inspection.path}</code> · {inspection.sample_rate} Hz · {inspection.channels} 声道 · {inspection.frame_count} 帧 · {inspection.duration_seconds.toFixed(3)} 秒 · {inspection.encoding}</p></div>}
    {proposal && <div className="ai-proposal"><div className="section-heading"><h3>待确认提案</h3><span className="badge">{proposal.mode}</span></div>
      <p className="hint">{proposal.graph.nodes.length} 个节点 · {proposal.graph.connections.length} 条连接</p>
      {files.length ? <ul className="ai-file-list">{files.map((file) => <li key={`${file.nodeId}-${file.parameterId}`}><code>{file.nodeId}.{file.parameterId}</code><span>{String(file.value)}</span></li>)}</ul> : <p className="muted">提案未声明文件路径参数。</p>}
      <details><summary>预览 Graph JSON</summary><pre className="result-json">{JSON.stringify(proposal.graph, null, 2)}</pre></details>
      {phase === "proposal" && <div className="action-row"><button className="primary" onClick={confirmAndRun}>确认并执行</button><button onClick={rejectProposal}>拒绝提案</button></div>}
    </div>}
    {summary && <div className="ai-message ai-summary" role="status"><h3>AI 结果解释</h3><p>{summary}</p></div>}
  </section>;
}
