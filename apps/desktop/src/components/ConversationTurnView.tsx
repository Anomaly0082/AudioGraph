import type { ConversationTurn } from "../conversation-model";
import type { AgentMode } from "../types/agent";
import type { GraphDocument } from "../model";
import { graphFromToolEvent, workflowFromToolEvent } from "../agent-model";
import Disclosure from "./Disclosure";
import AssistantMarkdown from "./AssistantMarkdown";

const labels: Record<string, string> = {
  completed: "已结束",
  cancelled: "已停止",
  limited: "达到上限",
  failed: "失败",
  interrupted: "已中断",
  running: "处理中",
};
export default function ConversationTurnView({
  turn,
  mode,
  busy,
  blocked,
  stopping,
  onApplyGraph,
  onOpenRun,
  onApplyWorkflow,
}: {
  turn: ConversationTurn;
  mode: AgentMode;
  busy: boolean;
  blocked: boolean;
  stopping: boolean;
  onApplyGraph: (graph: GraphDocument) => void;
  onOpenRun?: (id: string) => void;
  onApplyWorkflow?: (text: string, label: string) => void;
}) {
  const events = turn.reply?.events ?? turn.events ?? [];
  const state = turn.state ?? turn.reply?.state ?? "running";
  return (
    <article className="agent-turn">
      <div className="agent-user">
        <small>你</small>
        <p>{turn.prompt}</p>
      </div>
      {state === "interrupted" && (
        <p className="inline-error" role="status">
          上次调用已中断，部分工具可能已经执行。请先核对运行记录，不会自动重试。
        </p>
      )}
      {turn.error && turn.reply && (
        <p className="inline-error" role="alert">
          {turn.error}
        </p>
      )}
      {turn.reply ? (
        <div className="agent-answer">
          <div className="row agent-answer-heading">
            <strong>AI</strong>
            <span className="badge">{labels[state] ?? state}</span>
          </div>
          <AssistantMarkdown
            text={turn.reply.text || "本轮没有文字回复，请查看调用详情。"}
          />
          <small>
            {turn.reply.model_calls} 次模型请求 · {turn.reply.tool_calls}{" "}
            次工具调用
          </small>
          {!!turn.reply.omitted_text_bytes && (
            <p className="hint">回复过长，保存的内容已缩短。</p>
          )}
        </div>
      ) : (
        state !== "interrupted" && (
          <p className={turn.error ? "inline-error" : "hint"}>
            {turn.error ||
              (stopping ? "正在停止并等待工具清理…" : "正在调用模型或工具…")}
          </p>
        )
      )}
      {events.some(
        (event) => event.kind === "tool" && event.success === false,
      ) && (
        <p className="inline-error" role="status">
          本轮有工具调用失败，可查看调用详情。
        </p>
      )}
      {events.map((event, index) => {
        const graph = graphFromToolEvent(event);
        const workflow = workflowFromToolEvent(event);
        if (workflow && onApplyWorkflow)
          return (
            <div className="agent-graph-action" key={index}>
              <code>{workflow.label}</code>
              <button
                disabled={busy || blocked}
                onClick={() => onApplyWorkflow(workflow.text, workflow.label)}
              >
                载入 Workflow 编辑器
              </button>
            </div>
          );
        if (!graph) return null;
        const path = (event.arguments as { path?: unknown })?.path;
        return (
          <div className="agent-graph-action" key={index}>
            {typeof path === "string" && <code>{path}</code>}
            <button
              disabled={busy || blocked}
              onClick={() => onApplyGraph(graph)}
            >
              载入 Graph 编辑器
            </button>
          </div>
        );
      })}
      {!!events.length && (
        <Disclosure label="调用详情" className="agent-call-details">
          {events.map((event, index) => (
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
                  <pre>{JSON.stringify(event.arguments, null, 2)}</pre>
                </>
              )}
              {event.result !== undefined && (
                <>
                  <small>执行结果</small>
                  <pre>{JSON.stringify(event.result, null, 2)}</pre>
                </>
              )}
              {!!event.omitted_bytes && (
                <p className="hint">
                  这项详情过大，未完整保存；请按需查询关联运行记录。
                </p>
              )}
            </Disclosure>
          ))}
        </Disclosure>
      )}
      {!!turn.pending_tools?.length && (
        <Disclosure label="未确认的工具调用">
          <p className="hint">
            以下调用没有保存到完成回执，不能据此认定成功或失败。
          </p>
          <pre>{JSON.stringify(turn.pending_tools, null, 2)}</pre>
        </Disclosure>
      )}
      {!!turn.run_ids?.length && onOpenRun && (
        <Disclosure label="关联运行记录">
          <div className="action-row">
            {turn.run_ids.map((id) => (
              <button key={id} onClick={() => onOpenRun(id)}>
                运行 {id.slice(0, 8)}
              </button>
            ))}
          </div>
        </Disclosure>
      )}
    </article>
  );
}
