import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

const require = createRequire(import.meta.url);
const {
  createPluginManagerController,
} = require("../../../build/desktop-model-tests/hooks/usePluginManager.js");
const Panel =
  require("../../../build/desktop-model-tests/components/PluginManagerPanel.js").default;

const entry = (change = {}) => ({
  folder: "example-01",
  plugin_id: "org.example.gain",
  plugin_version: "0.1.0",
  node_types: ["org.example.gain_v1"],
  enabled: true,
  pending_uninstall: false,
  active: true,
  can_manage: true,
  revision: "rev-1",
  error: null,
  ...change,
});
const view = (change = {}) => ({
  directory: "C:\\Plugins",
  restart_required: false,
  entries: [entry()],
  warnings: [],
  ...change,
});
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
};

test("install requires a selected folder and explicit native-code trust", async () => {
  const calls = [];
  let selected = null;
  let approved = false;
  const controller = createPluginManagerController({
    invoke: async (command, args) => {
      calls.push([command, args]);
      return view({ restart_required: true });
    },
    chooseDirectory: async () => selected,
    confirm: async () => approved,
  });
  assert.equal(await controller.install(), false);
  assert.equal(calls.length, 0);
  selected = "C:\\TrustedPlugin";
  assert.equal(await controller.install(), false);
  assert.equal(calls.length, 0);
  approved = true;
  assert.equal(await controller.install(), true);
  assert.deepEqual(calls, [["plugin_manager_install", {
    sourceDirectory: selected,
    trusted: true,
  }]]);
  assert.equal(controller.getState().view.restart_required, true);
  assert.equal(controller.getState().busy, false);
});

test("late list responses cannot overwrite a successful install or newer read", async () => {
  const oldRead = deferred();
  const firstCalls = [];
  const controller = createPluginManagerController({
    invoke: (command) => {
      firstCalls.push(command);
      return command === "plugin_manager_list"
        ? oldRead.promise
        : Promise.resolve(view({ entries: [entry({ revision: "rev-2" })] }));
    },
    chooseDirectory: async () => "C:\\TrustedPlugin",
    confirm: async () => true,
  });
  const pendingRead = controller.refresh();
  assert.equal(await controller.install(), true);
  oldRead.resolve(view({ entries: [] }));
  assert.equal(await pendingRead, false);
  assert.equal(controller.getState().view.entries[0].revision, "rev-2");
  assert.deepEqual(firstCalls, ["plugin_manager_list", "plugin_manager_install"]);

  const earlier = deferred(), later = deferred();
  let reads = 0;
  const second = createPluginManagerController({
    invoke: () => ++reads === 1 ? earlier.promise : later.promise,
    chooseDirectory: async () => null,
    confirm: async () => false,
  });
  const first = second.refresh();
  const newest = second.refresh();
  later.resolve(view({ entries: [entry({ revision: "new" })] }));
  assert.equal(await newest, true);
  earlier.resolve(view({ entries: [entry({ revision: "old" })] }));
  assert.equal(await first, false);
  assert.equal(second.getState().view.entries[0].revision, "new");
});

test("enable and uninstall use the displayed revision and do not repeat pending work", async () => {
  const calls = [];
  let approved = true;
  const controller = createPluginManagerController({
    invoke: async (command, args) => {
      calls.push([command, args]);
      if (command === "plugin_manager_list") return view();
      if (command === "plugin_manager_set_enabled" && args.revision === "rev-3")
        return view({ entries: [entry({ enabled: false, revision: "rev-4", pending_uninstall: false })], restart_required: true });
      if (command === "plugin_manager_set_enabled")
        return view({ entries: [entry({ enabled: false, revision: "rev-2" })], restart_required: true });
      return view({ entries: [entry({ enabled: false, revision: "rev-3", pending_uninstall: true })], restart_required: true });
    },
    chooseDirectory: async () => null,
    confirm: async () => approved,
  });
  assert.equal(await controller.refresh(), true);
  approved = false;
  assert.equal(await controller.setEnabled("example-01", "rev-1", false), false);
  assert.equal(calls.length, 1);
  approved = true;
  assert.equal(await controller.setEnabled("example-01", "stale", false), false);
  assert.equal(await controller.setEnabled("example-01", "rev-1", false), true);
  assert.deepEqual(calls[1], ["plugin_manager_set_enabled", {
    folder: "example-01", revision: "rev-1", enabled: false,
  }]);
  assert.equal(await controller.uninstall("example-01", "rev-2"), true);
  assert.deepEqual(calls[2], ["plugin_manager_uninstall", {
    folder: "example-01", revision: "rev-2",
  }]);
  assert.equal(await controller.uninstall("example-01", "rev-3"), false);
  assert.equal(calls.length, 3);
  approved = false;
  assert.equal(await controller.cancelUninstall("example-01", "rev-3"), false);
  assert.equal(calls.length, 3);
  approved = true;
  assert.equal(await controller.cancelUninstall("example-01", "rev-3"), true);
  assert.deepEqual(calls[3], ["plugin_manager_set_enabled", {
    folder: "example-01", revision: "rev-3", enabled: false,
  }]);
  assert.equal(controller.getState().view.entries[0].pending_uninstall, false);
  assert.equal(controller.getState().view.entries[0].enabled, false);
  assert.equal(controller.getState().view.entries[0].revision, "rev-4");
  assert.equal(await controller.cancelUninstall("example-01", "rev-3"), false);
  assert.equal(calls.length, 4);
});

test("an operation blocks concurrent writes while its confirmation is open", async () => {
  const confirmation = deferred();
  const calls = [];
  const controller = createPluginManagerController({
    invoke: async (command, args) => {
      calls.push([command, args]);
      return view();
    },
    chooseDirectory: async () => "C:\\Plugin",
    confirm: () => confirmation.promise,
  });
  const installing = controller.install();
  assert.equal(controller.getState().busy, true);
  assert.equal(await controller.install(), false);
  assert.equal(await controller.refresh(), false);
  confirmation.resolve(false);
  assert.equal(await installing, false);
  assert.equal(calls.length, 0);
  assert.equal(controller.getState().busy, false);
});

test("panel works without a workspace and separates pending changes from startup errors", () => {
  const manager = {
    ...view({
      restart_required: true,
      entries: [entry({ enabled: false, pending_uninstall: true, active: true })],
      warnings: ["部分目录无法读取"],
    }),
    view: view({
      restart_required: true,
      entries: [entry({ enabled: false, pending_uninstall: true, active: true })],
      warnings: ["部分目录无法读取"],
    }),
    loading: false,
    busy: false,
    error: "",
    notice: "",
    refresh: async () => true,
    install: async () => true,
    setEnabled: async () => true,
    uninstall: async () => true,
    cancelUninstall: async () => true,
  };
  const html = renderToStaticMarkup(
    React.createElement(Panel, {
      manager,
      desktop: true,
      startupErrors: [{ package: "bad", code: "abi", message: "本次启动未加载" }],
    }),
  );
  assert.match(html, /安装本机文件夹/);
  assert.match(html, /本次启动：已选用/);
  assert.match(html, /下次启动：待卸载/);
  assert.match(html, /本次启动的发现错误/);
  assert.match(html, /取消卸载/);
  assert.doesNotMatch(html, /卸载并保留备份/);
  assert.doesNotMatch(html, /打开工作区后/);
});
