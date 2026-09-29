import type { AudioSession } from "../hooks/useAudioSession";

export default function WorkspacePanel({
  session,
  aiBusy,
  onClose,
  onChoose,
  onConnect,
  onDisconnect,
  onSettings,
}: {
  session: AudioSession;
  aiBusy: boolean;
  onClose: () => void;
  onChoose: () => void;
  onConnect: () => void;
  onDisconnect: () => void;
  onSettings: () => void;
}) {
  const locked = !!session.busy || aiBusy;
  return (
    <section
      className="panel workspace-panel"
      aria-labelledby="workspace-title"
    >
      <div className="section-heading">
        <h2 id="workspace-title">音频工作区</h2>
        <button onClick={onClose} aria-label="收起工作区设置">
          收起
        </button>
      </div>
      <p className="hint top-hint">
        选择音频所在目录。程序只在这个目录内读写文件；打开工作区不会启动音频设备。
      </p>
      <label htmlFor="workspace-path">目录路径</label>
      <div className="input-row">
        <input
          id="workspace-path"
          value={session.workspace}
          disabled={!!session.connection || locked}
          onChange={(event) => session.setWorkspace(event.target.value)}
          placeholder="选择一个已有的音频目录"
        />
        <button
          disabled={!session.desktop || !!session.connection || locked}
          onClick={onChoose}
        >
          选择目录
        </button>
        {session.connection ? (
          <button disabled={locked} onClick={onDisconnect}>
            关闭当前工作区
          </button>
        ) : (
          <button
            className="primary"
            disabled={!session.desktop || locked || !session.workspace.trim()}
            onClick={onConnect}
          >
            打开工作区
          </button>
        )}
      </div>
      <p className="hint">
        {session.connection
          ? "切换目录需先关闭当前工作区；活动任务会请求确认取消。"
          : "API配置与此目录无关。文件处理无需开启设备权限。"}
      </p>
      {aiBusy && (
        <p className="hint">请先结束当前操作，再切换工作区。</p>
      )}
      <button className="text-button" onClick={onSettings}>
        查看设备权限设置
      </button>
    </section>
  );
}
