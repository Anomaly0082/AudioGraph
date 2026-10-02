export type PluginManagerEntry = {
  folder: string;
  plugin_id: string | null;
  plugin_version: string | null;
  node_types: string[];
  enabled: boolean;
  pending_uninstall: boolean;
  active: boolean;
  can_manage: boolean;
  revision: string;
  error: string | null;
};

export type PluginManagerView = {
  directory: string;
  restart_required: boolean;
  entries: PluginManagerEntry[];
  warnings: string[];
};

export type PluginManagerOperation = "install" | "enable" | "disable" | "uninstall" | "cancel-uninstall";

export type PluginManagerState = {
  view: PluginManagerView | null;
  loading: boolean;
  busy: boolean;
  operation: PluginManagerOperation | null;
  error: string;
  notice: string;
};

export function sameManageableEntry(
  view: PluginManagerView | null,
  folder: string,
  revision: string,
): boolean {
  const entry = view?.entries.find((item) => item.folder === folder);
  return !!entry && entry.revision === revision && entry.can_manage && !entry.pending_uninstall;
}

export function samePendingUninstallEntry(
  view: PluginManagerView | null,
  folder: string,
  revision: string,
): boolean {
  const entry = view?.entries.find((item) => item.folder === folder);
  return !!entry && entry.revision === revision && entry.can_manage && entry.pending_uninstall;
}
