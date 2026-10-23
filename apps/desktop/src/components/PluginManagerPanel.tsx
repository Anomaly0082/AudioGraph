import { Puzzle, FolderPlus, RefreshCw, Trash2 } from "lucide-react";
import { workspaceDisplayPath } from "../recent-workspaces";
import type { PluginManager } from "../hooks/usePluginManager";

type StartupError = { package: string; code: string; message: string };

export default function PluginManagerPanel({
  manager,
  desktop,
  startupErrors = [],
}: {
  manager: PluginManager;
  desktop: boolean;
  startupErrors?: StartupError[];
}) {
  const view = manager.view;
  return (
    <section className="panel" aria-label="插件管理">
      <div className="section-heading">
        <h2><Puzzle className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />本机插件</h2>
        {view?.restart_required && <span className="badge">重启后生效</span>}
      </div>
      <p className="hint top-hint">
        安装、启停和卸载只影响下次启动；当前 Graph 和任务继续使用本次启动的节点。
      </p>
      <div className="action-row">
        <button
          type="button"
          disabled={!desktop || manager.busy}
          onClick={() => void manager.install()}
        >
          <FolderPlus className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
          安装本机文件夹…
        </button>
        <button
          type="button"
          disabled={!desktop || manager.busy || manager.loading}
          onClick={() => void manager.refresh()}
        >
          <RefreshCw className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
          刷新列表
        </button>
      </div>
      {!desktop && <p className="muted">请在桌面程序中管理插件。</p>}
      {desktop && manager.loading && <p role="status">正在读取插件列表…</p>}
      {manager.busy && <p role="status">正在处理插件操作…</p>}
      {manager.error && <p className="banner error" role="alert">{manager.error}</p>}
      {manager.notice && <p className="banner info" role="status">{manager.notice}</p>}
      {view && (
        <>
          <p className="hint">
            安装目录：<code style={{ overflowWrap: "anywhere" }}>{workspaceDisplayPath(view.directory)}</code>
          </p>
          {view.entries.length === 0 && <p className="muted">尚未安装插件。</p>}
          {view.entries.map((entry) => (
            <div className="panel plugin-entry" key={entry.folder}>
              <div className="section-heading">
                <h3>{entry.plugin_id ?? entry.folder}</h3>
                {entry.plugin_version && <span className="badge">{entry.plugin_version}</span>}
              </div>
              <p className="hint">
                本次启动：{entry.active ? "已选用" : "未选用"} · 下次启动：
                {entry.pending_uninstall ? "待卸载" : entry.enabled ? "启用" : "停用"}
              </p>
              {entry.node_types.length > 0 && (
                <p className="hint">节点：{entry.node_types.join("、")}</p>
              )}
              {entry.error && <p className="banner error" role="alert">{entry.error}</p>}
              {entry.can_manage && entry.pending_uninstall && (
                <div className="action-row">
                  <button
                    type="button"
                    disabled={!desktop || manager.busy}
                    onClick={() => void manager.cancelUninstall(entry.folder, entry.revision)}
                  >
                    取消卸载
                  </button>
                </div>
              )}
              {entry.can_manage && !entry.pending_uninstall && (
                <div className="action-row">
                  <button
                    type="button"
                    disabled={!desktop || manager.busy}
                    onClick={() => void manager.setEnabled(entry.folder, entry.revision, !entry.enabled)}
                  >
                    {entry.enabled ? "下次启动停用" : "下次启动启用"}
                  </button>
                  <button
                    type="button"
                    className="danger"
                    disabled={!desktop || manager.busy}
                    onClick={() => void manager.uninstall(entry.folder, entry.revision)}
                  >
                    <Trash2 className="ui-icon" aria-hidden="true" size={16} strokeWidth={1.65} />
                    卸载并保留备份
                  </button>
                </div>
              )}
            </div>
          ))}
          {view.warnings.map((warning, index) => (
            <p className="banner warning" role="alert" key={`${index}-${warning}`}>{warning}</p>
          ))}
        </>
      )}
      {startupErrors.length > 0 && (
        <div>
          <h3>本次启动的发现错误</h3>
          {startupErrors.map((error, index) => (
            <p className="banner error" role="alert" key={`${error.package}-${index}`}>
              {error.package}：{error.message}
            </p>
          ))}
        </div>
      )}
    </section>
  );
}
