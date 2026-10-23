import AgentToolsPanel from "../components/AgentToolsPanel";
import type { AgentTools } from "../hooks/useAgentTools";
import type { GraphDocument } from "../model";

type Props = {
  connected: boolean;
  onOpenSettings: () => void;
  agent: AgentTools;
  agentBlocked: boolean;
  onAgentSend: () => void;
  onAgentApply: (graph: GraphDocument) => void;
  onOpenRun?: (id: string) => void;
  onWorkflowApply?: (text: string, label: string) => void;
};

export default function WorkbenchPage({
  connected,
  onOpenSettings,
  agent,
  agentBlocked,
  onAgentSend,
  onAgentApply,
  onOpenRun,
  onWorkflowApply,
}: Props) {
  return (
    <div className="page-stack workbench-page">
      <AgentToolsPanel
        connectionHint={!connected ? "请先通过右上角打开音频所在目录。" : undefined}
        agent={agent}
        blocked={agentBlocked}
        onSend={onAgentSend}
        onApplyGraph={onAgentApply}
        onSettings={onOpenSettings}
        onOpenRun={onOpenRun}
        onApplyWorkflow={onWorkflowApply}
      />
    </div>
  );
}
