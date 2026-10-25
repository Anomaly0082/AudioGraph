import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

const require = createRequire(import.meta.url);
const {
  AudioPreviewOwner,
  audioMime,
  browserBreadcrumbs,
  browserConfigurationKind,
  browserFileKind,
  browserRelativePath,
  browserMutationTarget,
  browserEntryName,
  defaultConvertedAudioName,
  convertedAudioOutputPath,
  replaceConvertedAudioExtension,
  validatedAudioConversionOptions,
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
  assert.equal(formatBrowserBytes(32 * 1024 * 1024), "32.0 MiB");
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
  const owner = new AudioPreviewOwner({
    url: (token) => {
      calls.push(`url:${token}`);
      return `audiopreview://localhost/${token}`;
    },
    release: (token) => calls.push(`release:${token}`),
  });
  const player = {
    src: "",
    pause: () => calls.push("pause"),
    removeAttribute: (name) => calls.push(`remove:${name}`),
    load: () => calls.push("load"),
  };
  return { owner, calls, player };
}

const audioSession = (token = "one", bytes = 8) => ({token, bytes, mime:"audio/wav"});
test("audio session supports large files, never starts playback, and unloads before release", () => {
  const { owner, calls, player } = audioFixture();
  assert.equal(
    owner.replace(audioSession(), () => true),
    "audiopreview://localhost/one",
  );
  owner.attach(player);
  assert.equal(player.src, "audiopreview://localhost/one");
  assert.equal(
    owner.replace(audioSession("two", 1024 * 1024 * 1024), () => true),
    "audiopreview://localhost/two",
  );
  assert.deepEqual(calls.slice(2, 6), [
    "pause",
    "remove:src",
    "load",
    "release:one",
  ]);
  owner.clear();
  assert.equal(owner.url, null);
  assert.deepEqual(calls.slice(-4), [
    "pause",
    "remove:src",
    "load",
    "release:two",
  ]);
  assert.equal(
    calls.some((call) => call === "play"),
    false,
  );
});

test("audio detachment immediately pauses and unloads while a StrictMode ref can reattach the same URL", () => {
  const { owner, calls, player } = audioFixture();
  owner.replace(audioSession(), () => true);
  owner.attach(player);
  owner.attach(null);
  assert.deepEqual(calls.slice(-3), ["pause", "remove:src", "load"]);
  owner.attach(player);
  assert.equal(player.src, "audiopreview://localhost/one");
  assert.equal(calls.filter((call) => call.startsWith("url:")).length, 1);
  owner.clear();
  assert.equal(calls.at(-1), "release:one");
});

test("late audio releases its token and a scope change during URL creation immediately releases it", () => {
  const fixture = audioFixture();
  assert.equal(
    fixture.owner.replace(audioSession(), () => false),
    null,
  );
  assert.deepEqual(fixture.calls, ["release:one"]);
  let attempts = 0;
  assert.equal(
    fixture.owner.replace(
      audioSession(),
      () => ++attempts === 1,
    ),
    null,
  );
  assert.deepEqual(fixture.calls, ["release:one", "url:one", "release:one"]);
  assert.equal(fixture.owner.url, null);
  assert.throws(
    () => fixture.owner.replace(audioSession("empty", 0), () => true),
    /信息无效/,
  );
  assert.throws(
    () =>
      fixture.owner.replace({...audioSession(), mime:"text/html"}, () => true),
    /信息无效/,
  );
});
test("audio rejects untrusted URLs and malformed tokens, and clear is idempotent", () => {
  const released=[];
  for(const url of ["blob:unsafe", "https://evil.test/one", "http://audiopreview.localhost.evil/one", "audiopreview://localhost/other", "audiopreview://localhost/one?x=1"]) {
    const owner=new AudioPreviewOwner({url:()=>url,release:token=>released.push(token)});
    assert.throws(()=>owner.replace(audioSession(),()=>true), /地址无效/);
    assert.equal(owner.url,null);
  }
  assert.equal(released.length,5);
  const {owner,calls}=audioFixture();
  assert.throws(()=>owner.replace(audioSession("../one"),()=>true), /会话无效/);
  owner.replace(audioSession(),()=>true);
  owner.clear(); owner.clear();
  assert.equal(calls.filter(call=>call==="release:one").length,1);
});

const noop = () => {};
test("audio release drain waits for completion and reports close failure without unhandled rejection", async () => {
  let finish;
  const owner = new AudioPreviewOwner({url:token=>`http://audiopreview.localhost/${token}`,release:()=>new Promise(resolve=>finish=resolve)});
  owner.replace(audioSession(),()=>true);
  owner.clear();
  let drained=false;
  const pending=owner.drain().then(()=>{drained=true;});
  await Promise.resolve();
  assert.equal(drained,false);
  finish(); await pending; assert.equal(drained,true);
  const failed = new AudioPreviewOwner({url:token=>`http://audiopreview.localhost/${token}`,release:async()=>{throw Error('close failed');}});
  failed.replace(audioSession(),()=>true);failed.clear();
  await assert.rejects(failed.drain(),/close failed/);
});
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

test("audio conversion uses explicit matching formats beside its read-only source", () => {
  assert.equal(defaultConvertedAudioName("录音/a.test.m4a"), "a.test_converted.wav");
  assert.equal(convertedAudioOutputPath("录音/a.m4a", "new.wav"), "录音/new.wav");
  for (const name of ["a.mp3", "../a.wav", "C:/a.wav", "a/b.wav", "NUL.wav"])
    assert.throws(() => convertedAudioOutputPath("录音/a.m4a", name));
  assert.throws(() => convertedAudioOutputPath("录音/a.wav", "a.wav"));
  for (const format of ["wav", "flac", "mp3", "m4a"]) {
    assert.equal(defaultConvertedAudioName("录音/a.m4a", format), `a_converted.${format}`);
    assert.equal(convertedAudioOutputPath("录音/a.wav", `custom.${format}`, format), `录音/custom.${format}`);
    assert.equal(replaceConvertedAudioExtension("我的录音.v2.wav", format), `我的录音.v2.${format}`);
    assert.equal(replaceConvertedAudioExtension("new name", format), `new name.${format}`);
  }
  assert.throws(() => convertedAudioOutputPath("录音/a.wav", "new.wav", "flac"), /所选格式一致/);
  assert.throws(() => convertedAudioOutputPath("录音/a.wav", "new.ogg", "ogg"), /支持的输出格式/);
});
test("conversion options preserve defaults and reject unsupported explicit encoder settings", () => {
  assert.deepEqual(validatedAudioConversionOptions("mp3"), {});
  assert.deepEqual(validatedAudioConversionOptions("wav", {sampleRate:undefined,channels:undefined}), {});
  assert.deepEqual(validatedAudioConversionOptions("mp3", {sampleRate:48000,channels:1}), {sampleRate:48000,channels:1});
  assert.deepEqual(validatedAudioConversionOptions("m4a", {sampleRate:96000,channels:2}), {sampleRate:96000,channels:2});
  assert.deepEqual(validatedAudioConversionOptions("flac", {sampleRate:192000}), {sampleRate:192000});
  for(const sampleRate of [0,7999,192001,NaN,Infinity,44100.5,"48000"]) assert.throws(()=>validatedAudioConversionOptions("wav",{sampleRate}));
  for(const channels of [0,3,-1,NaN,"1"]) assert.throws(()=>validatedAudioConversionOptions("wav",{channels}));
  for(const sampleRate of [88200,96000,176400,192000]) assert.throws(()=>validatedAudioConversionOptions("mp3",{sampleRate}), /重新选择/);
  for(const sampleRate of [176400,192000]) assert.throws(()=>validatedAudioConversionOptions("m4a",{sampleRate}), /重新选择/);
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
  assert.doesNotMatch(unloaded, /32 MiB/);
  assert.doesNotMatch(unloaded, /<audio/);
  const loaded = render({ selected, audioUrl: "audiopreview://localhost/local-only" });
  assert.equal((loaded.match(/<audio /g) ?? []).length, 1);
  assert.match(loaded, /src="audiopreview:\/\/localhost\/local-only"/);
  assert.match(loaded, /controls=""/);
  // Inline SVG's standard xmlns URL is an identifier, not a network request.
  assert.doesNotMatch(loaded, /auto[Pp]lay|iframe|(?:src|href)="https?:\/\//);
  const large = render({ filesEnabled: true, selected: entry({ bytes: 1024 * 1024 * 1024 }) });
  assert.doesNotMatch(large, /32 MiB|disabled=""[^>]*>加载音频/);
  assert.doesNotMatch(render({selected,audioUrl:"https://evil.test/one"}), /<audio/);
  assert.match(large, /<button>格式转换<\/button>/, "snapshot size does not disable conversion");
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

test("desktop CSP allows scoped audio protocol only for media without permitting inline scripts", () => {
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
  assert.match(directive("media-src"), /(?:^|\s)audiopreview:(?:\s|$)/);
  assert.match(directive("media-src"), /http:\/\/audiopreview\.localhost/);
  assert.doesNotMatch(directive("media-src"), /blob:|asset:/);
  assert.doesNotMatch(directive("connect-src"), /audiopreview/);
  assert.match(directive("script-src"), /'self'/);
  assert.doesNotMatch(directive("script-src"), /unsafe-inline|unsafe-eval/);
});
