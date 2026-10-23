import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { desktopAvailable, invokeDesktop, listenWorkspaceFileDrop } from "../api/desktop";
import { formatError } from "../model";
import type { Connection } from "../types/desktop";
import {
  AudioPreviewOwner,
  AUDIO_PREVIEW_LIMIT,
  BROWSER_PAGE_SIZE,
  audioMime,
  browserFileKind,
  browserRelativePath,
  browserMutationTarget,
  browserEntryName,
  browserPathContains,
  matchesBrowserSnapshot,
  parentBrowserPath,
  type BrowserEntry,
  type BrowserListing,
  type BrowserSnapshot,
  type BrowserSpace,
  type BrowserTextPreview,
  type BrowserMutationOperation,
} from "../workspace-browser-model";

export function useWorkspaceBrowser(
  connection: Connection | null,
  visible: boolean,
  options: { enabled?: boolean; blockedReason?: () => string | null; protectedPaths?: () => { space: BrowserSpace; path: string }[] } = {},
) {
  const optionsRef = useRef(options);
  optionsRef.current = options;
  const enabled = options.enabled ?? desktopAvailable();
  const [mutating, setMutating] = useState(false);
  const mutatingRef = useRef(false);
  const [dropActive, setDropActive] = useState(false);
  const [activeEntry, setActiveEntry] = useState<BrowserEntry | null>(null);
  const destinationRef = useRef("");
  const [destination, setDestination] = useState("");
  type TreeBranch = { listing: BrowserListing | null; expanded: boolean; loading: boolean };
  const [branches, setBranches] = useState<Record<string, TreeBranch>>({});
  const branchesRef = useRef<Record<string, TreeBranch>>({});
  const treeEpoch = useRef(0);
  type Clipboard = { entry: BrowserEntry; operation: "copy" | "move"; identity: string; space: BrowserSpace };
  const [clipboard, setClipboard] = useState<Clipboard | null>(null);
  const clipboardRef = useRef<Clipboard | null>(null);
  function updateClipboard(value: Clipboard | null) { clipboardRef.current = value; setClipboard(value); }
  function resetTree() { ++treeEpoch.current; branchesRef.current = {}; setBranches({}); }
  function chooseDestination(value: string) { destinationRef.current = value; setDestination(value); }
  type MutationDialog = { operation: BrowserMutationOperation; path: string; space: BrowserSpace; sessionId: string; identity: string; directory: string; target: string; names?: string[]; dropToken?: string };
  const [mutationDialog, setMutationDialog] = useState<MutationDialog | null>(null);
  const mutationDialogRef = useRef<MutationDialog | null>(null);
  function showMutation(value: MutationDialog | null) { mutationDialogRef.current = value; setMutationDialog(value); }
  const [space, setSpaceState] = useState<BrowserSpace>("user");
  const [path, setPath] = useState("");
  const [listing, setListing] = useState<BrowserListing | null>(null);
  const [selected, setSelected] = useState<BrowserEntry | null>(null);
  const [preview, setPreview] = useState<BrowserTextPreview | null>(null);
  const [audioUrl, setAudioUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [audioLoading, setAudioLoading] = useState(false);
  const [error, setError] = useState("");
  const ownerRef = useRef<AudioPreviewOwner | null>(null);
  if (!ownerRef.current) ownerRef.current = new AudioPreviewOwner();
  const owner = ownerRef.current;
  const mounted = useRef(true);
  const currentConnection = useRef(connection);
  currentConnection.current = connection;
  const visibleRef = useRef(visible);
  visibleRef.current = visible;
  const spaceRef = useRef<BrowserSpace>("user");
  const pathRef = useRef("");
  const selectedRef = useRef<BrowserEntry | null>(null);
  const previewRef = useRef<BrowserTextPreview | null>(null);
  const selectionEpoch = useRef(0);
  const listEpoch = useRef(0);
  const audioEpoch = useRef(0);
  const audioLoadingRef = useRef(false);
  const pageOffsets = useRef<number[]>([]);
  const listingRef = useRef<BrowserListing | null>(null);
  const identity = JSON.stringify([
    connection?.sessionId ?? null,
    connection?.workspace ?? null,
  ]);
  const scopeRef = useRef({ identity, epoch: 0 });
  if (scopeRef.current.identity !== identity) {
    scopeRef.current = { identity, epoch: scopeRef.current.epoch + 1 };
    ++selectionEpoch.current;
    ++listEpoch.current;
    ++audioEpoch.current;
  }

  function captureSelection(): BrowserSnapshot | null {
    const session = currentConnection.current;
    const file = selectedRef.current;
    if (!session || !file) return null;
    return Object.freeze({
      sessionId: session.sessionId,
      workspace: session.workspace,
      space: spaceRef.current,
      path: file.path,
      epoch: selectionEpoch.current,
    });
  }
  function current(expected: BrowserSnapshot | null): boolean {
    return (
      mounted.current &&
      !!expected &&
      matchesBrowserSnapshot(expected, captureSelection(), visibleRef.current)
    );
  }
  function clearAudio() {
    ++audioEpoch.current;
    audioLoadingRef.current = false;
    owner.clear();
    if (mounted.current) {
      setAudioUrl(null);
      setAudioLoading(false);
    }
  }
  function clearSelection() {
    ++selectionEpoch.current;
    selectedRef.current = null;
    previewRef.current = null;
    setSelected(null);
    setPreview(null);
    setPreviewLoading(false);
    clearAudio();
  }
  const audioRef = useCallback((player: HTMLAudioElement | null) => {
    ownerRef.current?.attach(player);
  }, []);

  useLayoutEffect(() => {
    showMutation(null);
    setDropActive(false);
    mounted.current = true;
    return () => {
      mounted.current = false;
      ++audioEpoch.current;
      ++selectionEpoch.current;
      ++listEpoch.current;
      owner.clear();
    };
  }, []);
  useLayoutEffect(() => {
    resetTree();
    updateClipboard(null);
    setActiveEntry(null);
    chooseDestination("");
    showMutation(null);
    setDropActive(false);
    clearSelection();
    pathRef.current = "";
    setPath("");
    listingRef.current = null;
    setListing(null);
    pageOffsets.current = [];
    setLoading(false);
    setError("");
  }, [identity]);
  useLayoutEffect(() => {
    if (!visible) {
      resetTree();
      setActiveEntry(null);
      chooseDestination(pathRef.current);
      showMutation(null);
      setDropActive(false);
      ++listEpoch.current;
      clearSelection();
      setLoading(false);
    }
  }, [visible]);

  async function fetchPage(offset: number): Promise<boolean> {
    const session = currentConnection.current;
    if (!session || !visibleRef.current) return false;
    const requestSpace = spaceRef.current;
    const requestPath = pathRef.current;
    const scope = scopeRef.current;
    const token = ++listEpoch.current;
    const isCurrent = () =>
      mounted.current &&
      visibleRef.current &&
      token === listEpoch.current &&
      scope === scopeRef.current &&
      session.sessionId === currentConnection.current?.sessionId &&
      requestSpace === spaceRef.current &&
      requestPath === pathRef.current;
    setLoading(true);
    setError("");
    try {
      const reply = await invokeDesktop<BrowserListing>(
        "workspace_browser_list",
        {
          sessionId: session.sessionId,
          space: requestSpace,
          path: requestPath,
          offset,
          limit: BROWSER_PAGE_SIZE,
        },
      );
      if (!isCurrent()) return false;
      listingRef.current = reply;
      setListing(reply);
      return true;
    } catch (reason) {
      if (isCurrent()) setError(formatError(reason));
      return false;
    } finally {
      if (isCurrent()) setLoading(false);
    }
  }
  async function refresh(): Promise<boolean> {
    if (mutatingRef.current) return false;
    showMutation(null);
    resetTree();
    setActiveEntry(null);
    chooseDestination(pathRef.current);
    clearSelection();
    pageOffsets.current = [];
    return fetchPage(0);
  }
  function goTo(value: string) {
    if (mutatingRef.current) return;
    showMutation(null);
    setDropActive(false);
    try {
      const next = browserRelativePath(value);
      resetTree();
      setActiveEntry(null);
      chooseDestination(next);
      clearSelection();
      pathRef.current = next;
      setPath(next);
      listingRef.current = null;
      setListing(null);
      pageOffsets.current = [];
      void fetchPage(0);
    } catch (reason) {
      setError(formatError(reason));
    }
  }
  function setSpace(value: BrowserSpace) {
    if (mutatingRef.current) return;
    if (value !== "user" && value !== "ai") return;
    if (spaceRef.current === value) return;
    spaceRef.current = value;
    updateClipboard(null);
    setSpaceState(value);
    goTo("");
  }
  function enter(entry: BrowserEntry) {
    if (entry.kind === "directory") void toggleDirectory(entry);
  }
  async function toggleDirectory(entry: BrowserEntry, more = false, restoring = false) {
    if ((mutatingRef.current && !restoring) || entry.kind !== "directory") return;
    const branch = branchesRef.current[entry.path];
    if (branch?.loading) return;
    if ((more || !branch?.listing) && (Object.keys(branchesRef.current).length >= 100 || Object.values(branchesRef.current).reduce((sum, item) => sum + (item.listing?.entries.length ?? 0), 0) >= 2000)) { setError("目录树加载已达上限，请刷新后继续浏览。"); return; }
    const expanded = more || !branch?.expanded;
    const update = (value: TreeBranch) => { branchesRef.current = { ...branchesRef.current, [entry.path]: value }; setBranches(branchesRef.current); };
    if (!expanded || (branch?.listing && !more)) { update({ listing: branch?.listing ?? null, expanded, loading: false }); return; }
    const session = currentConnection.current;
    if (!session || !visibleRef.current) return;
    const scope = scopeRef.current, epoch = treeEpoch.current, requestSpace = spaceRef.current;
    const valid = () => mounted.current && visibleRef.current && scope === scopeRef.current && epoch === treeEpoch.current && requestSpace === spaceRef.current;
    update({ listing: branch?.listing ?? null, expanded: true, loading: true });
    try {
      const reply = await invokeDesktop<BrowserListing>("workspace_browser_list", { sessionId: session.sessionId, space: requestSpace, path: entry.path, offset: more ? branch?.listing?.next_offset ?? 0 : 0, limit: BROWSER_PAGE_SIZE });
      if (valid()) {
        const entries = more ? [...(branch?.listing?.entries ?? []), ...reply.entries] : reply.entries;
        const otherCount = Object.entries(branchesRef.current).reduce((sum, [key, item]) => sum + (key === entry.path ? 0 : item.listing?.entries.length ?? 0), 0);
        if (otherCount + entries.length > 2000) { update({ listing: branch?.listing ?? null, expanded: true, loading: false }); setError("目录树加载已达上限，请刷新后继续浏览。"); return; }
        update({ listing: { ...reply, entries }, expanded: true, loading: false });
      }
    } catch (reason) { if (valid()) { update({ listing: branch?.listing ?? null, expanded: true, loading: false }); setError(formatError(reason)); } }
  }
  async function restoreTreeContext(directory: string, focusPath?: string) {
    const scope = scopeRef.current, root = pathRef.current;
    const parts = directory.split("/").filter(Boolean);
    for (let i = 1; i <= parts.length; ++i) {
      const folder = parts.slice(0, i).join("/");
      if (folder === root || (root && !folder.startsWith(`${root}/`))) continue;
      if (scope !== scopeRef.current || !visibleRef.current) return;
      await toggleDirectory({ name: parts[i - 1], path: folder, kind: "directory", bytes: null, modified_at_ms: null }, false, true);
    }
    if (scope !== scopeRef.current || !visibleRef.current) return;
    chooseDestination(directory);
    if (focusPath) {
      const entries = [...(listingRef.current?.entries ?? []), ...Object.values(branchesRef.current).flatMap(branch => branch.listing?.entries ?? [])];
      const entry = entries.find(entry => entry.path === focusPath) ?? null;
      setActiveEntry(entry);
      if (entry) chooseDestination(entry.kind === "directory" ? entry.path : parentBrowserPath(entry.path));
    }
  }
  function up() {
    goTo(parentBrowserPath(pathRef.current));
  }
  async function nextPage() {
    if (mutatingRef.current) return;
    const currentPage = listingRef.current;
    if (!currentPage || currentPage.next_offset === null || loading) return;
    const previous = currentPage.offset;
    showMutation(null); setActiveEntry(null); chooseDestination(pathRef.current); clearSelection(); resetTree();
    if (await fetchPage(currentPage.next_offset)) {
      pageOffsets.current.push(previous);
      setActiveEntry(null); chooseDestination(pathRef.current); clearSelection(); resetTree();
    }
  }
  async function previousPage() {
    if (mutatingRef.current) return;
    if (!pageOffsets.current.length || loading) return;
    const previous = pageOffsets.current[pageOffsets.current.length - 1];
    showMutation(null); setActiveEntry(null); chooseDestination(pathRef.current); clearSelection(); resetTree();
    if (await fetchPage(previous)) { pageOffsets.current.pop(); setActiveEntry(null); chooseDestination(pathRef.current); clearSelection(); resetTree(); }
  }
  async function loadText() {
    const expected = captureSelection();
    if (!expected || !current(expected)) return;
    setPreviewLoading(true);
    setError("");
    try {
      const value = await invokeDesktop<BrowserTextPreview>(
        "workspace_browser_text",
        {
          sessionId: expected.sessionId,
          space: expected.space,
          path: expected.path,
        },
      );
      if (!current(expected)) return;
      previewRef.current = value;
      setPreview(value);
    } catch (reason) {
      if (current(expected)) setError(formatError(reason));
    } finally {
      if (current(expected)) setPreviewLoading(false);
    }
  }
  function select(entry: BrowserEntry) {
    if (mutatingRef.current) return;
    setActiveEntry(entry);
    chooseDestination(entry.kind === "directory" ? entry.path : parentBrowserPath(entry.path));
    if (entry.kind === "directory") {
      clearSelection();
      enter(entry);
      return;
    }
    clearSelection();
    selectedRef.current = entry;
    setSelected(entry);
    setError("");
    if (browserFileKind(entry) === "text") void loadText();
  }
  function focusEntry(entry: BrowserEntry) {
    if (mutatingRef.current) return;
    setActiveEntry(entry);
    chooseDestination(entry.kind === "directory" ? entry.path : parentBrowserPath(entry.path));
    if (entry.kind !== "directory") select(entry);
  }
  async function loadAudio(): Promise<boolean> {
    if (mutatingRef.current) return false;
    const expected = captureSelection();
    const file = selectedRef.current;
    if (!expected || !file || !current(expected) || audioLoadingRef.current)
      return false;
    const mime = audioMime(file.path);
    if (!mime || file.kind !== "file") return false;
    clearAudio();
    if (file.bytes !== null && file.bytes > AUDIO_PREVIEW_LIMIT) {
      setError("音频快照仅支持不超过 32 MiB 的文件。");
      return false;
    }
    const token = ++audioEpoch.current;
    const isCurrent = () => current(expected) && token === audioEpoch.current;
    audioLoadingRef.current = true;
    setAudioLoading(true);
    setError("");
    try {
      const bytes = await invokeDesktop<ArrayBuffer>(
        "workspace_browser_audio",
        {
          sessionId: expected.sessionId,
          space: expected.space,
          path: expected.path,
        },
      );
      if (!isCurrent()) return false;
      if (!(bytes instanceof ArrayBuffer))
        throw new Error("音频返回的二进制格式无效。");
      const url = owner.replace(bytes, mime, isCurrent);
      if (!url) return false;
      setAudioUrl(url);
      return true;
    } catch (reason) {
      if (isCurrent()) setError(formatError(reason));
      return false;
    } finally {
      if (isCurrent()) {
        audioLoadingRef.current = false;
        setAudioLoading(false);
      }
    }
  }
  function reportAudioError() {
    if (visibleRef.current && owner.url) {
      clearAudio();
      setError(
        "浏览器无法解码这份音频。可重新加载，或使用支持该格式的本地播放器。",
      );
    }
  }
  function capturePreview(): BrowserSnapshot | null {
    return previewRef.current ? captureSelection() : null;
  }
  function isCurrentPreview(expected: BrowserSnapshot | null) {
    return !!previewRef.current && current(expected);
  }
  function mutationBlocked(operation: BrowserMutationOperation, source: string, sourceSpace: BrowserSpace): string | null {
    const blocked = optionsRef.current.blockedReason?.();
    if (blocked) return blocked;
    if (["rename", "move", "trash"].includes(operation) && optionsRef.current.protectedPaths?.().some(item => item.space === sourceSpace && browserPathContains(source, item.path)))
      return "当前文件有未保存编辑，请先保存或另存，再执行此操作。";
    return null;
  }
  function beginMutation(operation: Exclude<BrowserMutationOperation, "import">, entry?: BrowserEntry) {
    const session = currentConnection.current;
    if (!enabled || !session || !visibleRef.current || mutatingRef.current) return;
    const source = operation === "mkdir" ? destinationRef.current : entry?.path;
    if (source === undefined || entry?.kind === "unsupported") return;
    const blocked = mutationBlocked(operation, source, spaceRef.current);
    if (blocked) { setError(blocked); return; }
    setError("");
    showMutation({ operation, path: source, space: spaceRef.current, sessionId: session.sessionId, identity: scopeRef.current.identity, directory: pathRef.current, target: operation === "mkdir" ? "新建文件夹" : source.split("/").pop() ?? "" });
    // User-triggered deletion is recoverable; keep task/dirty guards and the
    // synchronous mutation lease, but do not require another confirmation.
    if (operation === "trash") void submitMutation("");
  }
  function copyEntry(entry: BrowserEntry, cut = false) {
    if (!enabled || mutatingRef.current || entry.kind === "unsupported") return;
    const blocked = mutationBlocked(cut ? "move" : "copy", entry.path, spaceRef.current);
    if (blocked) { setError(blocked); return; }
    updateClipboard({ entry, operation: cut ? "move" : "copy", identity: scopeRef.current.identity, space: spaceRef.current });
  }
  async function paste() {
    const clip = clipboardRef.current, session = currentConnection.current;
    if (!clip || !session || !visibleRef.current || mutatingRef.current || clip.identity !== scopeRef.current.identity || clip.space !== spaceRef.current) return;
    const target = [destinationRef.current, clip.entry.name].filter(Boolean).join("/");
    showMutation({ operation: clip.operation, path: clip.entry.path, space: clip.space, sessionId: session.sessionId, identity: clip.identity, directory: pathRef.current, target });
    await submitMutation(target);
  }
  function cancelMutation() { if (!mutatingRef.current) showMutation(null); }
  async function submitMutation(value: string) {
    const dialog = mutationDialogRef.current;
    if (!dialog || mutatingRef.current || !enabled || !visibleRef.current || dialog.identity !== scopeRef.current.identity || dialog.directory !== pathRef.current || dialog.space !== spaceRef.current) return;
    let invoked = false;
    const scope = scopeRef.current;
    const previousDestination = destinationRef.current;
    try {
      const blocked = mutationBlocked(dialog.operation, dialog.path, dialog.space);
      if (blocked) throw new Error(blocked);
      const target = dialog.operation === "mkdir" ? [dialog.path, browserEntryName(value)].filter(Boolean).join("/") : dialog.operation === "rename" ? [parentBrowserPath(dialog.path), browserEntryName(value)].filter(Boolean).join("/") : ["copy", "move"].includes(dialog.operation) ? browserMutationTarget(value) : undefined;
      const samePath = target?.toLowerCase() === dialog.path.toLowerCase();
      if (target && !["mkdir", "copy"].includes(dialog.operation) && samePath) throw new Error("目标路径与原路径相同。");
      if (dialog.operation === "rename" && target && parentBrowserPath(target) !== parentBrowserPath(dialog.path)) throw new Error("重命名须保持原目录；更换目录请使用移动。");
      if (target && ["move", "copy"].includes(dialog.operation) && !samePath && browserPathContains(dialog.path, target)) throw new Error("目标不能位于源条目内部。");
      mutatingRef.current = true;
      setMutating(true);
      setError("");
      clearSelection();
      invoked = true;
      const reply = await invokeDesktop<{ message?: string; path?: string; paths?: string[] }>("workspace_files_mutate", { sessionId: dialog.sessionId, space: dialog.space, operation: dialog.operation, path: dialog.operation === "mkdir" ? target : dialog.path, ...(target && dialog.operation !== "mkdir" ? { target } : {}), ...(dialog.dropToken ? { dropToken: dialog.dropToken } : {}) });
      if (mounted.current && scope === scopeRef.current && visibleRef.current && dialog.directory === pathRef.current && dialog.space === spaceRef.current) {
        showMutation(null);
        if (dialog.operation === "move") updateClipboard(null);
        resetTree();
        setActiveEntry(null);
        pageOffsets.current = [];
        await fetchPage(0);
        if (scope === scopeRef.current && visibleRef.current) {
          const actualTarget = reply.path ?? reply.paths?.[0] ?? target;
          const folder = dialog.operation === "import" ? dialog.path : actualTarget && dialog.operation !== "trash" ? parentBrowserPath(actualTarget) : parentBrowserPath(dialog.path);
          await restoreTreeContext(folder, dialog.operation === "trash" || dialog.operation === "import" ? undefined : actualTarget);
        }
      }
    } catch (reason) {
      if (mounted.current && scope === scopeRef.current && visibleRef.current && dialog.directory === pathRef.current && dialog.space === spaceRef.current) {
        if (invoked) {
          if (["import", "copy", "move", "trash"].includes(dialog.operation)) showMutation(null);
          resetTree();
          pageOffsets.current = [];
          await fetchPage(0);
          if (scope === scopeRef.current && visibleRef.current) await restoreTreeContext(previousDestination);
        }
        if (mounted.current && scope === scopeRef.current && visibleRef.current && dialog.directory === pathRef.current && dialog.space === spaceRef.current) setError(formatError(reason));
      }
    } finally { mutatingRef.current = false; if (mounted.current) setMutating(false); }
  }
  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenWorkspaceFileDrop(event => {
      if (!visibleRef.current || !currentConnection.current || mutatingRef.current) return;
      if (event.sessionId && event.sessionId !== currentConnection.current.sessionId) return;
      setDropActive(event.kind === "enter" || event.kind === "over");
      if (event.kind !== "drop") return;
      if (event.error) { setError(event.error); return; }
      if (!event.token || !event.sessionId || event.sessionId !== currentConnection.current.sessionId) return;
      const blocked = optionsRef.current.blockedReason?.();
      if (blocked) { setError(blocked); return; }
      setError("");
      showMutation({ operation: "import", path: destinationRef.current, space: spaceRef.current, sessionId: event.sessionId, identity: scopeRef.current.identity, directory: pathRef.current, target: "", names: event.names ?? [], dropToken: event.token });
    }).then(stop => { if (disposed) stop(); else unlisten = stop; }).catch(reason => { if (!disposed && visibleRef.current) setError(formatError(reason)); });
    return () => { disposed = true; unlisten?.(); };
  }, [enabled]);
  useEffect(() => {
    if (visible && connection) void fetchPage(0);
  }, [identity, visible]);

  return {
    space,
    activeEntry,
    focusEntry,
    destination,
    branches,
    toggleDirectory,
    clipboard,
    copyEntry,
    paste,
    chooseRoot: () => { if (!mutatingRef.current) { setActiveEntry(null); chooseDestination(pathRef.current); } },
    filesEnabled: enabled,
    mutating,
    readMutating: () => mutatingRef.current,
    mutationDialog,
    beginMutation,
    cancelMutation,
    submitMutation,
    dropActive,
    setSpace,
    path,
    listing,
    entries: listing?.entries ?? [],
    selected,
    preview,
    audioUrl,
    audioRef,
    clearAudio,
    loading,
    previewLoading,
    audioLoading,
    busy: loading || previewLoading || audioLoading || mutating,
    error,
    refresh,
    goTo,
    enter,
    up,
    nextPage,
    previousPage,
    select,
    loadText,
    loadAudio,
    reportAudioError,
    capturePreview,
    isCurrentPreview,
    canPreviousPage: pageOffsets.current.length > 0,
  };
}

export type WorkspaceBrowser = ReturnType<typeof useWorkspaceBrowser>;
