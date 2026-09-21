import { useEffect, useMemo, useRef, useState } from "react";
import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import { bindRealtimeDevices, buildTaskOptions, canAdvanceTaskState, canCancel, createTemplate, formatError,
  isCurrentTaskResponse, isTerminal, parseGraph, type Mode, type TaskState, type TemplateKind } from "./model";

type NodeInfo = {
  typeId: string; displayName: string; description?: string; execution_domain: string;
  inputs: { id: string; type: string; required?: boolean }[];
  outputs: { id: string; type: string; required?: boolean }[];
  realtime_capabilities?: { format: { sample_rate: number; channels: number; sample_type: string; layout: string }; maximum_block_frames: number; supports_variable_blocks: boolean; offline_drivable: boolean };
  parameters?: { id: string; type: string; description?: string; required?: boolean;
    default?: unknown; minimum?: number; maximum?: number; unit?: string; enum?: string[] }[];
};
type Connection = { sessionId: string; workspace: string; allowDevices: boolean; allowMonitor: boolean; previousForcedDisconnect?: boolean; capabilities: { nodes: NodeInfo[] } };
type Reply = { success: boolean; data?: Record<string, any>; errors?: unknown[] };
type Device = { id: string; name: string; is_default: boolean };
type TaskView = { id: string; sessionId: string; state: TaskState | "unknown"; errors?: unknown[]; result?: unknown };
const modeLabels: Record<Mode, string> = { offline: "整段离线", streaming: "分块离线", realtime: "实时设备" };
const stateLabels: Record<string, string> = { queued: "已排队", running: "运行中", cancelling: "正在取消", succeeded: "已完成", failed: "执行失败", cancelled: "已取消", unknown: "状态未知" };
const templateLabels: Record<TemplateKind, string> = { text: "文本 · 无文件输出", wav: "WAV → Gain → WAV", stream: "分块 WAV → Gain → WAV", realtime: "实时输入 → Gain → 输出" };
const initialGraph = JSON.stringify(createTemplate("text").graph, null, 2);
function requireSuccess(reply: Reply): Record<string, any> {
  if (!reply.success) throw new Error(formatError(reply.errors ?? reply));
  return reply.data ?? {};
}

function RealtimeCapability({ node }: { node: NodeInfo }) {
  const capability = node.realtime_capabilities;
  if (!capability) return null;
  return <div className="parameter-card realtime-capability"><h4>实时约束</h4>
    <small>{capability.format.sample_rate} Hz · {capability.format.channels} 声道 · {capability.format.sample_type} / {capability.format.layout}</small>
    <small>最大块长：{capability.maximum_block_frames} 帧</small>
    <small>可变块长：{capability.supports_variable_blocks ? "支持" : "不支持"} · 离线驱动：{capability.offline_drivable ? "支持" : "不支持"}</small>
  </div>;
}

export default function App() {
  const desktop = isTauri();
  const [workspace, setWorkspace] = useState("");
  const [allowDevices, setAllowDevices] = useState(false);
  const [allowMonitor, setAllowMonitor] = useState(false);
  const [connection, setConnection] = useState<Connection | null>(null);
  const connectionRef = useRef<Connection | null>(null);
  const epochRef = useRef(0);
  const deadSessions = useRef(new Map<string, boolean>());
  const ownedSessions = useRef(new Set<string>());
  const [busy, setBusy] = useState<string | null>(null);
  const busyRef = useRef<string | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [forcedWarning, setForcedWarning] = useState("");
  const [mode, setMode] = useState<Mode>("offline");
  const [template, setTemplate] = useState<TemplateKind>("text");
  const [graphText, setGraphText] = useState(initialGraph);
  const [fileLabel, setFileLabel] = useState("未保存的 Graph");
  const [blockFrames, setBlockFrames] = useState("256");
  const [duration, setDuration] = useState("10");
  const [probe, setProbe] = useState(true);
  const [validatedKey, setValidatedKey] = useState<string | null>(null);
  const [task, setTaskState] = useState<TaskView | null>(null);
  const taskRef = useRef<TaskView | null>(null);
  const [selectedNode, setSelectedNode] = useState("");
  const [nodeSearch, setNodeSearch] = useState("");
  const [devices, setDevices] = useState<{ inputs: Device[]; outputs: Device[] }>({ inputs: [], outputs: [] });
  const [inputDevice, setInputDevice] = useState("");
  const [outputDevice, setOutputDevice] = useState("");
  const taskActive = task !== null && !isTerminal(task.state) && task.state !== "unknown";
  const locked = taskActive || busy !== null;
  const currentKey = JSON.stringify([connection?.sessionId, mode, graphText, blockFrames, duration, probe]);
  const validationCurrent = validatedKey === currentKey;
  const nodes = connection?.capabilities.nodes ?? [];
  const filteredNodes = nodes.filter((node) => `${node.typeId} ${node.displayName}`.toLowerCase().includes(nodeSearch.toLowerCase()));
  const node = nodes.find((item) => item.typeId === selectedNode);
  const localGraph = useMemo(() => {
    try { return { graph: parseGraph(graphText), error: "" }; }
    catch (reason) { return { graph: null, error: formatError(reason) }; }
  }, [graphText]);

  function setTask(value: TaskView | null) {
    const previous = taskRef.current;
    if (previous && value && previous.id === value.id && previous.sessionId === value.sessionId &&
        !canAdvanceTaskState(previous.state, value.state)) return;
    const next = previous && value && previous.id === value.id && previous.sessionId === value.sessionId &&
      isTerminal(previous.state) && previous.state === value.state
      ? { ...value, result: value.result === undefined ? previous.result : value.result,
          errors: value.errors === undefined ? previous.errors : value.errors }
      : value;
    taskRef.current = next; setTaskState(next);
  }
  function setConnected(value: Connection | null) { connectionRef.current = value; setConnection(value); }
  function editGraph(value: string) { setGraphText(value); setValidatedKey(null); }
  function disconnectState(sessionId: string, message: string) {
    if (connectionRef.current?.sessionId !== sessionId) return;
    epochRef.current++; setConnected(null); setValidatedKey(null); setDevices({ inputs: [], outputs: [] });
    const previous = taskRef.current;
    if (previous && !isTerminal(previous.state)) setTask({ ...previous, state: "unknown" });
    if (busyRef.current === "断开中") setNotice(message);
    else setError(`${message}\n连接已失效。任务可能已经开始，结果未知；不会自动重试提交。请检查输出后重新连接。`);
  }
  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<{ sessionId: string; message: string; forced?: boolean }>("backend-disconnected", (event) => {
      deadSessions.current.set(event.payload.sessionId, !!event.payload.forced);
      if (deadSessions.current.size > 32) deadSessions.current.delete(deadSessions.current.keys().next().value!);
      if (event.payload.forced && ownedSessions.current.has(event.payload.sessionId)) {
        setForcedWarning(`旧后端会话已被强制结束，未完成输出文件可能保留。${event.payload.message} 当前新会话不会自动重试旧任务。`);
      }
      disconnectState(event.payload.sessionId, event.payload.message);
    }).then((remove) => { if (disposed) remove(); else unlisten = remove; })
      .catch((reason) => { if (!disposed) setError(formatError(reason)); });
    return () => { disposed = true; unlisten?.(); };
  }, [desktop]);

  async function rpc(request: Record<string, unknown>, target = connectionRef.current): Promise<Reply> {
    if (!target) throw new Error("请先连接后端。");
    try { return await invoke<Reply>("control_request", { sessionId: target.sessionId, request }); }
    catch (reason) {
      disconnectState(target.sessionId, formatError(reason));
      throw new Error(`后端连接不可用：${formatError(reason)}。未自动重试，请检查任务和输出后重新连接。`);
    }
  }
  async function perform(label: string, action: () => Promise<void>) {
    if (busyRef.current) return;
    busyRef.current = label; setBusy(label); setError(""); setNotice("");
    try { await action(); } catch (reason) { setError(formatError(reason)); }
    finally { busyRef.current = null; setBusy(null); }
  }
  async function fetchTaskResult(id: string, target: Connection, epoch: number) {
    const data = requireSuccess(await rpc({ op: "tasks.result", task_id: id }, target));
    if (!isCurrentTaskResponse(epoch, epochRef.current, id, taskRef.current?.id ?? null)) return;
    setTask({ id, sessionId: target.sessionId, state: data.state, errors: data.errors, result: data.result });
  }
  useEffect(() => {
    if (!connection || !taskActive || !task || task.sessionId !== connection.sessionId) return;
    const target = connection, id = task.id, epoch = epochRef.current;
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const tick = async () => {
      try {
        const data = requireSuccess(await rpc({ op: "tasks.status", task_id: id }, target));
        if (disposed || !isCurrentTaskResponse(epoch, epochRef.current, id, taskRef.current?.id ?? null)) return;
        if (isTerminal(data.state)) { await fetchTaskResult(id, target, epoch); return; }
        if (!["queued", "running", "cancelling"].includes(data.state)) throw new Error("后端返回了未知任务状态。");
        setTask({ id, sessionId: target.sessionId, state: data.state, errors: data.errors });
        if (!disposed) timer = setTimeout(tick, 500);
      } catch (reason) {
        if (disposed || !isCurrentTaskResponse(epoch, epochRef.current, id, taskRef.current?.id ?? null)) return;
        setError(formatError(reason));
        const current = taskRef.current;
        if (current) setTask({ ...current, state: "unknown" });
      }
    };
    timer = setTimeout(tick, 500);
    return () => { disposed = true; if (timer) clearTimeout(timer); };
  }, [connection?.sessionId, task?.id, task?.state, taskActive]);

  const graphRequest = (op: string) => ({ op, mode, graph: parseGraph(graphText), options: buildTaskOptions(mode,
    { blockFrames: Number(blockFrames), durationSeconds: Number(duration), probe }) });
  async function chooseWorkspace() {
    const selected = await open({ directory: true, multiple: false, title: "选择音频项目工作目录" });
    if (typeof selected === "string") setWorkspace(selected);
  }
  async function connectBackend() {
    await perform("连接中", async () => {
      const epoch = ++epochRef.current;
      const value = await invoke<Connection>("connect", { workspace: workspace.trim(), allowDevices, allowMonitor });
      if (epoch !== epochRef.current || deadSessions.current.has(value.sessionId)) throw new Error("后端在连接期间已退出，请重新连接。");
      ownedSessions.current.add(value.sessionId);
      if (ownedSessions.current.size > 32) ownedSessions.current.delete(ownedSessions.current.values().next().value!);
      if (value.previousForcedDisconnect) setForcedWarning("重新连接时，旧后端因未及时退出而被强制结束。请检查可能保留的部分输出文件；没有自动重试旧任务。");
      setConnected(value); setWorkspace(value.workspace); setProbe(true); setValidatedKey(null);
      setSelectedNode(value.capabilities.nodes[0]?.typeId ?? "");
      setDevices({ inputs: [], outputs: [] }); setInputDevice(""); setOutputDevice("");
      setNotice("后端已连接。设备没有被自动打开，请先校验当前 Graph。");
    });
  }
  async function disconnectBackend() {
    await perform("断开中", async () => {
      const target = connectionRef.current;
      if (!target) return;
      if (taskActive && !await confirm("断开会请求取消当前任务。未完成文件可能保留。继续？", { title: "断开后端", kind: "warning" })) return;
      const report = await invoke<{ forced: boolean; message: string }>("disconnect", { sessionId: target.sessionId });
      if (report.forced) setForcedWarning(`旧后端会话已强制结束，可能保留部分输出文件。${report.message}`);
      if (connectionRef.current?.sessionId === target.sessionId) {
        epochRef.current++; setConnected(null); setValidatedKey(null); setDevices({ inputs: [], outputs: [] });
        const previous = taskRef.current;
        if (previous && !isTerminal(previous.state)) setTask({ ...previous, state: "unknown" });
      }
      setNotice(`${report.forced ? "已强制结束后端；未完成文件可能保留。" : "已断开后端。"} ${report.message}`);
    });
  }
  async function validateGraph() {
    await perform("校验中", async () => {
      const key = currentKey, epoch = epochRef.current;
      requireSuccess(await rpc(graphRequest("graph.validate")));
      if (epoch !== epochRef.current) return;
      setValidatedKey(key);
      setNotice("Graph 校验通过。校验不运行节点，也不验证设备在线或输入文件存在。");
    });
  }
  async function startTask() {
    await perform("提交中", async () => {
      const target = connectionRef.current;
      if (!target || taskRef.current || !validationCurrent) throw new Error("请先校验当前 Graph，并释放上一次任务记录。");
      if (mode === "realtime" && !target.allowDevices) throw new Error("当前会话未允许音频设备访问。");
      if (mode === "realtime" && !probe) {
        if (!target.allowMonitor) throw new Error("当前会话未允许有声输出。");
        if (!await confirm("这次运行会把麦克风声音送到选定输出。请佩戴耳机、调低音量，避免扬声器啸叫。确认开始？", { title: "确认有声输出", kind: "warning" })) return;
      }
      const epoch = epochRef.current;
      const data = requireSuccess(await rpc(graphRequest("tasks.start"), target));
      if (epoch !== epochRef.current) return;
      setTask({ id: data.task_id, sessionId: target.sessionId, state: data.state, errors: data.errors });
      if (isTerminal(data.state)) await fetchTaskResult(data.task_id, target, epoch);
    });
  }
  async function cancelTask() {
    await perform("请求取消", async () => {
      const current = taskRef.current, target = connectionRef.current;
      if (!current || !target || current.sessionId !== target.sessionId) return;
      const epoch = epochRef.current;
      const data = requireSuccess(await rpc({ op: "tasks.cancel", task_id: current.id }, target));
      if (!isCurrentTaskResponse(epoch, epochRef.current, current.id, taskRef.current?.id ?? null)) return;
      setTask({ ...current, state: data.state, errors: data.errors });
      if (isTerminal(data.state)) await fetchTaskResult(current.id, target, epoch);
    });
  }
  async function newTask() {
    await perform("释放记录", async () => {
      const current = taskRef.current, target = connectionRef.current;
      if (!current || taskActive) return;
      if (target && current.sessionId === target.sessionId && isTerminal(current.state)) {
        requireSuccess(await rpc({ op: "tasks.release", task_id: current.id }, target));
      }
      setTask(null);
      setNotice("记录已清除，可以手动运行新任务。输出文件未删除；已有文件仍不会被覆盖。");
    });
  }
  async function useTemplate() {
    await perform("载入模板", async () => {
      const okay = desktop ? await confirm("载入模板会替换当前编辑内容。请先保存需要保留的 Graph。继续？", { title: "载入模板", kind: "warning" }) : window.confirm("载入模板会替换当前编辑内容。继续？");
      if (!okay) return;
      const value = createTemplate(template);
      editGraph(JSON.stringify(value.graph, null, 2)); setMode(value.mode); setProbe(true); setFileLabel("未保存的 Graph");
      setNotice("模板已载入。设备不会自动选择或启动，请检查参数后校验。");
    });
  }
  async function loadGraph() {
    await perform("打开文件", async () => {
      const target = connectionRef.current;
      if (!target) return;
      const selected = await open({ multiple: false, defaultPath: target.workspace, filters: [{ name: "Graph JSON", extensions: ["json"] }] });
      if (typeof selected !== "string") return;
      const epoch = epochRef.current;
      const loaded = await invoke<{ path: string; graph: unknown }>("load_graph", { sessionId: target.sessionId, path: selected });
      const content = JSON.stringify(loaded.graph, null, 2);
      parseGraph(content);
      if (!await confirm("文件已解析。是否替换当前编辑内容？执行模式请自行核对。", { title: "打开 Graph" })) return;
      if (epoch !== epochRef.current) return;
      editGraph(content); setFileLabel(loaded.path);
    });
  }
  async function saveGraph() {
    await perform("保存文件", async () => {
      const target = connectionRef.current;
      if (!target) return;
      const graph = parseGraph(graphText), epoch = epochRef.current;
      const selected = await save({ defaultPath: `${target.workspace}/graph-new.json`, filters: [{ name: "Graph JSON", extensions: ["json"] }] });
      if (!selected) return;
      const saved = await invoke<{ path: string }>("save_graph", { sessionId: target.sessionId, path: selected, graph });
      if (epoch !== epochRef.current) return;
      setFileLabel(saved.path); setNotice("Graph 已保存为新文件；保存不会运行任务。");
    });
  }
  async function listDevices() {
    await perform("枚举设备", async () => {
      const target = connectionRef.current;
      if (!target?.allowDevices) throw new Error("请断开后显式允许设备访问。");
      const epoch = epochRef.current;
      const data = requireSuccess(await rpc({ op: "devices.list" }, target));
      if (epoch !== epochRef.current) return;
      setDevices({ inputs: data.inputs ?? [], outputs: data.outputs ?? [] }); setInputDevice(""); setOutputDevice("");
      setNotice("设备已枚举，尚未打开。选择后点击“应用设备到 Graph”才会修改图。");
    });
  }
  function applyDevices() {
    try {
      editGraph(JSON.stringify(bindRealtimeDevices(parseGraph(graphText), inputDevice, outputDevice), null, 2));
      setError(""); setNotice("仅更新实时端点的 device_id，其他节点、参数与连接保持原样。");
    } catch (reason) { setError(formatError(reason)); }
  }

  return <main className="workspace-shell">
    <header className="app-header"><div><p className="eyebrow">AUDIOPROCESS / CONTROL DESK</p><h1>音频 Graph 工作台</h1><p className="subtitle">编辑配置 · 校验能力 · 执行真实任务</p></div><div className={`connection-pill ${connection ? "connected" : ""}`}><span />{connection ? "后端已连接" : "后端未连接"}</div></header>
    {!desktop && <div className="banner preview" role="status">浏览器预览模式：可以编辑与查看模板。后端、文件和设备操作需要 Tauri 桌面应用。</div>}
    <section className="panel connection-panel" aria-labelledby="connection-title">
      <div className="section-heading"><div><span className="step">01</span><h2 id="connection-title">连接与权限</h2></div><small>连接不会自动打开音频设备</small></div>
      <label htmlFor="workspace-path">工作目录</label><div className="input-row"><input id="workspace-path" value={workspace} disabled={!!connection || locked} onChange={(event) => setWorkspace(event.target.value)} placeholder="选择或输入现有项目目录" /><button disabled={!desktop || !!connection || locked} onClick={() => perform("选择目录", chooseWorkspace)}>选择目录</button><button className="primary" disabled={!desktop || !!connection || locked || !workspace.trim()} onClick={connectBackend}>连接后端</button><button disabled={!connection || !!busy} onClick={disconnectBackend}>断开</button></div>
      <div className="permission-row"><label className="checkbox-label"><input type="checkbox" checked={allowDevices} disabled={!!connection || locked} onChange={(event) => { setAllowDevices(event.target.checked); if (!event.target.checked) setAllowMonitor(false); }} />允许音频设备访问</label><label className="checkbox-label"><input type="checkbox" checked={allowMonitor} disabled={!allowDevices || !!connection || locked} onChange={(event) => setAllowMonitor(event.target.checked)} />允许有声输出</label><span className="muted">权限在连接时固定；修改需先断开。</span></div>
    </section>
    {error && <div className="banner error" role="alert"><strong>操作未完成</strong><pre>{error}</pre><button onClick={() => setError("")} aria-label="关闭错误提示">关闭</button></div>}
    {notice && <div className="banner info" role="status">{notice}</div>}
    {forcedWarning && <div className="banner warning" role="alert">{forcedWarning} <button onClick={() => setForcedWarning("")}>我已知晓</button></div>}
    <div className="workbench-grid">
      <section className="panel editor-panel" aria-labelledby="editor-title">
        <div className="section-heading"><div><span className="step">02</span><h2 id="editor-title">Graph 配置</h2></div><span className={`badge ${validationCurrent ? "good" : ""}`}>{validationCurrent ? "校验通过" : "待校验"}</span></div>
        <div className="editor-toolbar"><label>模板<select aria-label="Graph 模板" value={template} disabled={locked} onChange={(event) => setTemplate(event.target.value as TemplateKind)}>{Object.entries(templateLabels).map(([key, value]) => <option key={key} value={key}>{value}</option>)}</select></label><button disabled={locked} onClick={useTemplate}>载入模板</button><button disabled={!desktop || !connection || locked} onClick={loadGraph}>打开 JSON</button><button disabled={!desktop || !connection || locked} onClick={saveGraph}>另存为新文件</button></div>
        <div className="file-caption" title={fileLabel}>{fileLabel}</div><label className="sr-only" htmlFor="graph-editor">Graph JSON 编辑器</label><textarea id="graph-editor" className="code-editor" spellCheck={false} value={graphText} disabled={locked} onChange={(event) => editGraph(event.target.value)} />
        <div className="editor-footer"><span>{localGraph.graph ? `${localGraph.graph.nodes.length} 个节点 · ${localGraph.graph.connections.length} 条连接` : "JSON 待修正"}</span><span>Graph v1 / UTF-8</span></div>
        {localGraph.error && <p className="inline-error">{localGraph.error}</p>}<p className="hint">相对路径基于当前工作目录，不是 JSON 所在目录。仅支持目录内文件；输出和保存均不覆盖已有文件。</p>
        <div className="execution-settings"><label>执行模式<select aria-label="执行模式" value={mode} disabled={locked} onChange={(event) => { setMode(event.target.value as Mode); setValidatedKey(null); }}>{Object.entries(modeLabels).map(([key, value]) => <option value={key} key={key}>{value}</option>)}</select></label>{mode !== "offline" && <label>每块帧数<input aria-label="每块帧数" type="number" min={1} max={65536} value={blockFrames} disabled={locked} onChange={(event) => { setBlockFrames(event.target.value); setValidatedKey(null); }} /></label>}{mode === "realtime" && <label>持续秒数<input aria-label="持续秒数" type="number" min={1} max={3600} value={duration} disabled={locked} onChange={(event) => { setDuration(event.target.value); setValidatedKey(null); }} /></label>}</div>
        {mode === "realtime" && <div className="realtime-options"><label className="checkbox-label"><input type="checkbox" checked={probe} disabled={locked || !connection?.allowMonitor} onChange={(event) => { setProbe(event.target.checked); setValidatedKey(null); }} />静音探测（Probe）</label><p className="hint">探测会采集并处理，但输出始终为零。执行需要设备权限。</p>{!probe && <p className="inline-warning">有声输出已选中。运行前需确认；请使用耳机并调低音量，避免啸叫。</p>}</div>}
        <div className="action-row"><button disabled={!connection || locked} onClick={validateGraph}>校验 Graph</button><button className="primary" disabled={!connection || locked || !!task || !validationCurrent || !!localGraph.error || (mode === "realtime" && !connection.allowDevices)} onClick={startTask}>{busy === "提交中" ? "提交中…" : "开始任务"}</button><span className="hint">{task ? "请先释放当前记录，再开始新任务。" : "配置变更后需重新校验。"}</span></div>
      </section>
      <aside className="side-column">
        <section className="panel capabilities-panel" aria-labelledby="nodes-title"><div className="section-heading"><div><span className="step">03</span><h2 id="nodes-title">节点能力</h2></div><span className="badge">{nodes.length} 个</span></div><input aria-label="搜索节点" placeholder="搜索节点类型" value={nodeSearch} onChange={(event) => setNodeSearch(event.target.value)} disabled={!connection} /><div className="node-picker">{filteredNodes.map((item) => <button key={item.typeId} className={selectedNode === item.typeId ? "selected" : ""} onClick={() => setSelectedNode(item.typeId)}><span>{item.displayName}</span><code>{item.typeId}</code></button>)}</div>
          {!connection && <p className="empty-state">连接后从真实 NodeRegistry 获取节点、端口和参数约束。</p>}
          {node && <div className="node-detail"><h3>{node.displayName}</h3><code>{node.typeId}</code><span className="badge">{node.execution_domain}</span><p>{node.description}</p><RealtimeCapability node={node} /><h4>端口</h4><ul className="port-list">{node.inputs.map((port) => <li key={`in-${port.id}`}><span>输入</span><code>{port.id}</code><b>{port.type}</b>{port.required && <small>必需</small>}</li>)}{node.outputs.map((port) => <li key={`out-${port.id}`}><span>输出</span><code>{port.id}</code><b>{port.type}</b></li>)}</ul><h4>参数</h4>{!node.parameters?.length && <p className="hint">无配置参数。</p>}{node.parameters?.map((parameter) => <div className="parameter-card" key={parameter.id}><div><code>{parameter.id}</code><span>{parameter.type}{parameter.required ? " · 必填" : ""}</span></div><p>{parameter.description}</p>{parameter.default !== undefined && <small>默认值：{JSON.stringify(parameter.default)}</small>}{(parameter.minimum !== undefined || parameter.maximum !== undefined) && <small>范围：{parameter.minimum ?? "不限"} ～ {parameter.maximum ?? "不限"} {parameter.unit ?? ""}</small>}{parameter.enum && <small>可选：{parameter.enum.join(" / ")}</small>}</div>)}</div>}
        </section>
        {mode === "realtime" && <section className="panel devices-panel"><div className="section-heading"><h2>设备绑定</h2><button disabled={!connection?.allowDevices || locked} onClick={listDevices}>枚举设备</button></div><p className="hint">只枚举，不打开；不会自动选设备或改写 Graph。</p><label>输入设备<select value={inputDevice} disabled={locked} onChange={(event) => setInputDevice(event.target.value)}><option value="">请选择输入端点</option>{devices.inputs.map((item) => <option key={item.id} value={item.id}>{item.name}{item.is_default ? "（系统默认）" : ""}</option>)}</select></label><label>输出设备<select value={outputDevice} disabled={locked} onChange={(event) => setOutputDevice(event.target.value)}><option value="">请选择输出端点</option>{devices.outputs.map((item) => <option key={item.id} value={item.id}>{item.name}{item.is_default ? "（系统默认）" : ""}</option>)}</select></label><button disabled={locked || !inputDevice || !outputDevice} onClick={applyDevices}>应用设备到 Graph</button></section>}
      </aside>
    </div>
    <section className="panel task-panel" aria-labelledby="task-title"><div className="section-heading"><div><span className="step">04</span><h2 id="task-title">任务与结果</h2></div>{task && <span className={`badge ${task.state === "succeeded" ? "good" : task.state === "failed" || task.state === "unknown" ? "bad" : ""}`}>{stateLabels[task.state] ?? task.state}</span>}</div>
      {!task ? <p className="empty-state">尚未提交任务。默认文本模板不访问设备、不创建文件，适合检查完整调用流程。</p> : <><div className="task-summary"><code>{task.id}</code><span>{stateLabels[task.state] ?? task.state}</span><button className="danger" disabled={!connection || !!busy || !canCancel(task.state) || task.sessionId !== connection.sessionId} onClick={cancelTask}>{task.state === "cancelling" ? "等待取消完成…" : "取消任务"}</button><button disabled={!!busy || taskActive} onClick={newTask}>{task.sessionId === connection?.sessionId && isTerminal(task.state) ? "释放记录 / 新任务" : "清除显示记录 / 新任务"}</button></div><p className="hint">释放只移除内存记录，不删除音频或文本文件。取消可能留下部分输出。</p>{task.state === "unknown" && <div className="banner warning">执行情况未知，请先检查文件与设备状态，再连接并手动建立新任务。</div>}{task.errors?.length ? <pre className="task-errors" role="alert">{formatError(task.errors)}</pre> : null}{task.result !== undefined ? <pre className="result-json" aria-label="任务结果 JSON">{JSON.stringify(task.result, null, 2)}</pre> : <p className="muted">{taskActive ? "每 500 ms 查询状态；取消不会阻塞界面。" : "本任务没有可用结果负载。"}</p>}</>}
    </section>
    <footer>AudioProcess · P6 最小控制界面 <span>{busy ?? "音频处理与任务状态由 C++ 后端提供"}</span></footer>
  </main>;
}
