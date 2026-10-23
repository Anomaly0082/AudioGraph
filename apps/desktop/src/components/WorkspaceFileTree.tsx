import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { ChevronLeft, ChevronRight, ClipboardPaste, FolderPlus, LoaderCircle, RefreshCw } from "lucide-react";
import WorkspaceEntryIcon from "./WorkspaceEntryIcon";
import type { WorkspaceBrowser } from "../hooks/useWorkspaceBrowser";
import { browserBreadcrumbs, browserFileKind, formatBrowserBytes, parentBrowserPath, type BrowserEntry, type BrowserSpace } from "../workspace-browser-model";

// Delay only the visual indicator, never the request or its result. Fast reads
// do not flash a temporary row; unmounting cancels the pending indicator.
function DelayedTreeSpinner({ fallback = null }: { fallback?: ReactNode }) {
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    const timer = window.setTimeout(() => setVisible(true), 250);
    return () => window.clearTimeout(timer);
  }, []);
  return visible ? <LoaderCircle className="workspace-loading-icon" size={14} strokeWidth={1.8} aria-hidden="true" focusable="false" /> : fallback;
}

export default function WorkspaceFileTree({ browser: b, connected }: { browser: WorkspaceBrowser; connected: boolean }) {
  const tree = useRef<HTMLDivElement>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; entry: BrowserEntry | null } | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const disabled = !connected || !b.filesEnabled || b.mutating;
  const active = b.activeEntry;
  const rows: BrowserEntry[] = [];
  function collect(entries: BrowserEntry[]) { for (const entry of entries) { rows.push(entry); if (b.branches?.[entry.path]?.expanded) collect(b.branches[entry.path].listing?.entries ?? []); } }
  collect(b.entries);
  useEffect(() => { setMenu(null); }, [b.space, b.path, b.listing, b.mutating]);
  useEffect(() => {
    if (!menu) return;
    menuRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
    const close = (event: Event) => { if (!menuRef.current?.contains(event.target as Node)) setMenu(null); };
    document.addEventListener("pointerdown", close);
    const scroll = () => setMenu(null);
    window.addEventListener("resize", scroll);
    window.addEventListener("wheel", scroll, true);
    return () => { document.removeEventListener("pointerdown", close); window.removeEventListener("resize", scroll); window.removeEventListener("wheel", scroll, true); };
  }, [menu]);
  function showMenu(x: number, y: number, entry: BrowserEntry | null) {
    if (b.mutating) return;
    if (entry) b.focusEntry(entry); else b.chooseRoot();
    setMenu({ x: Math.max(8, Math.min(x, innerWidth - 190)), y: Math.max(8, Math.min(y, innerHeight - 290)), entry });
  }
  function focusRow(entry: BrowserEntry) { b.focusEntry(entry); tree.current?.querySelector<HTMLButtonElement>(`[data-path="${CSS.escape(entry.path)}"]`)?.focus(); }
  function keyboard(event: KeyboardEvent<HTMLDivElement>) {
    if (event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement || (event.target as HTMLElement).isContentEditable || b.mutating) return;
    const index = rows.findIndex(entry => entry.path === active?.path);
    const ctrl = event.ctrlKey || event.metaKey;
    let handled = true;
    if (event.key === "ArrowDown" && rows.length) focusRow(rows[Math.min(index + 1, rows.length - 1)]);
    else if (event.key === "ArrowUp" && rows.length) focusRow(rows[Math.max(index - 1, 0)]);
    else if (event.key === "ArrowRight" && active?.kind === "directory") { if (!b.branches[active.path]?.expanded) void b.toggleDirectory(active); else { const child = b.branches[active.path].listing?.entries[0]; if (child) focusRow(child); } }
    else if (event.key === "ArrowLeft" && active) { if (active.kind === "directory" && b.branches[active.path]?.expanded) void b.toggleDirectory(active); else { const parent = rows.find(entry => entry.path === parentBrowserPath(active.path)); if (parent) focusRow(parent); } }
    else if (event.key === "F2" && active) b.beginMutation("rename", active);
    else if (event.key === "Delete" && active) { if (!event.repeat) b.beginMutation("trash", active); }
    else if (ctrl && event.key.toLowerCase() === "c" && active) b.copyEntry(active);
    else if (ctrl && event.key.toLowerCase() === "x" && active) b.copyEntry(active, true);
    else if (ctrl && event.key.toLowerCase() === "v") void b.paste();
    else if ((event.shiftKey && event.key === "F10") || event.key === "ContextMenu") { const r = (event.target as HTMLElement).getBoundingClientRect(); showMenu(r.left + 24, r.bottom, active ?? null); }
    else handled = false;
    if (handled) event.preventDefault();
  }
  const inline = b.mutationDialog && ["mkdir", "rename"].includes(b.mutationDialog.operation) ? b.mutationDialog : null;
  const hadInline = useRef(false);
  useEffect(() => {
    if (hadInline.current && !inline) {
      const row = active ? tree.current?.querySelector<HTMLButtonElement>(`[data-path="${CSS.escape(active.path)}"]`) : null;
      (row ?? tree.current)?.focus();
    }
    hadInline.current = !!inline;
  }, [inline, active]);
  function newFolder(parent: string, depth: number) { return inline?.operation === "mkdir" && inline.path === parent ? <InlineName key={`new:${parent}`} browser={b} depth={depth} /> : null; }
  function renderEntries(entries: BrowserEntry[], depth: number): React.ReactNode { return entries.map(entry => {
    const branch = b.branches?.[entry.path];
    const editing = inline?.operation === "rename" && inline.path === entry.path;
    return <div role="none" key={entry.path}>
      {editing ? <InlineName browser={b} depth={depth} /> : <button type="button" role="treeitem" data-path={entry.path} aria-label={entry.name} aria-level={depth + 1} aria-busy={branch?.loading || undefined} aria-selected={active?.path === entry.path} aria-expanded={entry.kind === "directory" ? !!branch?.expanded : undefined} tabIndex={active?.path === entry.path || (!active && rows[0]?.path === entry.path) ? 0 : -1} disabled={b.mutating} className={`workspace-tree-row ${active?.path === entry.path ? "selected" : ""} ${b.clipboard?.operation === "move" && b.clipboard.entry.path === entry.path ? "cut" : ""}`} style={{ paddingLeft: 10 + depth * 18 }} onClick={() => b.select(entry)} onContextMenu={event => { event.preventDefault(); event.stopPropagation(); showMenu(event.clientX, event.clientY, entry); }}>
        <span className={`workspace-tree-twistie ${branch?.expanded ? "expanded" : ""}`} aria-hidden="true">{entry.kind === "directory" && (branch?.loading ? <DelayedTreeSpinner fallback={<ChevronRight size={14} strokeWidth={1.8} focusable="false" />} /> : <ChevronRight size={14} strokeWidth={1.8} focusable="false" />)}</span>
        <WorkspaceEntryIcon entry={entry} expanded={!!branch?.expanded} />
        <span className="workspace-tree-name" title={entry.name}>{entry.name}</span><small className="workspace-tree-size">{entry.kind === "directory" ? "" : formatBrowserBytes(entry.bytes)}</small>
      </button>}
      {entry.kind === "directory" && branch?.expanded && <div role="group" aria-busy={branch.loading || undefined}>{newFolder(entry.path, depth + 1)}{renderEntries(branch.listing?.entries ?? [], depth + 1)}{branch.listing?.next_offset != null && <button className="workspace-tree-more" disabled={branch.loading || b.mutating} onClick={() => void b.toggleDirectory(entry, true)}>加载更多 {entry.name}</button>}{branch.listing?.warnings.map((warning, index) => <p className="hint" key={index}>{warning}</p>)}</div>}
    </div>;
  }); }
  const action = (fn: () => void) => { setMenu(null); tree.current?.focus(); fn(); };
  return <section className={`panel workspace-files-panel ${b.dropActive ? "workspace-files-drop-active" : ""}`}>
    <div className="workspace-files-toolbar">
      <h2>文件</h2>
      <select aria-label="文件工作区" value={b.space} disabled={!connected || b.mutating} onChange={event => b.setSpace(event.target.value as BrowserSpace)}><option value="user">用户工作区</option><option value="ai">AI 工作区</option></select>
      <div className="workspace-files-actions">
        <button aria-label="新建文件夹" title="新建文件夹" disabled={disabled} onClick={() => { if (active?.kind === "directory" && !b.branches[active.path]?.expanded) void b.toggleDirectory(active); b.beginMutation("mkdir"); }}><FolderPlus size={16} strokeWidth={1.65} aria-hidden="true" /></button>
        <button aria-label="刷新" title="刷新" disabled={!connected || b.loading || b.mutating} onClick={() => void b.refresh()}><RefreshCw size={16} strokeWidth={1.65} aria-hidden="true" /></button>
        <button aria-label="粘贴" title="粘贴" disabled={disabled || !b.clipboard} onClick={() => void b.paste()}><ClipboardPaste size={16} strokeWidth={1.65} aria-hidden="true" /></button>
      </div>
    </div>
    {b.path && <nav className="workspace-files-breadcrumbs" aria-label="文件路径">{browserBreadcrumbs(b.path).map(part => <button key={part.path} disabled={b.mutating || part.path === b.path} onClick={() => b.goTo(part.path)}>{part.name}</button>)}</nav>}
    {!connected ? <p className="empty-state">打开工作区后浏览本地文件。</p> : <>
      <div ref={tree} className="workspace-file-tree" role="tree" aria-busy={b.loading || undefined} tabIndex={0} aria-label="工作区文件列表" onKeyDown={keyboard} onContextMenu={event => { event.preventDefault(); showMenu(event.clientX, event.clientY, null); }} onClick={event => { if (event.target === event.currentTarget) b.chooseRoot(); }}>
        {newFolder(b.path, 0)}{renderEntries(b.entries, 0)}{!b.entries.length && !inline && (b.loading ? <DelayedTreeSpinner /> : <p className="empty-state">空目录；右键新建文件夹或粘贴。</p>)}
      </div>
      {b.listing && <footer className="workspace-files-footer"><small>{b.listing.total === null ? "总条目数未知" : `${b.listing.total} 个条目`}</small>{(b.canPreviousPage || b.listing.next_offset != null) && <div className="workspace-files-pagination"><button aria-label="上一页" title="上一页" disabled={b.loading || b.mutating || !b.canPreviousPage} onClick={() => void b.previousPage()}><ChevronLeft size={15} strokeWidth={1.8} aria-hidden="true" /></button><button aria-label="下一页" title="下一页" disabled={b.loading || b.mutating || b.listing.next_offset == null} onClick={() => void b.nextPage()}><ChevronRight size={15} strokeWidth={1.8} aria-hidden="true" /></button></div>}</footer>}
      {b.listing?.partial && <p className="hint">目录扫描未完成，当前列表仅包含部分条目。</p>}{b.listing?.warnings.map((warning, index) => <p className="hint" key={index}>{warning}</p>)}
    </>}
    {menu && createPortal(<div ref={menuRef} className="workspace-file-context-menu" role="menu" aria-label="文件操作" style={{ left: menu.x, top: menu.y }} onKeyDown={event => { if (event.key === "Escape") { setMenu(null); tree.current?.focus(); } if (event.key === "Tab") setMenu(null); if (event.key === "ArrowDown" || event.key === "ArrowUp") { event.preventDefault(); const items = [...menuRef.current!.querySelectorAll<HTMLButtonElement>("button:not(:disabled)")]; const index = items.indexOf(document.activeElement as HTMLButtonElement); items[(index + (event.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus(); } }}>
      <button role="menuitem" disabled={disabled} onClick={() => action(() => { if (menu.entry?.kind === "directory" && !b.branches[menu.entry.path]?.expanded) void b.toggleDirectory(menu.entry); b.beginMutation("mkdir"); })}>新建文件夹</button>
      {menu.entry && <><button role="menuitem" disabled={disabled || menu.entry.kind === "unsupported"} onClick={() => action(() => b.copyEntry(menu.entry!))}>复制 <kbd>Ctrl+C</kbd></button><button role="menuitem" disabled={disabled || menu.entry.kind === "unsupported"} onClick={() => action(() => b.copyEntry(menu.entry!, true))}>剪切 <kbd>Ctrl+X</kbd></button></>}
      <button role="menuitem" disabled={disabled || !b.clipboard} onClick={() => action(() => void b.paste())}>粘贴 <kbd>Ctrl+V</kbd></button>
      {menu.entry && browserFileKind(menu.entry) === "audio" && <button role="menuitem" disabled={disabled} onClick={() => action(() => b.beginConversion(menu.entry!))}>格式转换</button>}
      {menu.entry && <><button role="menuitem" disabled={disabled || menu.entry.kind === "unsupported"} onClick={() => action(() => b.beginMutation("rename", menu.entry!))}>重命名 <kbd>F2</kbd></button><button role="menuitem" disabled={disabled || menu.entry.kind === "unsupported"} onClick={() => action(() => b.beginMutation("trash", menu.entry!))}>移到回收站 <kbd>Delete</kbd></button></>}
    </div>, document.body)}
  </section>;
}

function InlineName({ browser, depth }: { browser: WorkspaceBrowser; depth: number }) {
  const dialog = browser.mutationDialog!;
  const [name, setName] = useState(dialog.target);
  const input = useRef<HTMLInputElement>(null);
  const composing = useRef(false);
  useEffect(() => { input.current?.focus(); input.current?.select(); }, []);
  return <form className="workspace-inline-name" style={{ paddingLeft: 52 + depth * 18 }} onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) browser.cancelMutation(); }} onSubmit={event => { event.preventDefault(); if (!composing.current) void browser.submitMutation(name); }}><input ref={input} aria-label={dialog.operation === "mkdir" ? "新文件夹名称" : "重命名名称"} value={name} disabled={browser.mutating} onCompositionStart={() => { composing.current = true; }} onCompositionEnd={() => { composing.current = false; }} onChange={event => setName(event.target.value)} onKeyDown={event => { if (event.key === "Enter" && (composing.current || event.nativeEvent.isComposing || event.keyCode === 229)) { event.preventDefault(); return; } if (event.key === "Escape") { event.preventDefault(); browser.cancelMutation(); } }} required autoComplete="off" /><button type="submit" disabled={browser.mutating}>确认</button><button type="button" disabled={browser.mutating} onClick={browser.cancelMutation}>取消</button>{dialog.operation === "rename" && <small>路径引用不会自动更新。</small>}</form>;
}
