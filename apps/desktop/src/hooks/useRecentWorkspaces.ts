import { useEffect, useRef, useSyncExternalStore } from "react";
import { invokeDesktop } from "../api/desktop";
import { createRecentWorkspaces } from "../recent-workspaces";

export function useRecentWorkspaces(desktop: boolean) {
  const controllerRef = useRef<ReturnType<typeof createRecentWorkspaces> | null>(null);
  if (!controllerRef.current) controllerRef.current = createRecentWorkspaces({
    list: () => invokeDesktop("recent_workspaces_list"),
    remember: (sessionId) => invokeDesktop("recent_workspaces_remember", { sessionId }),
    remove: (path) => invokeDesktop("recent_workspaces_remove", { path }),
    clear: () => invokeDesktop("recent_workspaces_clear"),
  });
  const controller = controllerRef.current;
  const state = useSyncExternalStore(controller.subscribe, controller.getSnapshot, controller.getSnapshot);
  useEffect(() => { if (desktop) void controller.initialize(); }, [controller, desktop]);
  return { ...state, remember: controller.remember, remove: controller.remove, clear: controller.clear };
}
