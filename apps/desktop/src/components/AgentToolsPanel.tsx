import { graphFromToolEvent } from "../agent-model";
import type { AgentTools } from "../hooks/useAgentTools";
import type { GraphDocument } from "../model";
import Disclosure from "./Disclosure";
import AssistantMarkdown from "./AssistantMarkdown";

const stateLabels = {
  completed: "已结束",
  cancelled: "已停止",
  limited: "达到上限",
  failed: "失败",
};
export default function AgentToolsPanel({
  agent,
  blocked,
  onSend,
  onApplyGraph,
  onSettings,
}: {
  agent: AgentTools;
  blocked: boolean;
  onSend: () => void;
  onApplyGraph: (graph: GraphDocument) => void;
  onSettings: () => void;
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
            disabled={agent.busy}
            onChange={(event) =>
              agent.setMode(event.target.value as "graph" | "workflow")
            }
          >
            <option value="graph">Graph · 编辑与校验</option>
            <option value="workflow">Workflow · 工具执行</option>
          </select>
        </label>
        <button
          disabled={agent.busy || blocked || !agent.spaces}
          onClick={() => void agent.reset()}
        >
          清空对话
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
      <div className="agent-conversation" aria-live="polite">
        {agent.turns.map((turn) => (
          <article className="agent-turn" key={turn.id}>
            <div className="agent-user">
              <small>你</small>
              <p>{turn.prompt}</p>
            </div>
            {turn.reply ? (
              <div className="agent-answer">
                <div className="row agent-answer-heading">
                  <strong>AI</strong>
                  <span className="badge">{stateLabels[turn.reply.state]}</span>
                </div>
                <AssistantMarkdown
                  text={turn.reply.text || "本轮没有文字回复，请查看工具结果。"}
                />
                <small>
                  {turn.reply.model_calls} 次模型请求 · {turn.reply.tool_calls}{" "}
                  次工具调用
                </small>
                {turn.reply.events.some(
                  (event) => event.kind === "tool" && event.success === false,
                ) && (
                  <p className="inline-error" role="status">
                    本轮有工具调用失败，可查看调用详情。
                  </p>
                )}
                {agent.mode === "graph" &&
                  turn.reply.events.map((event, index) => {
                    const graph = graphFromToolEvent(event);
                    if (!graph) return null;
                    const path = (event.arguments as { path?: unknown })?.path;
                    return (
                      <div className="agent-graph-action" key={index}>
                        {typeof path === "string" && <code>{path}</code>}
                        <button
                          disabled={agent.busy || blocked}
                          onClick={() => onApplyGraph(graph)}
                        >
                          载入 Graph 编辑器
                        </button>
                      </div>
                    );
                  })}
                {turn.reply.events.length > 0 && (
                  <Disclosure label="调用详情" className="agent-call-details">
                    {turn.reply.events.map((event, index) => (
                      <Disclosure
                        key={index}
                        className="agent-event"
                        label={
                          event.kind === "tool"
                            ? `${event.tool} · ${event.success ? "成功" : "失败"}`
                            : event.kind === "input"
                              ? "本轮需求与配置"
                              : event.kind === "assistant"
                                ? "模型输出"
                                : "状态"
                        }
                      >
                        {event.text && <pre>{event.text}</pre>}
                        {event.arguments !== undefined && (
                          <>
                            <small>工具参数</small>
                            <pre>
                              {JSON.stringify(event.arguments, null, 2)}
                            </pre>
                          </>
                        )}
                        {event.result !== undefined && (
                          <>
                            <small>执行结果</small>
                            <pre>{JSON.stringify(event.result, null, 2)}</pre>
                          </>
                        )}
                      </Disclosure>
                    ))}
                  </Disclosure>
                )}
              </div>
            ) : (
              <p className={turn.error ? "inline-error" : "hint"}>
                {turn.error ||
                  (agent.stopping
                    ? "正在停止并等待工具清理…"
                    : "正在调用模型或工具…")}
              </p>
            )}
          </article>
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
        读取的文本和工具结果会进入模型上下文；不发送音频样本。每轮最多8次模型请求、20次工具调用。
      </p>
    </section>
  );
}
