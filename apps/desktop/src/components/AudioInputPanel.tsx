import type { AudioInput } from "../hooks/useAudioInput";

export default function AudioInputPanel({
  input,
  connected,
  desktop,
  locked,
}: {
  input: AudioInput;
  connected: boolean;
  desktop: boolean;
  locked: boolean;
}) {
  const info = input.inspection;
  return (
    <section className="panel input-panel" aria-labelledby="input-title">
      <div className="section-heading">
        <div>
          <span className="step">输入</span>
          <h2 id="input-title">选择要处理的音频</h2>
        </div>
        <span className="badge">PCM16 WAV</span>
      </div>
      <label htmlFor="audio-input-path">工作区内的文件路径</label>
      <div className="input-row">
        <input
          id="audio-input-path"
          value={input.path}
          disabled={locked}
          placeholder="例如 input.wav，也可选择文件"
          onChange={(event) => input.setPath(event.target.value)}
        />
        <button
          type="button"
          disabled={!desktop || !connected || locked}
          onClick={() => void input.choose()}
        >
          选择文件
        </button>
        <button
          type="button"
          disabled={!connected || locked || input.busy || !input.path.trim()}
          onClick={() => void input.check()}
        >
          {input.busy ? "正在检查…" : "检查音频"}
        </button>
      </div>
      {!connected && (
        <p className="hint">
          先在右上角打开音频工作区。选择文件不会扩大目录访问范围。
        </p>
      )}
      {input.error && (
        <p className="inline-error" role="alert">
          {input.error}
        </p>
      )}
      {info ? (
        <>
          <dl className="audio-facts">
            <div>
              <dt>采样率</dt>
              <dd>{info.sample_rate.toLocaleString()} Hz</dd>
            </div>
            <div>
              <dt>声道</dt>
              <dd>
                {info.channels === 1
                  ? "单声道"
                  : info.channels === 2
                    ? "双声道"
                    : `${info.channels} 声道`}
              </dd>
            </div>
            <div>
              <dt>时长</dt>
              <dd>{info.duration_seconds.toFixed(2)} 秒</dd>
            </div>
          </dl>
          <p className="hint">
            {info.channels > 2 ||
            info.sample_rate < 8000 ||
            info.sample_rate > 192000
              ? "当前格式适配节点不支持这份音频的声道数或采样率。"
              : info.sample_rate === 48000 && info.channels === 1
                ? "格式可直接用于 RNNoise 语音降噪。"
                : "降噪模板会显式转换为 48 kHz 单声道。"}
          </p>
        </>
      ) : (
        <p className="hint">
          检查只读取格式与时长，不上传音频。AI生成方案前也会重新检查已填写的输入。
        </p>
      )}
    </section>
  );
}
