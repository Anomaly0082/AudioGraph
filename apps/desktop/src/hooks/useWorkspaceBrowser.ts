import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { invokeDesktop } from "../api/desktop";
import { formatError } from "../model";
import type { Connection } from "../types/desktop";
import {
  AudioPreviewOwner,
  AUDIO_PREVIEW_LIMIT,
  BROWSER_PAGE_SIZE,
  audioMime,
  browserFileKind,
  browserRelativePath,
  matchesBrowserSnapshot,
  parentBrowserPath,
  type BrowserEntry,
  type BrowserListing,
  type BrowserSnapshot,
  type BrowserSpace,
  type BrowserTextPreview,
} from "../workspace-browser-model";

export function useWorkspaceBrowser(
  connection: Connection | null,
  visible: boolean,
) {
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
    pageOffsets.current = [];
    return fetchPage(0);
  }
  function goTo(value: string) {
    try {
      const next = browserRelativePath(value);
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
    if (value !== "user" && value !== "ai") return;
    if (spaceRef.current === value) return;
    spaceRef.current = value;
    setSpaceState(value);
    goTo("");
  }
  function enter(entry: BrowserEntry) {
    if (entry.kind === "directory") goTo(entry.path);
  }
  function up() {
    goTo(parentBrowserPath(pathRef.current));
  }
  async function nextPage() {
    const currentPage = listingRef.current;
    if (!currentPage || currentPage.next_offset === null || loading) return;
    const previous = currentPage.offset;
    if (await fetchPage(currentPage.next_offset))
      pageOffsets.current.push(previous);
  }
  async function previousPage() {
    if (!pageOffsets.current.length || loading) return;
    const previous = pageOffsets.current[pageOffsets.current.length - 1];
    if (await fetchPage(previous)) pageOffsets.current.pop();
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
    if (entry.kind === "directory") {
      enter(entry);
      return;
    }
    clearSelection();
    selectedRef.current = entry;
    setSelected(entry);
    setError("");
    if (browserFileKind(entry) === "text") void loadText();
  }
  async function loadAudio(): Promise<boolean> {
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
  useEffect(() => {
    if (visible && connection) void fetchPage(0);
  }, [identity, visible]);

  return {
    space,
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
    busy: loading || previewLoading || audioLoading,
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
