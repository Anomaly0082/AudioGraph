import type { AiSettings } from "../hooks/useAiSettings";
import type { AudioSession } from "../hooks/useAudioSession";
import type { PluginManager } from "../hooks/usePluginManager";
import AiSettingsPanel from "../components/AiSettingsPanel";
import PluginManagerPanel from "../components/PluginManagerPanel";

export default function SettingsPage({
  settings,
  session,
  plugins,
  workflowBusy,
  onPermissions,
}: {
  settings: AiSettings;
  session: AudioSession;
  plugins: PluginManager;
  workflowBusy: boolean;
  onPermissions: (desired: { allowDevices: boolean; allowMonitor: boolean }) => void;
}) {
  return (
    <div className="page-stack settings-page">
      <AiSettingsPanel settings={settings} workflowBusy={workflowBusy} />
      <PluginManagerPanel
        manager={plugins}
        desktop={session.desktop}
        startupErrors={session.connection?.capabilities.plugins?.errors ?? []}
      />
      <section className="panel">
        <div className="section-heading">
          <h2>设备访问</h2>
          <span className="badge">只影响实时任务</span>
        </div>
        <p className="hint top-hint">
          文件处理无需设备权限。已连接时更改权限会重新连接当前工作区。
        </p>
        <div className="permission-options">
          <label className="checkbox-label">
            <input
              type="checkbox"
              checked={session.allowDevices}
              disabled={!!session.busy || workflowBusy}
              onChange={(event) => {
                onPermissions({ allowDevices: event.target.checked, allowMonitor: session.allowMonitor });
              }}
            />
            允许音频设备访问
          </label>
          <label className="checkbox-label">
            <input
              type="checkbox"
              checked={session.allowMonitor}
              disabled={
                !session.allowDevices ||
                !!session.busy ||
                workflowBusy
              }
              onChange={(event) =>
                onPermissions({ allowDevices: session.allowDevices, allowMonitor: event.target.checked })
              }
            />
            允许有声输出
          </label>
        </div>
        {session.busy === "更新设备权限中" && <p className="hint" role="status">正在更新权限并重新连接…</p>}
        {workflowBusy && <p className="hint">请先停止或等待 AI、Workflow 和当前编辑操作完成，再更改权限。</p>}
        <p className="hint">
          设备不会在打开工作区时启动。有声执行仍需要再次确认；请使用耳机、调低音量以避免啸叫。
        </p>
      </section>
    </div>
  );
}
