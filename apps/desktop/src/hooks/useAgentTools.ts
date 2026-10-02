import { useEffect, useRef, useState } from "react";
import { createAiRequestId, normalizeAiConfig } from "../ai-model";
import { invokeDesktop } from "../api/desktop";
import { parseAgentReply } from "../agent-model";
import { formatError } from "../model";
import {
  matchesConversationScope,
  mergeOlderTurns,
  parseConversation,
  parseConversationList,
  readConversationPage,
  reconcileConversation,
  type ConversationDetail,
  type ConversationSummary,
  type ConversationTurn,
} from "../conversation-model";
import type {
  AgentDraftContext,
  AgentMode,
  AgentSpaceInfo,
} from "../types/agent";
import type { Connection } from "../types/desktop";
import type { AiSettings } from "./useAiSettings";

export type AgentTurn = ConversationTurn;
type Options = {
  connection: Connection | null;
  settings: AiSettings;
  blocked: () => boolean;
};

export function useAgentTools({ connection, settings, blocked }: Options) {
  const [mode, setModeState] = useState<AgentMode>("graph");
  const [conversation, setConversation] = useState<ConversationDetail | null>(
    null,
  );
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [turns, setTurns] = useState<AgentTurn[]>([]);
  const [warnings, setWarnings] = useState<string[]>([]);
  const [spaces, setSpaces] = useState<AgentSpaceInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [spacesLoading, setSpacesLoading] = useState(false);
  const [conversationLoading, setConversationLoading] = useState(false);
  const [error, setError] = useState("");
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [attachGraph, setAttachGraph] = useState(true);
  const lock = useRef(false),
    mounted = useRef(true),
    epoch = useRef(0),
    spacesEpoch = useRef(0);
  const selected = useRef<string | null>(null);
  const modeRef = useRef<AgentMode>(mode);
  const request = useRef<{
    id: string;
    sessionId: string;
    conversationId: string;
    mode: AgentMode;
    epoch: number;
  } | null>(null);
  const latest = useRef({ connection, settings, blocked });
  latest.current = { connection, settings, blocked };
  const draftKey = conversation?.id ?? "new-" + mode;
  const prompt = drafts[draftKey] ?? "";
  const currentScope = () => ({
    sessionId: latest.current.connection?.sessionId,
    conversationId: selected.current,
    mode: modeRef.current,
    epoch: epoch.current,
  });
  const matches = (binding: NonNullable<typeof request.current>) =>
    mounted.current &&
    request.current?.id === binding.id &&
    matchesConversationScope(currentScope(), binding);
  const sameWorkspace = (sessionId: string, token: number) =>
    mounted.current &&
    latest.current.connection?.sessionId === sessionId &&
    epoch.current === token;

  function applyConversation(value: ConversationDetail) {
    selected.current = value.id;
    modeRef.current = value.mode;
    setModeState(value.mode);
    setConversation(value);
    setTurns(value.turns);
  }
  async function list(sessionId: string) {
    return parseConversationList(
      await invokeDesktop("conversation_list", { sessionId }),
    );
  }
  function acceptList(value: ReturnType<typeof parseConversationList>) {
    setConversations(value.records);
    setWarnings([
      ...value.warnings,
      ...(value.truncated ? ["仅显示部分会话，其余记录仍保留在本地。"] : []),
    ]);
  }

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
    const token = ++spacesEpoch.current;
    setSpaces(null);
    if (!connection) {
      setSpacesLoading(false);
      return;
    }
    setSpacesLoading(true);
    void invokeDesktop<AgentSpaceInfo>("agent_spaces", {
      sessionId: connection.sessionId,
      mode,
    })
      .then((value) => {
        if (mounted.current && token === spacesEpoch.current) setSpaces(value);
      })
      .catch((reason) => {
        if (mounted.current && token === spacesEpoch.current)
          setError(formatError(reason));
      })
      .finally(() => {
        if (mounted.current && token === spacesEpoch.current)
          setSpacesLoading(false);
      });
  }, [connection?.sessionId, mode]);
  useEffect(() => {
    const token = ++epoch.current;
    selected.current = null;
    setConversation(null);
    setConversations([]);
    setTurns([]);
    setWarnings([]);
    setDrafts({});
    setError("");
    if (request.current && request.current.sessionId !== connection?.sessionId)
      void invokeDesktop("agent_cancel", {
        requestId: request.current.id,
      }).catch(() => undefined);
    if (!connection) {
      setConversationLoading(false);
      return;
    }
    const sessionId = connection.sessionId;
    setConversationLoading(true);
    // Reopening only reads saved data. It never submits an AI turn.
    void (async () => {
      const value = await list(sessionId);
      if (!sameWorkspace(sessionId, token)) return;
      acceptList(value);
      const recent = value.records[0];
      if (recent) {
        const detail = await readConversationPage(
          invokeDesktop,
          sessionId,
          recent.id,
          recent.mode,
        );
        if (sameWorkspace(sessionId, token)) applyConversation(detail);
      } else {
        modeRef.current = "graph";
        setModeState("graph");
      }
    })()
      .catch((reason) => {
        if (sameWorkspace(sessionId, token)) setError(formatError(reason));
      })
      .finally(() => {
        if (sameWorkspace(sessionId, token)) setConversationLoading(false);
      });
  }, [connection?.sessionId]);

  async function selectConversation(id: string) {
    const target = conversations.find((item) => item.id === id);
    const sessionId = latest.current.connection?.sessionId;
    if (lock.current || conversationLoading || !sessionId || !target) return;
    lock.current = true;
    const token = ++epoch.current;
    setConversationLoading(true);
    setError("");
    try {
      const value = await readConversationPage(
        invokeDesktop,
        sessionId,
        id,
        target.mode,
      );
      if (sameWorkspace(sessionId, token)) applyConversation(value);
    } catch (reason) {
      if (sameWorkspace(sessionId, token)) setError(formatError(reason));
    } finally {
      lock.current = false;
      if (sameWorkspace(sessionId, token)) setConversationLoading(false);
    }
  }
  function setMode(value: AgentMode) {
    if (lock.current || conversationLoading || value === modeRef.current)
      return;
    const recent = conversations.find((item) => item.mode === value);
    if (recent) {
      void selectConversation(recent.id);
      return;
    }
    ++epoch.current;
    selected.current = null;
    modeRef.current = value;
    setModeState(value);
    setConversation(null);
    setTurns([]);
    setError("");
  }
  async function create(sessionId: string, chosenMode: AgentMode) {
    return parseConversation(
      await invokeDesktop("conversation_create", {
        sessionId,
        mode: chosenMode,
      }),
      undefined,
      chosenMode,
    );
  }
  async function newConversation() {
    const sessionId = latest.current.connection?.sessionId;
    if (lock.current || conversationLoading || !sessionId) return;
    lock.current = true;
    const token = ++epoch.current,
      chosenMode = modeRef.current;
    setConversationLoading(true);
    setError("");
    try {
      const value = await create(sessionId, chosenMode);
      if (!sameWorkspace(sessionId, token)) return;
      applyConversation(value);
      // Keep an unsent/recovered prompt available when starting a fresh conversation.
      setDrafts((old) => ({ ...old, [value.id]: old[draftKey] ?? "" }));
      setConversations((old) => [
        value,
        ...old.filter((item) => item.id !== value.id),
      ]);
    } catch (reason) {
      if (sameWorkspace(sessionId, token)) setError(formatError(reason));
    } finally {
      lock.current = false;
      if (sameWorkspace(sessionId, token)) setConversationLoading(false);
    }
  }
  async function loadOlder() {
    const sessionId = latest.current.connection?.sessionId;
    if (
      lock.current ||
      conversationLoading ||
      !sessionId ||
      !conversation?.has_more ||
      conversation.before == null
    )
      return;
    lock.current = true;
    const binding = {
      sessionId,
      conversationId: conversation.id,
      mode: conversation.mode,
      epoch: epoch.current,
    };
    setConversationLoading(true);
    setError("");
    try {
      const older = await readConversationPage(
        invokeDesktop,
        sessionId,
        conversation.id,
        conversation.mode,
        conversation.before,
      );
      if (
        !mounted.current ||
        !matchesConversationScope(currentScope(), binding)
      )
        return;
      setTurns((current) => mergeOlderTurns(older.turns, current));
      setConversation((current) =>
        current
          ? { ...current, before: older.before, has_more: older.has_more }
          : current,
      );
    } catch (reason) {
      if (mounted.current && matchesConversationScope(currentScope(), binding))
        setError(formatError(reason));
    } finally {
      lock.current = false;
      if (mounted.current && matchesConversationScope(currentScope(), binding))
        setConversationLoading(false);
    }
  }
  function setPrompt(value: string) {
    setDrafts((old) => ({ ...old, [draftKey]: value }));
  }

  async function send(context?: AgentDraftContext) {
    if (lock.current) return;
    const current = latest.current;
    if (
      !current.connection ||
      current.blocked() ||
      conversationLoading ||
      spacesLoading ||
      !spaces
    ) {
      setError("请先打开工作区并结束其他操作。");
      return;
    }
    if (settings.settingsLoading || settings.settingsBusy) {
      setError("模型配置尚未就绪。");
      return;
    }
    const text = prompt.trim();
    if (!text) return;
    if (new TextEncoder().encode(text).length > 16 * 1024) {
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
    const sessionId = current.connection.sessionId,
      turnMode = modeRef.current,
      token = epoch.current;
    const contextGraph = context ? structuredClone(context) : undefined;
    lock.current = true;
    setBusy(true);
    setStopping(false);
    setError("");
    let binding: NonNullable<typeof request.current> | null = null;
    try {
      let id = selected.current;
      if (!id) {
        const value = await create(sessionId, turnMode);
        if (!sameWorkspace(sessionId, token)) return;
        applyConversation(value);
        setConversations((old) => [value, ...old]);
        id = value.id;
      }
      binding = {
        id: createAiRequestId(),
        sessionId,
        conversationId: id,
        mode: turnMode,
        epoch: token,
      };
      request.current = binding;
      setTurns((old) => [
        ...old,
        {
          id: binding!.id,
          prompt: text,
          state: "running",
          run_ids: [],
          pending_tools: [],
        },
      ]);
      setDrafts((old) => ({ ...old, [draftKey]: "", [id!]: "" }));
      const value = await invokeDesktop<unknown>("agent_turn", {
        sessionId,
        requestId: binding.id,
        conversationId: id,
        mode: turnMode,
        prompt: text,
        config,
        contextGraph,
      });
      if (!matches(binding)) return;
      const reply = parseAgentReply(value, binding.id, id);
      const live: AgentTurn = {
        id: binding.id,
        prompt: text,
        state: reply.state,
        reply,
        run_ids: reply.run_ids ?? [],
      };
      setTurns((old) =>
        old.map((turn) =>
          turn.id === binding!.id
            ? {
                ...turn,
                state: reply.state,
                reply,
                run_ids: reply.run_ids ?? [],
              }
            : turn,
        ),
      );
      try {
        const saved = await readConversationPage(
          invokeDesktop,
          sessionId,
          id,
          turnMode,
        );
        if (matches(binding)) {
          const merged = reconcileConversation(saved, live);
          applyConversation(merged.detail);
          if (merged.unsaved) {
            setError(
              "本轮未完整保存，消息已放回输入框。请先核对执行结果，再决定是否重试或新建会话。",
            );
            setDrafts((old) => ({ ...old, [id!]: text }));
          }
        }
        const listing = await list(sessionId);
        if (matches(binding)) acceptList(listing);
      } catch (reason) {
        if (matches(binding))
          setError(
            "回复已收到，但重新读取保存的会话失败：" + formatError(reason),
          );
      }
    } catch (reason) {
      if (binding && matches(binding)) {
        const message = formatError(reason);
        setError(message);
        setTurns((old) =>
          old.map((turn) =>
            turn.id === binding!.id
              ? { ...turn, state: "interrupted", error: message }
              : turn,
          ),
        );
        try {
          const saved = await readConversationPage(
            invokeDesktop,
            sessionId,
            binding.conversationId,
            turnMode,
          );
          if (matches(binding)) {
            applyConversation(
              reconcileConversation(saved, {
                id: binding.id,
                prompt: text,
                state: "interrupted",
                error: message,
              }).detail,
            );
            setDrafts((old) => ({ ...old, [binding!.conversationId]: text }));
          }
        } catch {
          /* Keep explicit uncertainty; never replay the request. */
        }
      } else if (!binding && sameWorkspace(sessionId, token))
        setError(formatError(reason));
    } finally {
      if (!binding || request.current?.id === binding.id) {
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
      if (matches(active)) {
        setError("停止请求失败：" + formatError(reason));
        setStopping(false);
      }
    }
  }
  return {
    mode,
    setMode,
    turns,
    conversationId: conversation?.id ?? null,
    conversations,
    warnings,
    hasOlder: conversation?.has_more ?? false,
    selectConversation,
    newConversation,
    loadOlder,
    spaces,
    busy,
    readBusy: () => lock.current,
    canStop: request.current !== null,
    stopping,
    loading: spacesLoading || conversationLoading,
    error,
    setError,
    prompt,
    setPrompt,
    attachGraph,
    setAttachGraph,
    send,
    stop,
  };
}
export type AgentTools = ReturnType<typeof useAgentTools>;
