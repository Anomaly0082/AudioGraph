import type { AgentTools } from "../hooks/useAgentTools";
import type { GraphDocument } from "../model";
import Disclosure from "./Disclosure";
import ConversationTurnView from "./ConversationTurnView";
import { Plus, Send, Settings2, Square } from "lucide-react";

function conversationLabel(title: string): string {
  const label = title === "New conversation" ? "新会话" : title;
  const characters = Array.from(label);
  return characters.length > 42 ? `${characters.slice(0, 28).join("")}…${characters.slice(-10).join("")}` : label;
}
export default function AgentToolsPanel({
  agent,
  blocked,
  onSend,
  onApplyGraph,
  onSettings,
  onOpenRun,
  onApplyWorkflow,
  connectionHint,
}: {
  agent: AgentTools;
  blocked: boolean;
  onSend: () => void;
  onApplyGraph: (graph: GraphDocument) => void;
  onSettings: () => void;
  onOpenRun?: (id: string) => void;
  onApplyWorkflow?: (text: string, label: string) => void;
  connectionHint?: string;
}) {
  return (
    <section
      className="panel agent-tools-panel"
      aria-labelledby="agent-tools-title"
    >
      <h2 id="agent-tools-title" className="agent-visually-hidden">工具助手</h2>
      <div className="agent-toolbar">
        <label>
          <span className="agent-visually-hidden">模式</span>
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
          <span className="agent-visually-hidden">会话</span>
          <select
            aria-label="选择AI会话"
            title={agent.conversations?.find(item => item.id === agent.conversationId)?.title}
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
              <option key={item.id} value={item.id} title={item.title}>
                {conversationLabel(item.title)}
              </option>
            ))}
          </select>
        </label>
        <div className="agent-toolbar-actions">
        <button
          disabled={agent.busy || agent.loading || !agent.spaces}
          onClick={() => void agent.newConversation()}
        >
          <Plus className="ui-icon" size={16} strokeWidth={1.65} aria-hidden="true" />新建会话
        </button>
        <button className="small" onClick={onSettings}><Settings2 className="ui-icon" size={16} strokeWidth={1.65} aria-hidden="true" />模型设置</button>
        </div>
      </div>
      <Disclosure label="工作区与可用工具" className="agent-spaces">
      <p className="hint agent-mode-hint">
        {agent.mode === "graph"
          ? "可查询与编辑配置，不提供运行工具。"
          : "可在 AI 工作区运行 Graph 和 Workflow。"}
      </p>
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
      <p className="hint agent-privacy-hint">
        查询到的历史配置、文本和工具结果会发送给模型。内置音频检查不上传音频；第三方插件可能联网处理音频。每轮最多16次模型请求、40次工具调用。
      </p>
      </Disclosure>
      <div className="agent-conversation" aria-live="polite" aria-label="对话消息">
      {connectionHint && <p className="hint">{connectionHint}</p>}
      {agent.hasOlder && (
        <button
          disabled={agent.busy || agent.loading}
          onClick={() => void agent.loadOlder()}
        >
          查看更早消息
        </button>
      )}
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
      {(agent.error || !!agent.warnings?.length) && <div className="agent-alerts">
        {agent.error && <p className="inline-error" role="alert">{agent.error}</p>}
        {(agent.warnings ?? []).map((warning, index) => <p key={index} className="inline-error" role="status">{warning}</p>)}
      </div>}
      <div className="agent-composer">
        <label className="agent-prompt-label" htmlFor="agent-prompt">
          <span className="agent-visually-hidden">消息</span>
          <textarea
            id="agent-prompt"
            disabled={agent.busy}
            value={agent.prompt}
            onChange={(event) => agent.setPrompt(event.target.value)}
            placeholder={
              agent.mode === "graph"
                ? "描述要修改的 Graph"
                : "描述要执行的处理任务"
            }
          />
        </label>
        <div className="action-row agent-composer-actions">
          <label className="checkbox-label">
            <input type="checkbox" checked={agent.attachGraph} disabled={agent.busy} onChange={(event) => agent.setAttachGraph(event.target.checked)} />
            附带当前 Graph 草稿
          </label>
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
            <Send className="ui-icon" size={16} strokeWidth={1.65} aria-hidden="true" />发送
          </button>
          {agent.canStop && (
            <button
              className="danger"
              disabled={agent.stopping}
              onClick={() => void agent.stop()}
            >
              <Square className="ui-icon" size={16} strokeWidth={1.65} aria-hidden="true" />{agent.stopping ? "正在停止" : "停止"}
            </button>
          )}
        </div>
      </div>
    </section>
  );
}
