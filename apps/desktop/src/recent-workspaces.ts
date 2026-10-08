export type RecentWorkspacesState = { paths: string[]; busy: boolean; warning: string };
export type RecentWorkspacesServices = {
  list: () => Promise<{ paths: string[] }>;
  remember: (sessionId: string) => Promise<{ paths: string[] }>;
  remove: (path: string) => Promise<{ paths: string[] }>;
  clear: () => Promise<{ paths: string[] }>;
};

export function workspaceFolderName(path: string) {
  const display = path.replace(/^\\\\\?\\/, "").replace(/^UNC\\/, "\\\\");
  return display.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || display;
}
export function workspaceDisplayPath(path: string) {
  return path.replace(/^\\\\\?\\UNC\\/, "\\\\").replace(/^\\\\\?\\/, "");
}

export function createRecentWorkspaces(services: RecentWorkspacesServices) {
  let state: RecentWorkspacesState = { paths: [], busy: false, warning: "" };
  let revision = 0;
  let initialized = false;
  let pending: Promise<void> = Promise.resolve();
  const listeners = new Set<() => void>();
  const update = (patch: Partial<RecentWorkspacesState>) => {
    state = { ...state, ...patch };
    listeners.forEach((listener) => listener());
  };
  const failure = (reason: unknown) => update({ warning: `最近工作区记录未更新：${reason instanceof Error ? reason.message : String(reason)}` });
  async function initialize() {
    if (initialized) return;
    initialized = true;
    const expected = revision;
    try {
      const value = await services.list();
      if (revision === expected) update({ paths: value.paths });
    } catch (reason) {
      if (revision === expected) failure(reason);
    }
  }
  function mutate(action: () => Promise<{ paths: string[] }>, queue: boolean) {
    if (state.busy && !queue) return Promise.resolve();
    revision++;
    update({ busy: true, warning: "" });
    const next = pending.then(async () => {
      try { update({ paths: (await action()).paths }); }
      catch (reason) { failure(reason); }
    });
    pending = next;
    return next.finally(() => { if (pending === next) update({ busy: false }); });
  }
  return {
    getSnapshot: () => state,
    subscribe: (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    initialize,
    remember: (sessionId: string) => mutate(() => services.remember(sessionId), true),
    remove: (path: string) => mutate(() => services.remove(path), false),
    clear: () => mutate(services.clear, false),
  };
}
