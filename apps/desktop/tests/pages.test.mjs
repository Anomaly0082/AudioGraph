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
const NodeCatalog =
  require("../../../build/desktop-model-tests/components/NodeCatalog.js").default;
const {
  taskBlocksSubmission,
} = require("../../../build/desktop-model-tests/task-finalization.js");

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

test("external nodes use the ordinary catalog UI and retain their provenance", () => {
  const node = {
    typeId: "test.plugin.gain_v1",
    displayName: "External Gain",
    execution_domain: "synchronous",
    inputs: [{ id: "audio", type: "Audio", required: true }],
    outputs: [{ id: "audio", type: "Audio" }],
    parameters: [
      {
        id: "gain_db",
        type: "number",
        required: true,
        minimum: -24,
        maximum: 12,
      },
    ],
    plugin: {
      id: "test.plugin",
      implementation_version: "0.1.0",
      package_sha256: "a".repeat(64),
    },
  };
  const html = render(NodeCatalog, {
    nodes: [node],
    selectedId: node.typeId,
    search: "",
    onSearch: noop,
    onSelect: noop,
  });
  assert.match(html, /External Gain/);
  assert.match(html, /test.plugin/);
  assert.match(html, /0.1.0/);
  assert.match(html, /gain_db/);
});

test("plugin settings expose the selected directory and rejected package diagnostics without activation controls", () => {
  const settings = {
    config: { baseUrl: "", model: "", apiKey: "" },
    editing: false,
    settingsBusy: false,
    settingsReady: true,
    settingsError: "",
    settingsNotice: "",
    configStatus: "",
    editConfig: noop,
    saveCurrentSettings: noop,
    clearSavedSettings: noop,
  };
  const session = {
    allowDevices: false,
    allowMonitor: false,
    busy: null,
    setAllowDevices: noop,
    setAllowMonitor: noop,
    connection: {
      capabilities: {
        nodes: [],
        plugin_directory: "C:/test/plugins",
        plugins: {
          available: [{ plugin_id: "test.plugin", plugin_version: "0.1.0" }],
          errors: [
            {
              package: "bad-package",
              code: "plugin_invalid",
              message: "Invalid package hash",
            },
          ],
        },
      },
    },
  };
  const html = render(SettingsPage, { settings, session, workflowBusy: false });
  assert.match(html, /C:\/test\/plugins/);
  assert.match(html, /重启软件后生效/);
  assert.match(html, /test.plugin/);
  assert.match(html, /Invalid package hash/);
  assert.match(html, /role="alert"/);
  assert.doesNotMatch(html, /自动安装|立即启用|热加载/);
});

test("busy activity keeps the main pages accessible with a generic file page, not experiments", () => {
  const html = render(AppShell, {
    page: "editor",
    workspace: "C:/audio",
    onNavigate: noop,
    onWorkspace: noop,
    children: React.createElement("p", null, "draft-marker"),
  });
  const nav = html.match(/<nav aria-label="主导航">([\s\S]*?)<\/nav>/)?.[1];
  assert.ok(nav);
  assert.equal((nav.match(/<button /g) ?? []).length, 5);
  assert.match(nav, />文件<\/button>/);
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
  const html = render(AppShell, {
    page: "workbench",
    workspace: "\\\\?\\C:\\audio\\TestSwitch",
    onNavigate: noop,
    onWorkspace: noop,
  });
  assert.match(html, /工作区：TestSwitch/);
  assert.ok(html.includes('title="C:\\audio\\TestSwitch"'));
  assert.doesNotMatch(html, /工作区已打开|status-dot|<footer/);
  const disconnected = render(AppShell, {
    page: "workbench",
    workspace: null,
    onNavigate: noop,
    onWorkspace: noop,
  });
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
  assert.match(html, /查看运行记录/);
  assert.doesNotMatch(html, /graph-connections|graph-edge-row/);
  assert.match(html, /中键拖动平移/);
});

test("editor submission follows task ownership and release, not the presence of a retained task", () => {
  const draft = {
    mode: "offline",
    template: "text",
    graphText: JSON.stringify(graph),
    fileLabel: "draft.json",
    blockFrames: "256",
    duration: "10",
    probe: true,
    validationCurrent: true,
    localGraph: { graph, error: "" },
    setTemplate: noop,
    editGraph: noop,
    setMode: noop,
    setBlockFrames: noop,
    setDuration: noop,
    setProbe: noop,
  };
  const task = (state, released = false, sessionId = "session-a") => ({
    id: "task-1",
    sessionId,
    state,
    released,
    resultRead: released,
    errors:
      state === "failed"
        ? [{ code: "output_exists", node_id: "output", parameter_id: "path" }]
        : undefined,
    submission: { mode: "offline", graph, options: {} },
  });
  const cases = [
    [null, false],
    ...["queued", "running", "cancelling", "unknown"].map((state) => [
      task(state),
      true,
    ]),
    ...["succeeded", "failed", "cancelled"].flatMap((state) => [
      [task(state), true],
      [task(state, true), false],
    ]),
    [task("running", false, "old-session"), false],
  ];
  function editor(current, changes = {}) {
    return render(EditorPage, {
      draft,
      session: {
        desktop: true,
        busy: null,
        connection: { sessionId: "session-a", capabilities: { nodes: [] } },
        devices: { inputs: [], outputs: [] },
        task: current,
        taskBlocked: taskBlocksSubmission(current, "session-a"),
      },
      selection: { search: "" },
      submittingLocked: false,
      onSelection: noop,
      onValidate: noop,
      onRun: noop,
      onLoadTemplate: noop,
      onLoad: noop,
      onSave: noop,
      onDevices: noop,
      onTasks: noop,
      ...changes,
    });
  }
  function disabled(html) {
    const button = html.match(/<button\b([^>]*)>提交任务<\/button>/);
    assert.ok(button, "Submit button must exist");
    return /\bdisabled(?:=|\s|$)/.test(button[1]);
  }
  for (const [current, blocked] of cases) {
    const html = editor(current);
    assert.equal(disabled(html), blocked, JSON.stringify(current));
    if (current)
      assert.match(html, /查看运行记录/, "Retained history remains accessible");
  }
  const releasedFailure = task("failed", true);
  assert.equal(
    disabled(editor(releasedFailure, { submittingLocked: true })),
    true,
  );
  assert.equal(
    disabled(
      editor(releasedFailure, {
        draft: { ...draft, validationCurrent: false },
      }),
    ),
    true,
  );
  assert.equal(
    disabled(
      editor(releasedFailure, {
        draft: { ...draft, localGraph: { graph, error: "invalid" } },
      }),
    ),
    true,
  );
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
  assert.doesNotMatch(html, /数据保存范围|草稿、AI提案和当前任务在切页时保留/);
});
