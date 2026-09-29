import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

const require = createRequire(import.meta.url);
const AppShell =
  require("../../../build/desktop-model-tests/components/AppShell.js").default;
const WorkbenchPage =
  require("../../../build/desktop-model-tests/pages/WorkbenchPage.js").default;
const EditorPage =
  require("../../../build/desktop-model-tests/pages/EditorPage.js").default;
const TasksPage =
  require("../../../build/desktop-model-tests/pages/TasksPage.js").default;
const SettingsPage =
  require("../../../build/desktop-model-tests/pages/SettingsPage.js").default;

const render = (Component, props) =>
  renderToStaticMarkup(React.createElement(Component, props));
const noop = () => {};
const graph = {
  schema_version: 1,
  nodes: [
    {
      id: "draft-node",
      type: "text_input",
      parameters: { text: "draft-marker" },
    },
  ],
  connections: [],
};

test("busy activity keeps four main pages accessible and experiments are not a main page", () => {
  const html = render(AppShell, {
    page: "editor",
    workspace: "C:/audio",
    onNavigate: noop,
    onWorkspace: noop,
    children: React.createElement("p", null, "draft-marker"),
  });
  const nav = html.match(/<nav aria-label="主导航">([\s\S]*?)<\/nav>/)?.[1];
  assert.ok(nav);
  assert.equal((nav.match(/<button /g) ?? []).length, 4);
  assert.match(nav, /运行记录/);
  assert.doesNotMatch(nav, /参数对比|任务与结果/);
  assert.doesNotMatch(nav, /disabled/);
  assert.match(nav, /aria-current="page"/);
  assert.match(html, /draft-marker/);
  assert.doesNotMatch(html, /app-footer|<footer|就绪/);
  assert.match(html, /工作区：audio/);
  assert.match(html, /title="C:\/audio"/);
  assert.doesNotMatch(html, /方案与任务分开|Node · Graph · Executor/);
});

test("workspace uses one compact entry with a readable full path tooltip", () => {
  const html = render(AppShell, { page: "workbench", workspace: "\\\\?\\C:\\audio\\TestSwitch",
    onNavigate: noop, onWorkspace: noop });
  assert.match(html, /工作区：TestSwitch/);
  assert.ok(html.includes('title="C:\\audio\\TestSwitch"'));
  assert.doesNotMatch(html, /工作区已打开|status-dot|<footer/);
  const disconnected = render(AppShell, { page: "workbench", workspace: null,
    onNavigate: noop, onWorkspace: noop });
  assert.match(disconnected, />打开工作区<\/button>/);
});

test("workbench keeps only the tool assistant and explicit view buttons", () => {
  const input = {
    path: "sample.wav",
    inspection: null,
    busy: false,
    error: "",
    setPath: noop,
    choose: noop,
    check: noop,
  };
  const agent = {
    mode: "graph",
    busy: false,
    canStop: false,
    loading: false,
    error: "",
    spaces: {
      user_root: "C:/audio",
      ai_root: "C:/managed",
      tools: ["nodes_list"],
    },
    turns: [
      {
        id: "r1",
        prompt: "请处理 sample.wav",
        reply: {
          state: "completed",
          text: "文件已写入",
          model_calls: 2,
          tool_calls: 1,
          events: [
            {
              kind: "tool",
              tool: "file_write_text",
              success: true,
              arguments: { path: "g.json", content: JSON.stringify(graph) },
            },
          ],
        },
      },
    ],
    prompt: "",
    attachGraph: true,
    setMode: noop,
    reset: noop,
    setAttachGraph: noop,
    setPrompt: noop,
  };
  const html = render(WorkbenchPage, {
    input,
    connected: true,
    desktop: true,
    agent,
    agentBlocked: false,
    setup: {
      approach: "ai",
      preset: "wav",
      output: "processed.wav",
      gain: "-6",
    },
    onSetup: noop,
    onPrepare: noop,
    onOpenSettings: noop,
    onAgentSend: noop,
    onAgentApply: noop,
  });
  assert.match(html, /工具助手/);
  assert.match(html, /请处理 sample.wav/);
  assert.match(html, /载入 Graph 编辑器/);
  assert.match(html, /查看工作区与可用工具/);
  assert.doesNotMatch(
    html,
    /单次 Graph 提案|待确认提案|<details|<summary|API Key|apiKey/,
  );
});

test("editor displays its independent draft while a task owns a different submission", () => {
  const draft = {
    mode: "offline",
    template: "text",
    graphText: JSON.stringify(graph),
    fileLabel: "draft.json",
    blockFrames: "256",
    duration: "10",
    probe: true,
    validationCurrent: false,
    localGraph: { graph, error: "" },
    setTemplate: noop,
    editGraph: noop,
    setMode: noop,
    setBlockFrames: noop,
    setDuration: noop,
    setProbe: noop,
  };
  const submittedGraph = structuredClone(graph);
  submittedGraph.nodes[0].parameters.text = "submitted-marker";
  const session = {
    desktop: true,
    busy: null,
    connection: null,
    devices: { inputs: [], outputs: [] },
    task: {
      id: "task-1",
      sessionId: "session-a",
      state: "running",
      submission: { mode: "offline", graph: submittedGraph, options: {} },
    },
  };
  const html = render(EditorPage, {
    draft,
    session,
    selection: { search: "", nodeId: "", inputDevice: "", outputDevice: "" },
    onSelection: noop,
    submittingLocked: false,
    onValidate: noop,
    onRun: noop,
    onLoadTemplate: noop,
    onLoad: noop,
    onSave: noop,
    onDevices: noop,
    onApplyDevices: noop,
    onTasks: noop,
  });
  assert.match(html, /draft-marker/);
  assert.doesNotMatch(html, /submitted-marker/);
  assert.match(html, /查看现有任务/);
  assert.doesNotMatch(html, /graph-connections|graph-edge-row/);
  assert.match(html, /中键拖动平移/);
});

test("tasks page presents the submitted snapshot and authoritative output", () => {
  const submittedGraph = structuredClone(graph);
  submittedGraph.nodes[0].parameters.text = "submitted-marker";
  const session = {
    connection: { sessionId: "session-a", capabilities: { nodes: [] } },
    busy: null,
    taskActive: false,
    task: {
      id: "task-1",
      sessionId: "session-a",
      state: "succeeded",
      submission: { mode: "offline", graph: submittedGraph, options: {} },
      result: {
        outputs: { file: { type: "FilePath", value: "processed.wav" } },
      },
    },
  };
  const html = render(TasksPage, {
    session,
    aiBusy: false,
    onCancel: noop,
    onRelease: noop,
    onWorkbench: noop,
    onEditor: noop,
    onCopy: noop,
  });
  assert.match(html, /submitted-marker/);
  assert.match(html, /固定快照/);
  assert.match(html, /processed.wav/);
  assert.match(html, /释放记录，准备新任务/);
  assert.doesNotMatch(html, /draft-marker/);
});

test("settings page alone renders API Key as a password input", () => {
  const key = "SECRET_TEST_KEY_928";
  const settings = {
    config: {
      baseUrl: "https://example.test/v1",
      model: "model-a",
      apiKey: key,
    },
    settingsPath: "C:/config.json",
    settingsLoading: false,
    settingsBusy: false,
    settingsError: "",
    settingsNotice: "",
    configStatus: "配置已保存",
    editConfig: noop,
    saveCurrentSettings: noop,
    clearSavedSettings: noop,
  };
  const session = {
    allowDevices: false,
    allowMonitor: false,
    connection: null,
    busy: null,
    setAllowDevices: noop,
    setAllowMonitor: noop,
  };
  const html = render(SettingsPage, {
    settings,
    session,
    workflowBusy: false,
  });
  assert.match(html, /API Key/);
  assert.match(html, /type="password"/);
  assert.match(html, /SECRET_TEST_KEY_928/);
  assert.match(html, /允许音频设备访问/);
});
