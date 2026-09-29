import type { AudioInput } from "../hooks/useAudioInput";
import { templateLabels, type QuickPreset } from "../presentation";
import AudioInputPanel from "../components/AudioInputPanel";
import AgentToolsPanel from "../components/AgentToolsPanel";
import type { AgentTools } from "../hooks/useAgentTools";
import type { GraphDocument } from "../model";

export type WorkbenchSetup = {
  approach: "ai" | "template";
  preset: QuickPreset;
  output: string;
  gain: string;
};
type Props = {
  input: AudioInput;
  connected: boolean;
  desktop: boolean;
  setup: WorkbenchSetup;
  onSetup: (value: WorkbenchSetup) => void;
  onPrepare: () => void;
  onOpenSettings: () => void;
  agent: AgentTools;
  agentBlocked: boolean;
  onAgentSend: () => void;
  onAgentApply: (graph: GraphDocument) => void;
};

export default function WorkbenchPage({
  input,
  connected,
  desktop,
  setup,
  onSetup,
  onPrepare,
  onOpenSettings,
  agent,
  agentBlocked,
  onAgentSend,
  onAgentApply,
}: Props) {
  return (
    <div className="page-stack">
      {!connected && (
        <section className="welcome-card">
          <div>
            <p>请先通过右上角打开音频所在目录。</p>
          </div>
        </section>
      )}
      {setup.approach === "template" && (
        <AudioInputPanel
          input={input}
          connected={connected}
          desktop={desktop}
          locked={agentBlocked || agent.busy}
        />
      )}
      <div className="section-tabs" role="group" aria-label="准备方案的方式">
        <button
          type="button"
          aria-pressed={setup.approach === "ai"}
          className={setup.approach === "ai" ? "active" : ""}
          onClick={() => onSetup({ ...setup, approach: "ai" })}
        >
          AI 助手
        </button>
        <button
          type="button"
          aria-pressed={setup.approach === "template"}
          className={setup.approach === "template" ? "active" : ""}
          onClick={() => onSetup({ ...setup, approach: "template" })}
        >
          快捷模板
        </button>
      </div>
      {setup.approach === "ai" ? (
        <AgentToolsPanel
          agent={agent}
          blocked={agentBlocked}
          onSend={onAgentSend}
          onApplyGraph={onAgentApply}
          onSettings={onOpenSettings}
        />
      ) : (
        <section className="panel preset-panel" aria-labelledby="preset-title">
          <div className="section-heading">
            <div>
              <span className="step">方案</span>
              <h2 id="preset-title">从常用处理开始</h2>
            </div>
          </div>
          <div className="preset-cards">
            {(["denoise", "wav", "stream"] as QuickPreset[]).map((kind) => (
              <button
                type="button"
                aria-pressed={setup.preset === kind}
                className={
                  setup.preset === kind ? "preset-card selected" : "preset-card"
                }
                key={kind}
                onClick={() => onSetup({ ...setup, preset: kind })}
              >
                <strong>{templateLabels[kind]}</strong>
                <span>
                  {kind === "denoise"
                    ? "转单声道 → 48 kHz → RNNoise"
                    : kind === "wav"
                      ? "读取 WAV → 调整增益 → 写出"
                      : "逐块读取与处理，避免载入整段"}
                </span>
              </button>
            ))}
          </div>
          <div className="form-grid">
            <label>
              新的输出文件名
              <input
                value={setup.output}
                onChange={(event) =>
                  onSetup({ ...setup, output: event.target.value })
                }
                placeholder="processed.wav"
              />
            </label>
            {setup.preset !== "denoise" && (
              <label>
                增益（dB）
                <input
                  type="number"
                  min={-24}
                  max={12}
                  step={0.5}
                  value={setup.gain}
                  onChange={(event) =>
                    onSetup({ ...setup, gain: event.target.value })
                  }
                />
              </label>
            )}
          </div>
          <p className="hint">
            输入支持 PCM16 WAV。降噪与格式适配当前限 8–192 kHz
            单/双声道，输出不覆盖已有文件。
          </p>
          <div className="action-row">
            <button
              className="primary"
              disabled={
                agentBlocked ||
                agent.busy ||
                !input.path.trim() ||
                !setup.output.trim()
              }
              onClick={onPrepare}
            >
              准备 Graph 并查看
            </button>
          </div>
        </section>
      )}
    </div>
  );
}
