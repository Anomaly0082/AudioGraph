import type { AiSettings } from "../hooks/useAiSettings";
import type { AudioSession } from "../hooks/useAudioSession";
import AiSettingsPanel from "../components/AiSettingsPanel";

export default function SettingsPage({
  settings,
  session,
  workflowBusy,
}: {
  settings: AiSettings;
  session: AudioSession;
  workflowBusy: boolean;
}) {
  return (
    <div className="page-stack settings-page">
      <AiSettingsPanel settings={settings} workflowBusy={workflowBusy} />
      <section className="panel">
        <h2>节点插件</h2>
        {session.connection ? (
          <>
            {session.connection.capabilities.plugin_directory && (
              <p>
                <code style={{ overflowWrap: "anywhere" }}>
                  {session.connection.capabilities.plugin_directory.replace(
                    /^\\\\\?\\/,
                    "",
                  )}
                </code>
              </p>
            )}
            <p className="hint">
              将可信插件的完整文件夹放入此目录，重启软件后生效。原生插件可能自行访问文件和网络。
            </p>
            {(session.connection.capabilities.plugins?.available ?? []).map(
              (plugin) => (
                <p key={plugin.plugin_id}>
                  {plugin.plugin_id} · {plugin.plugin_version}
                </p>
              ),
            )}
            {!session.connection.capabilities.plugins?.available.length && (
              <p className="muted">暂无可用外部插件。</p>
            )}
            {(session.connection.capabilities.plugins?.errors ?? []).map(
              (error, index) => (
                <p className="banner error" role="alert" key={index}>
                  {error.package}：{error.message}
                </p>
              ),
            )}
          </>
        ) : (
          <p className="muted">打开工作区后查看插件目录与状态。</p>
        )}
      </section>
      <section className="panel">
        <div className="section-heading">
          <h2>设备访问</h2>
          <span className="badge">只影响实时任务</span>
        </div>
        <p className="hint top-hint">
          文件处理无需设备权限。以下选项在打开工作区时生效，已连接时不能悄悄扩大权限。
        </p>
        <div className="permission-options">
          <label className="checkbox-label">
            <input
              type="checkbox"
              checked={session.allowDevices}
              disabled={!!session.connection || !!session.busy || workflowBusy}
              onChange={(event) => {
                session.setAllowDevices(event.target.checked);
                if (!event.target.checked) session.setAllowMonitor(false);
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
                !!session.connection ||
                !!session.busy ||
                workflowBusy
              }
              onChange={(event) =>
                session.setAllowMonitor(event.target.checked)
              }
            />
            允许有声输出
          </label>
        </div>
        <p className="hint">
          设备不会在打开工作区时启动。有声执行仍需要再次确认；请使用耳机、调低音量以避免啸叫。
        </p>
      </section>
    </div>
  );
}
