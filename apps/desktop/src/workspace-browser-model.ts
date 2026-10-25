import { parseGraph } from "./model";

export type BrowserSpace = "user" | "ai";
export type BrowserMutationOperation = "mkdir" | "rename" | "copy" | "move" | "trash" | "import";
export function browserEntryName(value: string): string {
  if (value.includes("/")) throw new Error("名称不能包含目录分隔符。");
  return browserMutationTarget(value);
}
export function browserMutationTarget(value: string): string {
  const path = browserRelativePath(value);
  if (!path || path.split("/").some(part => /[<>"|?*\x00-\x1f]/.test(part) || /[. ]$/.test(part) || /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(\.|$)/i.test(part)))
    throw new Error("请输入有效的工作区相对路径（使用 / 分隔，不能使用系统保留名称）。");
  return path;
}
export function browserPathContains(parent: string, child: string): boolean {
  const a = parent.toLowerCase();
  const b = child.toLowerCase();
  return a === b || b.startsWith(`${a}/`);
}
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

export const TEXT_PREVIEW_LIMIT = 256 * 1024;
export const BROWSER_PAGE_SIZE = 100;
export const audioOutputFormats = ["wav", "flac", "mp3", "m4a"] as const;
export type AudioOutputFormat = typeof audioOutputFormats[number];
export type AudioConversionOptions = { sampleRate?: number; channels?: number };
export const audioConversionSampleRates = [8000, 16000, 22050, 24000, 32000, 44100, 48000, 88200, 96000, 176400, 192000] as const;
export function validatedAudioConversionOptions(format: AudioOutputFormat, options: AudioConversionOptions = {}): AudioConversionOptions {
  if (!audioOutputFormats.includes(format)) throw new Error("请选择支持的输出格式。");
  const { sampleRate, channels } = options;
  if (sampleRate !== undefined && (!Number.isInteger(sampleRate) || sampleRate < 8000 || sampleRate > 192000)) throw new Error("采样率须为 8000 至 192000 Hz 的整数。");
  if (channels !== undefined && channels !== 1 && channels !== 2) throw new Error("请选择单声道或双声道，或保持原样。");
  const lossyRates = [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000];
  if (sampleRate !== undefined && ((format === "mp3" && !lossyRates.includes(sampleRate)) || (format === "m4a" && ![...lossyRates, 64000, 88200, 96000].includes(sampleRate)))) throw new Error(`${format === "mp3" ? "MP3" : "M4A"} 不支持 ${sampleRate} Hz，请重新选择采样率。`);
  return { ...(sampleRate !== undefined ? { sampleRate } : {}), ...(channels !== undefined ? { channels } : {}) };
}
export function replaceConvertedAudioExtension(name: string, format: AudioOutputFormat): string {
  return `${name.replace(/\.[^./\\]+$/, "")}.${format}`;
}
export function defaultConvertedAudioName(path: string, format: AudioOutputFormat = "wav"): string {
  const name = path.split("/").pop() ?? "audio";
  return `${name.replace(/\.[^.]+$/, "") || "audio"}_converted.${format}`;
}
export function convertedAudioOutputPath(source: string, name: string, format: AudioOutputFormat = "wav"): string {
  if (!audioOutputFormats.includes(format)) throw new Error("请选择支持的输出格式。");
  const clean = browserEntryName(name);
  if (!clean.toLowerCase().endsWith(`.${format}`)) throw new Error(`输出文件名必须以 .${format} 结尾，与所选格式一致。`);
  const target = [parentBrowserPath(source), clean].filter(Boolean).join("/");
  if (target.toLowerCase() === source.toLowerCase()) throw new Error("输出文件名不能与源文件相同。");
  return target;
}

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
export type AudioPreviewSession = { token: string; bytes: number; mime: string };
type AudioResources = {
  url: (token: string) => string;
  release: (token: string) => void | Promise<void>;
};
export function trustedAudioPreviewUrl(value: string): boolean {
  return /^(?:audiopreview:\/\/localhost|http:\/\/audiopreview\.localhost)\/[a-zA-Z0-9_-]+$/.test(value);
}

// One owner makes replacement and late-response disposal testable without a
// browser. It never calls play(): native controls own the user's playback.
export class AudioPreviewOwner {
  private player: AudioPlayer | null = null;
  private value: string | null = null;
  private token: string | null = null;
  private releases = new Set<Promise<void>>();
  private releaseErrors: unknown[] = [];
  constructor(
    private resources: AudioResources,
  ) {}
  private release(token: string) {
    let result: void | Promise<void>;
    try { result = this.resources.release(token); } catch (error) { this.releaseErrors.push(error); return; }
    const pending = Promise.resolve(result).catch(error => {
      this.releaseErrors.push(error);
    }).finally(() => { this.releases.delete(pending); });
    this.releases.add(pending);
  }
  // Route cleanup is best effort; mutations explicitly wait for file handles
  // to close and surface failure before attempting to rename or remove them.
  async drain(): Promise<void> {
    while (this.releases.size) await Promise.all(this.releases);
    if (this.releaseErrors.length) {
      const error = this.releaseErrors[0];
      this.releaseErrors = [];
      throw error;
    }
  }
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
    if (this.token) this.release(this.token);
    this.token = null;
    this.value = null;
  }
  replace(
    session: AudioPreviewSession,
    isCurrent: () => boolean,
  ): string | null {
    const token = session?.token;
    if (typeof token !== "string" || !/^[a-zA-Z0-9_-]+$/.test(token)) throw new Error("音频会话无效。");
    if (!isCurrent()) { this.release(token); return null; }
    if (!Number.isSafeInteger(session.bytes) || session.bytes <= 0 || !Object.values(audioTypes).includes(session.mime)) {
      this.release(token);
      throw new Error("音频会话信息无效。");
    }
    let url: string;
    try {
      url = this.resources.url(token);
      if (!trustedAudioPreviewUrl(url) || new URL(url).pathname !== `/${token}`) throw new Error("音频地址无效。");
    } catch (error) { this.release(token); throw error; }
    this.clear();
    if (!isCurrent()) {
      this.release(token);
      return null;
    }
    this.token = token;
    this.value = url;
    return url;
  }
}
