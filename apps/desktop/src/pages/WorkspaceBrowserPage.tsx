import { useEffect, useRef, useState } from "react";
import DirectoryBrowser from "../components/WorkspaceFileTree";
import type { WorkspaceBrowser } from "../hooks/useWorkspaceBrowser";
import {
  trustedAudioPreviewUrl,
  browserConfigurationKind,
  browserFileKind,
  formatBrowserBytes,
  audioOutputFormats,
  audioConversionSampleRates,
  validatedAudioConversionOptions,
  replaceConvertedAudioExtension,
  type AudioOutputFormat,
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
            加载后点击播放器播放。
          </p>
          <div className="action-row audio-preview-actions">
          <button
            disabled={
              browser.audioLoading ||
              browser.mutating
            }
            onClick={() => void browser.loadAudio()}
          >
            {browser.audioLoading ? "加载音频中…" : "加载音频"}
          </button>
          <button disabled={!browser.filesEnabled || browser.mutating} onClick={() => browser.beginConversion(entry)}>格式转换</button>
          </div>
          {browser.audioUrl && trustedAudioPreviewUrl(browser.audioUrl) && (
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
      {props.browser.conversionDialog && <AudioConversionDialog key={`${props.browser.conversionDialog.identity}:${props.browser.conversionDialog.space}:${props.browser.conversionDialog.source.path}`} browser={props.browser} />}
    </div>
  );
}

function AudioConversionDialog({ browser }: { browser: WorkspaceBrowser }) {
  const conversion = browser.conversionDialog!;
  const [name, setName] = useState(conversion.outputName);
  const [outputFormat, setOutputFormat] = useState<AudioOutputFormat>(conversion.outputFormat);
  const [sampleRate, setSampleRate] = useState<number | undefined>(conversion.options?.sampleRate);
  const [channels, setChannels] = useState<number | undefined>(conversion.options?.channels);
  const [optionsExpanded, setOptionsExpanded] = useState(!!(conversion.options?.sampleRate || conversion.options?.channels));
  let optionError = "";
  try { validatedAudioConversionOptions(outputFormat, { sampleRate, channels }); } catch (error) { optionError = error instanceof Error ? error.message : String(error); }
  const modal = useRef<HTMLDialogElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const previousFocus = useRef<HTMLElement | null>(null);
  const composing = useRef(false);
  const running = !!browser.conversionState;
  const cancelling = browser.conversionState?.phase === "cancelling";
  useEffect(() => {
    previousFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    modal.current?.showModal(); input.current?.focus(); input.current?.select();
    return () => { if (previousFocus.current?.isConnected) previousFocus.current.focus(); };
  }, []);
  return <dialog ref={modal} className="workspace-file-dialog" aria-labelledby="audio-conversion-title" onCancel={event => { event.preventDefault(); if (!running) void browser.cancelConversion(); }}>
    <form onSubmit={event => { event.preventDefault(); if (!composing.current && !optionError) void browser.submitConversion(name, outputFormat, { sampleRate, channels }); }}>
      <h2 id="audio-conversion-title">格式转换</h2>
      <p>{conversion.space === "user" ? "用户工作区" : "AI 工作区"} · <code>{conversion.source.path}</code></p>
      <label>输出格式<select aria-label="输出格式" value={outputFormat} disabled={running} onChange={event => { const format = event.target.value as AudioOutputFormat; setOutputFormat(format); setName(current => replaceConvertedAudioExtension(current, format)); }}>{audioOutputFormats.map(format => <option key={format} value={format}>{format === "m4a" ? "M4A (AAC)" : format.toUpperCase()}</option>)}</select></label>
      <label>新输出文件名<input ref={input} aria-label="新输出文件名" value={name} disabled={running} required autoComplete="off" onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }} onChange={event => setName(event.target.value)} onKeyDown={event => { if (event.key === "Enter" && (composing.current || event.nativeEvent.isComposing || event.keyCode === 229)) event.preventDefault(); }} /></label>
      <button type="button" aria-expanded={optionsExpanded} aria-controls="audio-conversion-options" disabled={running} onClick={() => setOptionsExpanded(current => !current)}>转换选项</button>
      {optionsExpanded && <div id="audio-conversion-options">
        <label>采样率<select aria-label="采样率" value={sampleRate ?? ""} disabled={running} onChange={event => setSampleRate(event.target.value === "" ? undefined : Number(event.target.value))}><option value="">保持原样</option>{audioConversionSampleRates.map(rate => <option key={rate} value={rate}>{rate} Hz</option>)}</select></label>
        <label>声道<select aria-label="声道" value={channels ?? ""} disabled={running} onChange={event => setChannels(event.target.value === "" ? undefined : Number(event.target.value))}><option value="">保持原样</option><option value="1">单声道</option><option value="2">双声道</option></select></label>
      </div>}
      <p className="hint">源文件保留，输出到同一目录，不覆盖已有文件。</p>
      {(outputFormat === "mp3" || outputFormat === "m4a") && <p className="hint">MP3/M4A 为有损编码。</p>}
      {running && <p role="status">{cancelling ? "正在取消转换…" : "正在转换…"}</p>}
      {browser.error && <p className="banner error" role="alert">{browser.error}</p>}
      {optionError && <p className="banner error" role="alert">{optionError}</p>}
      <div className="action-row"><button type="button" disabled={cancelling} onClick={() => void browser.cancelConversion()}>{cancelling ? "正在取消…" : "取消"}</button><button type="submit" disabled={running || !!optionError}>开始转换</button></div>
    </form>
  </dialog>;
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
