import type { AgentTools } from "../hooks/useAgentTools";
import type { GraphDocument } from "../model";
import Disclosure from "./Disclosure";
import ConversationTurnView from "./ConversationTurnView";
export default function AgentToolsPanel({
  agent,
  blocked,
  onSend,
  onApplyGraph,
  onSettings,
  onOpenRun,
  onApplyWorkflow,
}: {
  agent: AgentTools;
  blocked: boolean;
  onSend: () => void;
  onApplyGraph: (graph: GraphDocument) => void;
  onSettings: () => void;
  onOpenRun?: (id: string) => void;
  onApplyWorkflow?: (text: string, label: string) => void;
}) {
  return (
    <section
      className="panel agent-tools-panel"
      aria-labelledby="agent-tools-title"
    >
      <div className="section-heading">
        <h2 id="agent-tools-title">工具助手</h2>
        <button className="small" onClick={onSettings}>
          模型设置
        </button>
      </div>
      <div className="agent-toolbar">
        <label>
          模式
          <select
            value={agent.mode}
            disabled={agent.busy || agent.loading}
            onChange={(event) =>
              agent.setMode(event.target.value as "graph" | "workflow")
            }
          >
            <option value="graph">Graph · 编辑与校验</option>
            <option value="workflow">Workflow · 工具执行</option>
          </select>
        </label>
        <label className="conversation-picker">
          会话
          <select
            aria-label="选择AI会话"
            value={agent.conversationId ?? ""}
            disabled={agent.busy || agent.loading || !agent.spaces}
            onChange={(event) =>
              void agent.selectConversation(event.target.value)
            }
          >
            {!agent.conversationId && (
              <option value="">新会话（发送后保存）</option>
            )}
            {(agent.conversations ?? []).map((item) => (
              <option key={item.id} value={item.id}>
                {item.mode === "graph" ? "Graph" : "Workflow"} ·{" "}
                {item.title === "New conversation" ? "新会话" : item.title}
              </option>
            ))}
          </select>
        </label>
        <button
          disabled={agent.busy || agent.loading || !agent.spaces}
          onClick={() => void agent.newConversation()}
        >
          新建会话
        </button>
      </div>
      <p className="hint">
        {agent.mode === "graph"
          ? "可查询与编辑配置，不提供运行工具。"
          : "可在 AI 工作区运行 Graph 和 Workflow。"}
      </p>
      <Disclosure label="工作区与可用工具" className="agent-spaces">
        {agent.spaces ? (
          <>
            <dl>
              <dt>用户工作区 · 只读 / 新建交付</dt>
              <dd>{agent.spaces.user_root}</dd>
              <dt>AI 工作区 · 可增删改</dt>
              <dd>{agent.spaces.ai_root}</dd>
            </dl>
            <p className="hint">{agent.spaces.tools.join(" · ")}</p>
          </>
        ) : (
          <p className="hint">
            {agent.loading ? "正在准备工作区…" : "打开工作区后可使用。"}
          </p>
        )}
      </Disclosure>
      {agent.error && (
        <p className="inline-error" role="alert">
          {agent.error}
        </p>
      )}
      {(agent.warnings ?? []).map((warning, index) => (
        <p key={index} className="inline-error" role="status">
          {warning}
        </p>
      ))}
      {agent.hasOlder && (
        <button
          disabled={agent.busy || agent.loading}
          onClick={() => void agent.loadOlder()}
        >
          查看更早消息
        </button>
      )}
      <div className="agent-conversation" aria-live="polite">
        {agent.turns.map((turn) => (
          <ConversationTurnView
            key={turn.id}
            turn={turn}
            mode={agent.mode}
            busy={agent.busy}
            blocked={blocked}
            stopping={agent.stopping}
            onApplyGraph={onApplyGraph}
            onOpenRun={onOpenRun}
            onApplyWorkflow={onApplyWorkflow}
          />
        ))}
      </div>
      <label className="checkbox-label">
        <input
          type="checkbox"
          checked={agent.attachGraph}
          disabled={agent.busy}
          onChange={(event) => agent.setAttachGraph(event.target.checked)}
        />
        附带当前 Graph 草稿
      </label>
      <label className="agent-prompt-label" htmlFor="agent-prompt">
        消息
        <textarea
          id="agent-prompt"
          disabled={agent.busy}
          value={agent.prompt}
          onChange={(event) => agent.setPrompt(event.target.value)}
          placeholder={
            agent.mode === "graph"
              ? "例如：查看节点列表，把当前 Graph 的增益改成 -6 dB，保存到你的工作区。"
              : "例如：把用户工作区的 input.wav 复制到你的工作区，运行增益处理，然后将结果以新文件交付。"
          }
        />
      </label>
      <div className="action-row">
        <button
          className="primary"
          disabled={
            agent.busy ||
            blocked ||
            agent.loading ||
            !agent.spaces ||
            !agent.prompt.trim()
          }
          onClick={onSend}
        >
          发送
        </button>
        {agent.canStop && (
          <button
            className="danger"
            disabled={agent.stopping}
            onClick={() => void agent.stop()}
          >
            {agent.stopping ? "正在停止" : "停止"}
          </button>
        )}
      </div>
      <p className="hint">
        查询到的历史配置、文本和工具结果会发送给模型。内置音频检查不上传音频；第三方插件可能联网处理音频。每轮最多16次模型请求、40次工具调用。
      </p>
    </section>
  );
}
