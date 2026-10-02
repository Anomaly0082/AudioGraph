import { useEffect, useRef, useState } from "react";
import { formatError } from "../model";
import {
  sameManageableEntry,
  samePendingUninstallEntry,
  type PluginManagerEntry,
  type PluginManagerOperation,
  type PluginManagerState,
  type PluginManagerView,
} from "../types/plugin-manager";

export type PluginManagerServices = {
  invoke: <T>(command: string, args: Record<string, unknown>) => Promise<T>;
  chooseDirectory: () => Promise<string | null>;
  confirm: (
    message: string,
    title: string,
    kind?: "warning" | "info",
  ) => Promise<boolean>;
};

export const initialPluginManagerState: PluginManagerState = {
  view: null,
  loading: false,
  busy: false,
  operation: null,
  error: "",
  notice: "",
};

export function createPluginManagerController(
  initialServices: PluginManagerServices,
  onChange: (state: PluginManagerState) => void = () => {},
) {
  let services = initialServices;
  let state = initialPluginManagerState;
  let mounted = true;
  let readToken = 0;
  let mutation = 0;
  let operationToken = 0;

  function update(change: Partial<PluginManagerState>) {
    state = { ...state, ...change };
    if (mounted) onChange(state);
  }
  function acceptView(view: PluginManagerView) {
    if (
      !view ||
      typeof view.directory !== "string" ||
      typeof view.restart_required !== "boolean" ||
      !Array.isArray(view.entries) ||
      !Array.isArray(view.warnings)
    )
      throw new Error("插件管理响应格式无效，请重试。");
    update({ view });
  }
  async function refresh(): Promise<boolean> {
    if (state.busy) return false;
    const token = ++readToken;
    const version = mutation;
    update({ loading: true, error: "" });
    try {
      const view = await services.invoke<PluginManagerView>(
        "plugin_manager_list",
        {},
      );
      if (token !== readToken || mutation !== version || state.busy) return false;
      acceptView(view);
      return true;
    } catch (reason) {
      if (token === readToken && mutation === version && !state.busy)
        update({ error: `读取插件列表失败：${formatError(reason)}` });
      return false;
    } finally {
      if (token === readToken && mutation === version && !state.busy)
        update({ loading: false });
    }
  }
  function begin(operation: PluginManagerOperation): number | null {
    if (state.busy) return null;
    ++readToken; // Any earlier list response can no longer overwrite a write.
    ++mutation;
    const token = ++operationToken;
    update({ busy: true, loading: false, operation, error: "", notice: "" });
    return token;
  }
  function end(token: number) {
    if (token === operationToken)
      update({ busy: false, operation: null });
  }
  async function install(): Promise<boolean> {
    const token = begin("install");
    if (token === null) return false;
    try {
      const sourceDirectory = await services.chooseDirectory();
      if (!sourceDirectory) return false;
      const trusted = await services.confirm(
        "仅安装你信任的本机原生插件。插件运行时可自行访问文件和网络。确认安装此完整文件夹？",
        "安装本机插件",
        "warning",
      );
      if (!trusted || token !== operationToken) return false;
      const view = await services.invoke<PluginManagerView>(
        "plugin_manager_install",
        { sourceDirectory, trusted: true },
      );
      acceptView(view);
      update({ notice: "插件已安装。重启软件后按启用状态加载。" });
      return true;
    } catch (reason) {
      update({ error: `安装插件失败：${formatError(reason)}` });
      return false;
    } finally {
      end(token);
    }
  }
  function target(folder: string, revision: string): PluginManagerEntry | null {
    const entry = state.view?.entries.find((item) => item.folder === folder);
    return entry && sameManageableEntry(state.view, folder, revision)
      ? entry
      : null;
  }
  async function setEnabled(
    folder: string,
    revision: string,
    enabled: boolean,
  ): Promise<boolean> {
    const original = target(folder, revision);
    if (!original || original.enabled === enabled) return false;
    const token = begin(enabled ? "enable" : "disable");
    if (token === null) return false;
    try {
      const confirmed = await services.confirm(
        enabled
          ? "下次启动时启用此原生插件？当前 Graph 和任务不会改变。"
          : "下次启动时停用此插件？当前 Graph 和任务不会改变。",
        enabled ? "启用插件" : "停用插件",
      );
      if (!confirmed) return false;
      const current = target(folder, revision);
      if (!current || current.enabled !== original.enabled || token !== operationToken) {
        update({ error: "插件状态已改变，请刷新后重试。" });
        return false;
      }
      const view = await services.invoke<PluginManagerView>(
        "plugin_manager_set_enabled",
        { folder, revision, enabled },
      );
      acceptView(view);
      update({ notice: enabled ? "已设为下次启动启用。" : "已设为下次启动停用。" });
      return true;
    } catch (reason) {
      update({ error: `更新插件状态失败：${formatError(reason)}` });
      return false;
    } finally {
      end(token);
    }
  }
  async function uninstall(folder: string, revision: string): Promise<boolean> {
    const original = target(folder, revision);
    if (!original) return false;
    const token = begin("uninstall");
    if (token === null) return false;
    try {
      const confirmed = await services.confirm(
        "将此插件移出安装目录并保留可恢复备份？音频文件、历史记录和插件数据会保留；重启后生效。",
        "卸载插件",
        "warning",
      );
      if (!confirmed) return false;
      if (!target(folder, revision) || token !== operationToken) {
        update({ error: "插件状态已改变，请刷新后重试。" });
        return false;
      }
      const view = await services.invoke<PluginManagerView>(
        "plugin_manager_uninstall",
        { folder, revision },
      );
      acceptView(view);
      update({ notice: "已安排卸载；重启后生效，音频历史和插件数据保留。" });
      return true;
    } catch (reason) {
      update({ error: `卸载插件失败：${formatError(reason)}` });
      return false;
    } finally {
      end(token);
    }
  }
  async function cancelUninstall(folder: string, revision: string): Promise<boolean> {
    if (!samePendingUninstallEntry(state.view, folder, revision)) return false;
    const token = begin("cancel-uninstall");
    if (token === null) return false;
    try {
      const confirmed = await services.confirm(
        "取消待处理的卸载？插件会保持停用，当前 Graph 和任务不会改变。",
        "取消卸载",
        "info",
      );
      if (!confirmed) return false;
      if (!samePendingUninstallEntry(state.view, folder, revision) || token !== operationToken) {
        update({ error: "插件状态已改变，请刷新后重试。" });
        return false;
      }
      const view = await services.invoke<PluginManagerView>(
        "plugin_manager_set_enabled",
        { folder, revision, enabled: false },
      );
      acceptView(view);
      update({ notice: "已取消卸载；插件保持停用。" });
      return true;
    } catch (reason) {
      update({ error: `取消卸载失败：${formatError(reason)}` });
      return false;
    } finally {
      end(token);
    }
  }
  return {
    getState: () => state,
    setServices: (next: PluginManagerServices) => { services = next; },
    setMounted: (next: boolean) => { mounted = next; if (next) onChange(state); },
    refresh,
    install,
    setEnabled,
    uninstall,
    cancelUninstall,
  };
}

export function usePluginManager(
  services: PluginManagerServices,
  available: boolean,
) {
  const [state, setState] = useState<PluginManagerState>(initialPluginManagerState);
  const controller = useRef<ReturnType<typeof createPluginManagerController> | null>(null);
  if (!controller.current)
    controller.current = createPluginManagerController(services, setState);
  controller.current.setServices(services);
  useEffect(() => {
    controller.current?.setMounted(true);
    if (available) void controller.current?.refresh();
    return () => controller.current?.setMounted(false);
  }, [available]);
  return {
    ...state,
    refresh: controller.current.refresh,
    install: controller.current.install,
    setEnabled: controller.current.setEnabled,
    uninstall: controller.current.uninstall,
    cancelUninstall: controller.current.cancelUninstall,
  };
}

export type PluginManager = ReturnType<typeof usePluginManager>;
