import { useEffect, useRef } from "react";
import DirectoryBrowser from "../components/WorkspaceFileTree";
import type { WorkspaceBrowser } from "../hooks/useWorkspaceBrowser";
import {
  AUDIO_PREVIEW_LIMIT,
  browserConfigurationKind,
  browserFileKind,
  formatBrowserBytes,
} from "../workspace-browser-model";

export type WorkspaceBrowserPageProps = {
  browser: WorkspaceBrowser;
  connected: boolean;
  onOpenGraph: (text: string, label: string) => void;
  onOpenWorkflow: (text: string, label: string) => void;
};


function FilePreview({
  browser,
  onOpenGraph,
  onOpenWorkflow,
}: Omit<WorkspaceBrowserPageProps, "connected">) {
  const entry = browser.selected;
  if (!entry)
    return (
      <section className="panel">
        {browser.activeEntry?.kind === "directory" ? <><h2>文件夹</h2><p><code>{browser.activeEntry.path}</code></p></> : <p className="muted">选择文件查看内容或加载音频。</p>}
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
              browser.mutating ||
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
                    disabled={browser.mutating}
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
      {props.browser.mutating && <p role="status">正在执行文件操作，请稍候…</p>}
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
      {props.browser.mutationDialog?.operation === "import" && <FileMutationDialog key={props.browser.mutationDialog.dropToken} browser={props.browser} />}
    </div>
  );
}

function FileMutationDialog({ browser }: { browser: WorkspaceBrowser }) {
  const dialog = browser.mutationDialog!;
  const title = "导入外部文件";
  const modal = useRef<HTMLDialogElement>(null);
  const previousFocus = useRef<HTMLElement | null>(null);
  useEffect(() => {
    previousFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    modal.current?.showModal();
    return () => { if (previousFocus.current?.isConnected) previousFocus.current.focus(); };
  }, []);
  return <dialog ref={modal} className="workspace-file-dialog" aria-labelledby="workspace-file-dialog-title" onCancel={event => { event.preventDefault(); browser.cancelMutation(); }}>
    <form onSubmit={event => { event.preventDefault(); void browser.submitMutation(""); }}>
      <h2 id="workspace-file-dialog-title">{title}</h2>
      <p>{dialog.space === "user" ? "用户工作区" : "AI 工作区"} · <code>{dialog.path || "/"}</code></p>
      <p>复制到此目录，原文件保留，不覆盖已有文件。</p><ul>{(dialog.names ?? []).map((name, i) => <li key={i}>{name}</li>)}</ul>
      {browser.error && <p className="banner error" role="alert">{browser.error}</p>}
      <div className="action-row"><button type="button" disabled={browser.mutating} onClick={browser.cancelMutation}>取消</button><button type="submit" disabled={browser.mutating}>{browser.mutating ? "处理中…" : `确认${title}`}</button></div>
    </form>
  </dialog>;
}
