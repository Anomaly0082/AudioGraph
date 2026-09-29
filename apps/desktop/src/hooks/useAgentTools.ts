import { useEffect, useRef, useState } from "react";
import { createAiRequestId, normalizeAiConfig } from "../ai-model";
import { invokeDesktop } from "../api/desktop";
import { parseAgentReply } from "../agent-model";
import { formatError } from "../model";
import type {
  AgentDraftContext,
  AgentMode,
  AgentReply,
  AgentSpaceInfo,
} from "../types/agent";
import type { Connection } from "../types/desktop";
import type { AiSettings } from "./useAiSettings";

export type AgentTurn = {
  id: string;
  prompt: string;
  reply?: AgentReply;
  error?: string;
};
type Options = {
  connection: Connection | null;
  settings: AiSettings;
  blocked: () => boolean;
};
export function useAgentTools({ connection, settings, blocked }: Options) {
  const [mode, setModeState] = useState<AgentMode>("graph");
  const [turns, setTurns] = useState<Record<AgentMode, AgentTurn[]>>({
    graph: [],
    workflow: [],
  });
  const [spaces, setSpaces] = useState<AgentSpaceInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [prompts, setPrompts] = useState<Record<AgentMode, string>>({
    graph: "",
    workflow: "",
  });
  const [attachGraph, setAttachGraph] = useState(true);
  const lock = useRef(false);
  const mounted = useRef(true);
  const epoch = useRef(0);
  const request = useRef<{
    id: string;
    sessionId: string;
    mode: AgentMode;
  } | null>(null);
  const latest = useRef({ connection, settings, blocked, mode });
  latest.current = { connection, settings, blocked, mode };
  const matches = (id: string, sessionId: string) =>
    mounted.current &&
    request.current?.id === id &&
    latest.current.connection?.sessionId === sessionId;
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      if (request.current)
        void invokeDesktop("agent_cancel", {
          requestId: request.current.id,
        }).catch(() => undefined);
    };
  }, []);
  useEffect(() => {
    const token = ++epoch.current;
    setSpaces(null);
    setError("");
    if (!connection) {
      setLoading(false);
      return;
    }
    setLoading(true);
    void invokeDesktop<AgentSpaceInfo>("agent_spaces", {
      sessionId: connection.sessionId,
      mode,
    })
      .then((value) => {
        if (mounted.current && token === epoch.current) setSpaces(value);
      })
      .catch((reason) => {
        if (mounted.current && token === epoch.current)
          setError(formatError(reason));
      })
      .finally(() => {
        if (mounted.current && token === epoch.current) setLoading(false);
      });
  }, [connection?.sessionId, mode]);
  useEffect(() => {
    setTurns({ graph: [], workflow: [] });
    setPrompts({ graph: "", workflow: "" });
    if (request.current && request.current.sessionId !== connection?.sessionId)
      void invokeDesktop("agent_cancel", {
        requestId: request.current.id,
      }).catch(() => undefined);
  }, [connection?.sessionId]);
  function setMode(value: AgentMode) {
    if (!lock.current) setModeState(value);
  }
  function setPrompt(value: string) {
    setPrompts((old) => ({ ...old, [mode]: value }));
  }
  async function send(context?: AgentDraftContext) {
    if (lock.current) return;
    const current = latest.current;
    if (!current.connection || current.blocked() || loading || !spaces) {
      setError("请先打开工作区并结束其他操作。");
      return;
    }
    if (settings.settingsLoading || settings.settingsBusy) {
      setError("模型配置尚未就绪。");
      return;
    }
    const prompt = prompts[mode].trim();
    if (!prompt) return;
    if (new TextEncoder().encode(prompt).length > 16 * 1024) {
      setError("消息不能超过16KiB。");
      return;
    }
    let config;
    try {
      config = normalizeAiConfig(settings.config);
    } catch (reason) {
      setError(formatError(reason));
      return;
    }
    const id = createAiRequestId(),
      sessionId = current.connection.sessionId,
      turnMode = mode;
    const contextGraph = context ? structuredClone(context) : undefined;
    lock.current = true;
    request.current = { id, sessionId, mode: turnMode };
    setBusy(true);
    setStopping(false);
    setError("");
    setTurns((old) => ({
      ...old,
      [turnMode]: [...old[turnMode], { id, prompt }],
    }));
    setPrompts((old) => ({ ...old, [turnMode]: "" }));
    try {
      const value = await invokeDesktop<unknown>("agent_turn", {
        sessionId,
        requestId: id,
        mode: turnMode,
        prompt,
        config,
        contextGraph,
      });
      if (!matches(id, sessionId)) return;
      const reply = parseAgentReply(value, id);
      setTurns((old) => ({
        ...old,
        [turnMode]: old[turnMode].map((turn) =>
          turn.id === id ? { ...turn, reply } : turn,
        ),
      }));
    } catch (reason) {
      if (matches(id, sessionId)) {
        const message = formatError(reason);
        setError(message);
        setTurns((old) => ({
          ...old,
          [turnMode]: old[turnMode].map((turn) =>
            turn.id === id ? { ...turn, error: message } : turn,
          ),
        }));
      }
    } finally {
      if (request.current?.id === id) {
        request.current = null;
        lock.current = false;
        if (mounted.current) {
          setBusy(false);
          setStopping(false);
        }
      }
    }
  }
  async function stop() {
    const active = request.current;
    if (!active) return;
    setStopping(true);
    try {
      await invokeDesktop("agent_cancel", { requestId: active.id });
    } catch (reason) {
      if (matches(active.id, active.sessionId)) {
        setError(`停止请求失败：${formatError(reason)}`);
        setStopping(false);
      }
    }
  }
  async function reset() {
    if (lock.current || latest.current.blocked() || !connection) return;
    lock.current = true;
    setBusy(true);
    setError("");
    const sessionId = connection.sessionId,
      selectedMode = mode;
    try {
      await invokeDesktop("agent_reset", { sessionId, mode: selectedMode });
      if (mounted.current && latest.current.connection?.sessionId === sessionId)
        setTurns((old) => ({ ...old, [selectedMode]: [] }));
    } catch (reason) {
      if (mounted.current) setError(formatError(reason));
    } finally {
      lock.current = false;
      if (mounted.current) setBusy(false);
    }
  }
  return {
    mode,
    setMode,
    turns: turns[mode],
    spaces,
    busy,
    canStop: request.current !== null,
    stopping,
    loading,
    error,
    setError,
    prompt: prompts[mode],
    setPrompt,
    attachGraph,
    setAttachGraph,
    send,
    stop,
    reset,
  };
}
export type AgentTools = ReturnType<typeof useAgentTools>;
