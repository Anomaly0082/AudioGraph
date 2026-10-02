import type { WorkspaceBrowser } from "../hooks/useWorkspaceBrowser";
import {
  AUDIO_PREVIEW_LIMIT,
  browserBreadcrumbs,
  browserConfigurationKind,
  browserFileKind,
  formatBrowserBytes,
  type BrowserSpace,
} from "../workspace-browser-model";

export type WorkspaceBrowserPageProps = {
  browser: WorkspaceBrowser;
  connected: boolean;
  onOpenGraph: (text: string, label: string) => void;
  onOpenWorkflow: (text: string, label: string) => void;
};

function DirectoryBrowser({
  browser,
  connected,
}: Pick<WorkspaceBrowserPageProps, "browser" | "connected">) {
  return (
    <section className="panel">
      <div className="section-heading">
        <h2>文件</h2>
        <button
          disabled={!connected || browser.loading}
          onClick={() => void browser.refresh()}
        >
          {browser.loading ? "刷新中…" : "刷新"}
        </button>
      </div>
      <div className="editor-toolbar">
        <label>
          工作区
          <select
            aria-label="文件工作区"
            value={browser.space}
            disabled={!connected}
            onChange={(event) =>
              browser.setSpace(event.target.value as BrowserSpace)
            }
          >
            <option value="user">用户工作区</option>
            <option value="ai">AI 工作区</option>
          </select>
        </label>
        <button disabled={!connected || !browser.path} onClick={browser.up}>
          返回上级
        </button>
      </div>
      <nav className="action-row" aria-label="文件路径">
        {browserBreadcrumbs(browser.path).map((part) => (
          <button
            key={part.path}
            disabled={!connected || part.path === browser.path}
            onClick={() => browser.goTo(part.path)}
          >
            {part.name}
          </button>
        ))}
      </nav>
      {!connected ? (
        <p className="empty-state">打开工作区后浏览本地文件。</p>
      ) : (
        <>
          {browser.entries.length ? (
            <div className="run-list" aria-label="工作区文件列表">
              {browser.entries.map((entry) => (
                <button
                  key={entry.path}
                  className={`run-row ${browser.selected?.path === entry.path ? "selected" : ""}`}
                  aria-pressed={browser.selected?.path === entry.path}
                  onClick={() => browser.select(entry)}
                >
                  <span>
                    <strong>{entry.name}</strong>
                    <small>
                      {entry.kind === "directory"
                        ? "文件夹"
                        : entry.kind === "unsupported"
                          ? "不支持的条目"
                          : "文件"}
                    </small>
                  </span>
                  <span>
                    <small>{formatBrowserBytes(entry.bytes)}</small>
                  </span>
                </button>
              ))}
            </div>
          ) : (
            <p className="empty-state">
              {browser.loading ? "正在读取…" : "此目录没有可浏览的条目。"}
            </p>
          )}
          <div className="action-row">
            <button
              disabled={browser.loading || !browser.canPreviousPage}
              onClick={() => void browser.previousPage()}
            >
              上一页
            </button>
            <button
              disabled={browser.loading || browser.listing?.next_offset == null}
              onClick={() => void browser.nextPage()}
            >
              下一页
            </button>
            {browser.listing && (
              <span className="muted">
                {browser.listing.total === null
                  ? "总条目数未知"
                  : `共 ${browser.listing.total} 个条目`}
              </span>
            )}
          </div>
          {browser.listing?.partial && (
            <p className="hint">目录扫描未完成，当前列表仅包含部分条目。</p>
          )}
          {browser.listing?.warnings.map((warning, index) => (
            <p className="hint" key={index}>
              {warning}
            </p>
          ))}
        </>
      )}
    </section>
  );
}

function FilePreview({
  browser,
  onOpenGraph,
  onOpenWorkflow,
}: Omit<WorkspaceBrowserPageProps, "connected">) {
  const entry = browser.selected;
  if (!entry)
    return (
      <section className="panel">
        <p className="muted">选择文件查看内容或加载音频。</p>
      </section>
    );
  const kind = browserFileKind(entry);
  const configuration = browserConfigurationKind(browser.preview);
  const label = `${browser.space}: ${entry.path}`;
  return (
    <section className="panel">
      <div className="section-heading">
        <h2>文件预览</h2>
      </div>
      <p>
        <code>{entry.path}</code>
      </p>
      <p className="muted">
        {formatBrowserBytes(entry.bytes)}
        {entry.modified_at_ms !== null &&
          ` · ${new Date(entry.modified_at_ms).toLocaleString()}`}
      </p>
      {entry.message && <p className="hint">{entry.message}</p>}
      {kind === "audio" ? (
        <>
          <p className="hint">
            音频按本地文件快照加载，上限 32 MiB；加载后点击播放器播放。
          </p>
          <button
            disabled={
              browser.audioLoading ||
              (entry.bytes !== null && entry.bytes > AUDIO_PREVIEW_LIMIT)
            }
            onClick={() => void browser.loadAudio()}
          >
            {browser.audioLoading ? "加载音频中…" : "加载音频"}
          </button>
          {entry.bytes !== null && entry.bytes > AUDIO_PREVIEW_LIMIT && (
            <p className="hint">文件超过 32 MiB，无法在此加载。</p>
          )}
          {browser.audioUrl && (
            <div className="action-row">
              <audio
                ref={browser.audioRef}
                src={browser.audioUrl}
                controls
                preload="none"
                aria-label="本地音频播放器"
                onError={browser.reportAudioError}
              />
            </div>
          )}
        </>
      ) : kind === "text" ? (
        <>
          {browser.previewLoading && <p role="status">正在读取文本…</p>}
          {browser.preview && (
            <>
              {browser.preview.truncated && (
                <p className="hint">
                  文本预览已截断，仅显示前 256 KiB；不能作为完整配置载入。
                </p>
              )}
              <pre
                className="code-editor"
                style={{
                  whiteSpace: "pre-wrap",
                  overflowWrap: "anywhere",
                  maxHeight: "60vh",
                  overflow: "auto",
                }}
              >
                {browser.preview.text}
              </pre>
              {configuration && (
                <div className="action-row">
                  <button
                    onClick={() => {
                      if (!browser.preview || browser.preview.truncated) return;
                      const captured = browser.capturePreview();
                      if (!browser.isCurrentPreview(captured)) return;
                      if (configuration === "graph")
                        onOpenGraph(browser.preview.text, label);
                      else onOpenWorkflow(browser.preview.text, label);
                    }}
                  >
                    载入 {configuration === "graph" ? "Graph" : "Workflow"}{" "}
                    编辑器
                  </button>
                </div>
              )}
            </>
          )}
        </>
      ) : (
        <p className="hint">此类型仅显示文件信息。</p>
      )}
    </section>
  );
}

export default function WorkspaceBrowserPage(props: WorkspaceBrowserPageProps) {
  return (
    <div className="page-stack workspace-browser">
      {props.browser.error && (
        <div className="banner error" role="alert">
          {props.browser.error}
        </div>
      )}
      <div className="run-layout">
        <DirectoryBrowser browser={props.browser} connected={props.connected} />
        <FilePreview
          browser={props.browser}
          onOpenGraph={props.onOpenGraph}
          onOpenWorkflow={props.onOpenWorkflow}
        />
      </div>
    </div>
  );
}
