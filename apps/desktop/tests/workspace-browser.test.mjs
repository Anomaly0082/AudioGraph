import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

const require = createRequire(import.meta.url);
const {
  AUDIO_PREVIEW_LIMIT,
  AudioPreviewOwner,
  audioMime,
  browserBreadcrumbs,
  browserConfigurationKind,
  browserFileKind,
  browserRelativePath,
  browserMutationTarget,
  browserEntryName,
  browserPathContains,
  formatBrowserBytes,
  matchesBrowserSnapshot,
  parentBrowserPath,
} = require("../../../build/desktop-model-tests/workspace-browser-model.js");
const Page =
  require("../../../build/desktop-model-tests/pages/WorkspaceBrowserPage.js").default;

function entry(overrides = {}) {
  return {
    name: "clip.wav",
    path: "sounds/clip.wav",
    kind: "file",
    bytes: 24,
    modified_at_ms: null,
    ...overrides,
  };
}
function snapshot(overrides = {}) {
  return {
    sessionId: "session-a",
    workspace: "C:/project-a",
    space: "user",
    path: "sounds/clip.wav",
    epoch: 1,
    ...overrides,
  };
}
const graphText = JSON.stringify({
  schema_version: 1,
  nodes: [{ id: "in", type: "text_input", parameters: { text: "local" } }],
  connections: [],
});
const workflowText = JSON.stringify({
  schema_version: 1,
  inputs: {},
  steps: [],
  outputs: {},
});
function preview(text, overrides = {}) {
  return {
    space: "user",
    path: "config.json",
    text,
    bytes: new TextEncoder().encode(text).length,
    truncated: false,
    ...overrides,
  };
}

test("directory navigation uses normalized relative breadcrumbs and bounded parents", () => {
  assert.deepEqual(browserBreadcrumbs("a/b"), [
    { name: "根目录", path: "" },
    { name: "a", path: "a" },
    { name: "b", path: "a/b" },
  ]);
  assert.equal(parentBrowserPath("a/b"), "a");
  assert.equal(parentBrowserPath("a"), "");
  assert.equal(parentBrowserPath(""), "");
  for (const path of [
    "../a",
    "a/../b",
    "/outside",
    "C:/outside",
    "a\\b",
    "a//b",
    "a/./b",
  ]) {
    assert.throws(() => browserRelativePath(path), /工作区内/);
  }
});

test("late file and audio responses match session, workspace, space, path, epoch and visibility", () => {
  assert.equal(matchesBrowserSnapshot(snapshot(), snapshot()), true);
  for (const change of [
    { sessionId: "session-b" },
    { workspace: "C:/project-b" },
    { space: "ai" },
    { path: "sounds/next.wav" },
    { epoch: 2 },
  ]) {
    assert.equal(matchesBrowserSnapshot(snapshot(), snapshot(change)), false);
  }
  assert.equal(matchesBrowserSnapshot(snapshot(), snapshot(), false), false);
  assert.equal(matchesBrowserSnapshot(snapshot(), null), false);
});

test("file classifiers choose explicit audio MIME or plain text without executing unknown types", () => {
  for (const [extension, mime] of Object.entries({
    wav: "audio/wav",
    mp3: "audio/mpeg",
    ogg: "audio/ogg",
    oga: "audio/ogg",
    flac: "audio/flac",
    m4a: "audio/mp4",
    aac: "audio/aac",
    webm: "audio/webm",
  })) {
    assert.equal(audioMime(`A.${extension.toUpperCase()}`), mime);
    assert.equal(browserFileKind(entry({ path: `A.${extension}` })), "audio");
  }
  assert.equal(browserFileKind(entry({ path: "unsafe.html" })), "text");
  assert.equal(
    browserFileKind(entry({ name: "README", path: "README" })),
    "text",
  );
  assert.equal(browserFileKind(entry({ path: "unknown.exe" })), "unknown");
  assert.equal(browserFileKind(entry({ kind: "unsupported" })), "unknown");
  assert.equal(formatBrowserBytes(AUDIO_PREVIEW_LIMIT), "32.0 MiB");
});

test("only complete shaped configurations offer editor loading", () => {
  assert.equal(browserConfigurationKind(preview(graphText)), "graph");
  assert.equal(browserConfigurationKind(preview(workflowText)), "workflow");
  for (const value of [
    preview(graphText, { truncated: true }),
    preview(workflowText, { truncated: true }),
    preview("{broken"),
    preview("{}"),
    preview('{"schema_version":1,"steps":[],"outputs":{}}'),
    preview('{"schema_version":1,"nodes":[{"id":"in"}],"connections":[]}'),
    preview(
      '{"schema_version":1,"schema_version":1,"nodes":[{"id":"in","type":"text_input"}],"connections":[]}',
    ),
  ]) {
    assert.equal(browserConfigurationKind(value), null);
  }
  assert.equal(browserConfigurationKind(null), null);
  assert.equal(
    browserConfigurationKind(preview(workflowText + " ".repeat(64 * 1024))),
    null,
  );
});

function audioFixture() {
  const calls = [];
  let serial = 0;
  const owner = new AudioPreviewOwner({
    create: (blob) => {
      calls.push(`create:${blob.type}:${blob.size}`);
      return `blob:${++serial}`;
    },
    revoke: (url) => calls.push(`revoke:${url}`),
  });
  const player = {
    src: "",
    pause: () => calls.push("pause"),
    removeAttribute: (name) => calls.push(`remove:${name}`),
    load: () => calls.push("load"),
  };
  return { owner, calls, player };
}

test("audio snapshot never starts playback, owns at most one Blob, and unloads before revoke", () => {
  const { owner, calls, player } = audioFixture();
  assert.equal(
    owner.replace(new ArrayBuffer(8), "audio/wav", () => true),
    "blob:1",
  );
  owner.attach(player);
  assert.equal(player.src, "blob:1");
  assert.equal(
    owner.replace(new ArrayBuffer(9), "audio/wav", () => true),
    "blob:2",
  );
  assert.deepEqual(calls.slice(1, 5), [
    "pause",
    "remove:src",
    "load",
    "revoke:blob:1",
  ]);
  owner.clear();
  assert.equal(owner.url, null);
  assert.deepEqual(calls.slice(-4), [
    "pause",
    "remove:src",
    "load",
    "revoke:blob:2",
  ]);
  assert.equal(
    calls.some((call) => call === "play"),
    false,
  );
});

test("audio detachment immediately pauses and unloads while a StrictMode ref can reattach the same URL", () => {
  const { owner, calls, player } = audioFixture();
  owner.replace(new ArrayBuffer(8), "audio/wav", () => true);
  owner.attach(player);
  owner.attach(null);
  assert.deepEqual(calls.slice(-3), ["pause", "remove:src", "load"]);
  owner.attach(player);
  assert.equal(player.src, "blob:1");
  assert.equal(calls.filter((call) => call.startsWith("create:")).length, 1);
  owner.clear();
  assert.equal(calls.at(-1), "revoke:blob:1");
});

test("late audio creates no URL and a scope change during creation immediately revokes it", () => {
  const fixture = audioFixture();
  assert.equal(
    fixture.owner.replace(new ArrayBuffer(8), "audio/wav", () => false),
    null,
  );
  assert.deepEqual(fixture.calls, []);
  let attempts = 0;
  assert.equal(
    fixture.owner.replace(
      new ArrayBuffer(8),
      "audio/wav",
      () => ++attempts === 1,
    ),
    null,
  );
  assert.deepEqual(fixture.calls, ["create:audio/wav:8", "revoke:blob:1"]);
  assert.equal(fixture.owner.url, null);
  assert.throws(
    () => fixture.owner.replace(new ArrayBuffer(0), "audio/wav", () => true),
    /32 MiB/,
  );
  assert.throws(
    () =>
      fixture.owner.replace(
        new ArrayBuffer(AUDIO_PREVIEW_LIMIT + 1),
        "audio/wav",
        () => true,
      ),
    /32 MiB/,
  );
});

const noop = () => {};
function render(overrides = {}, props = {}) {
  const browser = {
    space: "user",
    path: "",
    entries: [],
    selected: null,
    preview: null,
    audioUrl: null,
    loading: false,
    previewLoading: false,
    audioLoading: false,
    error: "",
    canPreviousPage: false,
    listing: {
      space: "user",
      path: "",
      entries: [],
      offset: 0,
      next_offset: null,
      total: 0,
      partial: false,
      warnings: [],
    },
    refresh: noop,
    up: noop,
    goTo: noop,
    setSpace: noop,
    select: noop,
    nextPage: noop,
    previousPage: noop,
    loadAudio: noop,
    audioRef: noop,
    reportAudioError: noop,
    capturePreview: () => snapshot(),
    isCurrentPreview: () => true,
    ...overrides,
  };
  return renderToStaticMarkup(
    React.createElement(Page, {
      browser,
      connected: true,
      onOpenGraph: noop,
      onOpenWorkflow: noop,
      ...props,
    }),
  );
}

test("browser renders compact tree entries, shared toolbar, breadcrumbs and pagination", () => {
  const html = render({
    path: "sounds",
    entries: [
      entry(),
      entry({
        name: "sub",
        path: "sounds/sub",
        kind: "directory",
        bytes: null,
      }),
    ],
    listing: { total: null, partial: true, warnings: [], next_offset: 100 },
  });
  assert.match(html, /文件工作区/);
  assert.match(html, /根目录/);
  assert.match(html, /sounds\/clip.wav|clip.wav/);
  assert.match(html, /下一页/);
  assert.match(html, /总条目数未知/);
  assert.match(html, /当前列表仅包含部分条目/);
  assert.doesNotMatch(html, /<details|<summary/);
  assert.match(html, /aria-expanded="false"/);
  assert.match(html, /role="tree"/);
  assert.match(html, /role="treeitem"/);
  assert.match(html, /新建文件夹/);
  assert.match(html, /粘贴/);
  assert.doesNotMatch(html, /的操作|workspace-file-actions/);
});

test("mutation targets reject workspace escape and Windows reserved names", () => {
  assert.equal(browserMutationTarget("folder/new.txt"), "folder/new.txt");
  for (const value of ["", "../a", "C:/a", "a\\b", "a//b", "a/./b", "a/NUL.txt", "a/CON", "a/file.", "a/file ", "a/*.txt", "a/a:b"])
    assert.throws(() => browserMutationTarget(value));
  assert.equal(browserPathContains("Folder", "folder/child/file.json"), true);
  assert.equal(browserPathContains("Folder", "folder"), true);
  assert.equal(browserPathContains("Folder", "folder-other/file.json"), false);
  assert.equal(browserEntryName("新文件夹"), "新文件夹");
  assert.throws(() => browserEntryName("folder/name"));
});

test("file tree uses meaningful SVG icons and omits inactive pagination", () => {
  const html = render({
    entries: [
      entry({name:"folder",path:"folder",kind:"directory",bytes:null}),
      entry({name:"clip.wav",path:"clip.wav"}),
      entry({name:"graph.json",path:"graph.json"}),
      entry({name:"notes.txt",path:"notes.txt"}),
      entry({name:"archive.zip",path:"archive.zip"}),
    ],
    branches: {folder:{expanded:true,loading:false,listing:{entries:[],next_offset:null,warnings:[]}}},
    listing: {total:5,next_offset:null,warnings:[]},
    canPreviousPage:false,
  });
  for (const kind of ["folder", "audio", "json", "text", "file"])
    assert.match(html, new RegExp(`workspace-entry-icon-${kind}`));
  assert.match(html, /workspace-tree-twistie expanded/);
  assert.match(html, /lucide-chevron-right/);
  assert.doesNotMatch(html, />[·⌄›]</);
  assert.doesNotMatch(html, /aria-label="上一页"|aria-label="下一页"/);
  assert.match(html, /aria-label="刷新"/);
  assert.match(html, /title="clip.wav"/);
});

test("root view omits repeated location labels and successful-operation banners", () => {
  const html = render({ notice: "操作已完成", destination: "" });
  assert.doesNotMatch(html, /根目录|目标目录|操作已完成|workspace-files-destination/);
  assert.doesNotMatch(html, /aria-label="文件路径"/);
});

test("trash has no confirmation dialog while import still preserves originals", () => {
  const base = { path: "nested/data.wav", space: "user", sessionId: "a", identity: "a", directory: "nested", target: "" };
  const trash = render({ mutationDialog: { ...base, operation: "trash" } });
  assert.doesNotMatch(trash, /<dialog|移到回收站后可恢复/);
  const importing = render({ mutationDialog: { ...base, operation: "import", names: ["external.wav"] } });
  assert.match(importing, /external.wav/);
  assert.match(importing, /原文件保留/);
  assert.match(importing, /不覆盖/);
  assert.match(importing, /<dialog/);
  assert.match(importing, /取消/);
});

test("audio selection requires loading and loaded native controls never autoplay", () => {
  const selected = entry();
  const unloaded = render({ selected });
  assert.match(unloaded, /加载音频/);
  assert.match(unloaded, /32 MiB/);
  assert.doesNotMatch(unloaded, /<audio/);
  const loaded = render({ selected, audioUrl: "blob:local-only" });
  assert.equal((loaded.match(/<audio /g) ?? []).length, 1);
  assert.match(loaded, /src="blob:local-only"/);
  assert.match(loaded, /controls=""/);
  // Inline SVG's standard xmlns URL is an identifier, not a network request.
  assert.doesNotMatch(loaded, /auto[Pp]lay|iframe|(?:src|href)="https?:\/\//);
  assert.match(
    render({ selected: entry({ bytes: AUDIO_PREVIEW_LIMIT + 1 }) }),
    /文件超过 32 MiB/,
  );
});

test("HTML remains escaped plain text and truncated JSON never offers editor import", () => {
  const html = render({
    selected: entry({ name: "test.html", path: "test.html" }),
    preview: preview(
      '<script>alert(1)</script><img src="https://remote.test/a.png">',
    ),
  });
  assert.match(html, /&lt;script&gt;/);
  assert.doesNotMatch(html, /<script|<img|iframe/);
  const truncated = render({
    selected: entry({ path: "config.json" }),
    preview: preview(graphText, { truncated: true }),
  });
  assert.match(truncated, /文本预览已截断/);
  assert.doesNotMatch(truncated, /载入 Graph 编辑器/);
  assert.match(
    render({
      selected: entry({ path: "config.json" }),
      preview: preview(graphText),
    }),
    /载入 Graph 编辑器/,
  );
  assert.match(
    render({
      selected: entry({ path: "flow.json" }),
      preview: preview(workflowText),
    }),
    /载入 Workflow 编辑器/,
  );
});

test("unknown files show metadata and local decoding errors remain explicit", () => {
  const html = render({
    selected: entry({ path: "program.exe" }),
    error: "本地解码失败",
  });
  assert.match(html, /此类型仅显示文件信息/);
  assert.match(html, /role="alert"/);
  assert.match(html, /本地解码失败/);
  assert.doesNotMatch(html, /加载音频|<audio|运行文件/);
});

test("desktop CSP allows local Blob audio without permitting inline scripts", () => {
  const config = JSON.parse(
    readFileSync(
      new URL("../src-tauri/tauri.conf.json", import.meta.url),
      "utf8",
    ),
  );
  const csp = config.app.security.csp;
  assert.equal(typeof csp, "string");
  const directive = (name) =>
    csp
      .split(";")
      .map((part) => part.trim())
      .find((part) => part.startsWith(`${name} `)) ?? "";
  assert.match(directive("media-src"), /(?:^|\s)blob:(?:\s|$)/);
  assert.match(directive("script-src"), /'self'/);
  assert.doesNotMatch(directive("script-src"), /unsafe-inline|unsafe-eval/);
});
