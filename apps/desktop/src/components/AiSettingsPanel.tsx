import type { AiSettings } from "../hooks/useAiSettings";

export type AiSettingsPanelProps = {
  settings: AiSettings;
  workflowBusy: boolean;
};

export default function AiSettingsPanel({
  settings,
  workflowBusy,
}: AiSettingsPanelProps) {
  const {
    config,
    settingsPath,
    settingsLoading,
    settingsBusy,
    settingsError,
    settingsNotice,
    configStatus,
    editConfig,
    saveCurrentSettings,
    clearSavedSettings,
  } = settings;
  const locked = workflowBusy || settingsBusy;
  return (
    <section
      className="panel ai-settings-panel"
      aria-labelledby="ai-settings-title"
    >
      <div className="section-heading">
        <h2 id="ai-settings-title">模型服务设置</h2>
        <span className="badge">本机配置</span>
      </div>
      <p className="hint">
        配置 OpenAI 兼容模型服务。生成提案前会自动保存有效配置。
      </p>
      {workflowBusy && (
        <p className="hint">
          当前AI流程正在使用这份配置。结束流程或拒绝待确认方案后，再修改模型服务。
        </p>
      )}
      <div className="ai-config-grid">
        <label>
          OpenAI 兼容地址
          <input
            value={config.baseUrl}
            disabled={locked}
            placeholder="https://api.openai.com/v1"
            onChange={(event) =>
              editConfig((current) => ({
                ...current,
                baseUrl: event.target.value,
                apiKey: "",
              }))
            }
          />
        </label>
        <label>
          模型
          <input
            value={config.model}
            disabled={locked}
            placeholder="请填写模型名称"
            onChange={(event) =>
              editConfig((current) => ({
                ...current,
                model: event.target.value,
              }))
            }
          />
        </label>
        <label>
          API Key
          <div className="key-row">
            <input
              type="password"
              autoComplete="off"
              value={config.apiKey}
              disabled={locked}
              placeholder="本地 HTTP 服务可留空"
              onChange={(event) =>
                editConfig((current) => ({
                  ...current,
                  apiKey: event.target.value,
                }))
              }
            />
            <button
              type="button"
              disabled={locked || !config.apiKey}
              onClick={() =>
                editConfig((current) => ({ ...current, apiKey: "" }))
              }
            >
              清除当前输入
            </button>
          </div>
        </label>
      </div>
      <p className="hint">
        地址、模型和 API Key 保存在本机明文 JSON
        文件中，不写入仓库；需求文字、音频路径和会话不会保存。
      </p>
      <div className="action-row">
        <button
          type="button"
          disabled={locked || settingsLoading}
          onClick={() => saveCurrentSettings(workflowBusy)}
        >
          保存配置
        </button>
        <button
          type="button"
          disabled={locked || settingsLoading}
          onClick={() => void clearSavedSettings(workflowBusy)}
        >
          清除已保存配置
        </button>
        <span className="hint" role="status">
          {configStatus}
        </span>
      </div>
      {settingsPath && (
        <p className="hint">
          保存位置：<code>{settingsPath}</code>
        </p>
      )}
      {settingsError && (
        <div className="banner error ai-message" role="alert">
          <strong>本机配置提示</strong>
          <pre>{settingsError}</pre>
        </div>
      )}
      {settingsNotice && (
        <p className="hint" role="status">
          {settingsNotice}
        </p>
      )}
    </section>
  );
}
