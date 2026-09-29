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
      <section className="panel">
        <h2>数据保存范围</h2>
        <p className="hint">
          API配置保存到本机用户目录。草稿、AI提案和当前任务在切页时保留，但本版不会在退出后恢复；需要保留Graph时请在编辑器另存。音频不会被上传给模型服务。
        </p>
      </section>
    </div>
  );
}
