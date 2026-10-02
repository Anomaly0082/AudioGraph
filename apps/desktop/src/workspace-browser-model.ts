import { parseGraph } from "./model";

export type BrowserSpace = "user" | "ai";
export type BrowserEntry = {
  name: string;
  path: string;
  kind: "file" | "directory" | "unsupported";
  bytes: number | null;
  modified_at_ms: number | null;
  message?: string;
};
export type BrowserListing = {
  space: BrowserSpace;
  path: string;
  entries: BrowserEntry[];
  offset: number;
  next_offset: number | null;
  total: number | null;
  partial: boolean;
  warnings: string[];
};
export type BrowserTextPreview = {
  space: BrowserSpace;
  path: string;
  text: string;
  bytes: number;
  truncated: boolean;
};
export type BrowserSnapshot = Readonly<{
  sessionId: string;
  workspace: string;
  space: BrowserSpace;
  path: string;
  epoch: number;
}>;

export const AUDIO_PREVIEW_LIMIT = 32 * 1024 * 1024;
export const TEXT_PREVIEW_LIMIT = 256 * 1024;
export const BROWSER_PAGE_SIZE = 100;

const audioTypes: Record<string, string> = {
  wav: "audio/wav",
  mp3: "audio/mpeg",
  ogg: "audio/ogg",
  oga: "audio/ogg",
  flac: "audio/flac",
  m4a: "audio/mp4",
  aac: "audio/aac",
  webm: "audio/webm",
};
const textExtensions = new Set([
  "json",
  "txt",
  "md",
  "markdown",
  "csv",
  "tsv",
  "log",
  "yaml",
  "yml",
  "toml",
  "ini",
  "cfg",
  "xml",
  "html",
  "htm",
  "css",
  "js",
  "ts",
  "py",
  "rs",
  "c",
  "cpp",
  "h",
  "hpp",
  "sh",
  "ps1",
  "bat",
]);
const textNames = new Set([
  "readme",
  "license",
  "notice",
  "changelog",
  "authors",
  ".gitignore",
  ".gitattributes",
  "cmakelists.txt",
]);

export function audioMime(path: string): string | null {
  return audioTypes[path.split(".").pop()?.toLowerCase() ?? ""] ?? null;
}
export function browserFileKind(
  entry: BrowserEntry,
): "text" | "audio" | "unknown" {
  if (entry.kind !== "file") return "unknown";
  if (audioMime(entry.path)) return "audio";
  return textNames.has(entry.name.toLowerCase()) ||
    textExtensions.has(entry.path.split(".").pop()?.toLowerCase() ?? "")
    ? "text"
    : "unknown";
}

export function browserRelativePath(path: string): string {
  if (path === "") return path;
  if (
    path.startsWith("/") ||
    /[\\:\0]/.test(path) ||
    path.split("/").some((part) => !part || part === "." || part === "..")
  ) {
    throw new Error("文件路径必须位于所选工作区内。");
  }
  return path;
}
export function parentBrowserPath(path: string): string {
  return browserRelativePath(path).split("/").slice(0, -1).join("/");
}
export function browserBreadcrumbs(
  path: string,
): { name: string; path: string }[] {
  const parts = browserRelativePath(path).split("/").filter(Boolean);
  return [
    { name: "根目录", path: "" },
    ...parts.map((name, index) => ({
      name,
      path: parts.slice(0, index + 1).join("/"),
    })),
  ];
}
export function matchesBrowserSnapshot(
  expected: BrowserSnapshot,
  current: BrowserSnapshot | null,
  visible = true,
): boolean {
  return (
    visible &&
    !!current &&
    expected.sessionId === current.sessionId &&
    expected.workspace === current.workspace &&
    expected.space === current.space &&
    expected.path === current.path &&
    expected.epoch === current.epoch
  );
}
export function formatBrowserBytes(bytes: number | null): string {
  if (bytes === null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

// Recognition only offers loading a complete draft. Execution still requires
// the editor's explicit validation and run controls.
export function browserConfigurationKind(
  preview: BrowserTextPreview | null,
): "graph" | "workflow" | null {
  if (
    !preview ||
    preview.truncated ||
    new TextEncoder().encode(preview.text).length > TEXT_PREVIEW_LIMIT
  )
    return null;
  try {
    parseGraph(preview.text, { allowEmpty: true });
    return "graph";
  } catch {
    /* A Workflow has a different document shape. */
  }
  if (new TextEncoder().encode(preview.text).length > 64 * 1024) return null;
  try {
    const value: unknown = JSON.parse(preview.text);
    const object = (v: unknown): v is Record<string, unknown> =>
      !!v && typeof v === "object" && !Array.isArray(v);
    return object(value) &&
      value.schema_version === 1 &&
      object(value.inputs) &&
      Array.isArray(value.steps) &&
      object(value.outputs)
      ? "workflow"
      : null;
  } catch {
    return null;
  }
}

type AudioPlayer = Pick<
  HTMLAudioElement,
  "pause" | "removeAttribute" | "load" | "src"
>;
type AudioResources = {
  create: (blob: Blob) => string;
  revoke: (url: string) => void;
};

// One owner makes replacement and late-response disposal testable without a
// browser. It never calls play(): native controls own the user's playback.
export class AudioPreviewOwner {
  private player: AudioPlayer | null = null;
  private value: string | null = null;
  constructor(
    private resources: AudioResources = {
      create: (blob) => URL.createObjectURL(blob),
      revoke: (url) => URL.revokeObjectURL(url),
    },
  ) {}
  get url(): string | null {
    return this.value;
  }
  private unload(player: AudioPlayer) {
    player.pause();
    player.removeAttribute("src");
    player.load();
  }
  attach(player: AudioPlayer | null) {
    if (this.player === player) return;
    if (this.player) this.unload(this.player);
    this.player = player;
    // Callback refs may detach/reattach in development StrictMode. The scope
    // owns the URL; detaching always stops audio without inventing another URL.
    if (player && this.value) player.src = this.value;
  }
  clear() {
    if (this.player) this.unload(this.player);
    if (this.value) this.resources.revoke(this.value);
    this.value = null;
  }
  replace(
    bytes: ArrayBuffer,
    mime: string,
    isCurrent: () => boolean,
  ): string | null {
    if (!isCurrent()) return null;
    if (!bytes.byteLength || bytes.byteLength > AUDIO_PREVIEW_LIMIT) {
      throw new Error("音频快照需为非空文件且不超过 32 MiB。");
    }
    this.clear();
    const url = this.resources.create(new Blob([bytes], { type: mime }));
    if (!isCurrent()) {
      this.resources.revoke(url);
      return null;
    }
    this.value = url;
    return url;
  }
}
