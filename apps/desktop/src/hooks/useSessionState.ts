import { useEffect, useRef, useState } from "react";
import { confirmDesktop, invokeDesktop, listenSessionClose, listenSessionCloseCancelled } from "../api/desktop";
import { createSessionWriter, validateSessionState, type SessionState } from "../session-state-model";
import { formatError } from "../model";

export function useSessionState(desktop: boolean, capture: () => SessionState, restore: (state: SessionState) => Promise<void>) {
  const [ready, setReady] = useState(!desktop);
  const [closing, setClosing] = useState(false);
  const [error, setError] = useState("");
  const [disabled, setDisabled] = useState(false);
  const callbacks = useRef({ capture, restore }); callbacks.current = { capture, restore };
  const enabled = useRef(false);
  const started = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const closeIds = useRef(new Set<string>());
  const activeClose = useRef<string | null>(null);
  const initialized = useRef<Promise<void> | null>(null);
  const writer = useRef(createSessionWriter(state => invokeDesktop<void>("session_state_save", { state })));
  const snapshot = JSON.stringify(capture());
  async function flush() {
    if (timer.current) clearTimeout(timer.current);
    timer.current = null;
    if (!enabled.current) throw new Error("恢复文件自动保存已暂停。");
    writer.current.enqueue(callbacks.current.capture());
    await writer.current.flush();
  }
  async function close(requestId: string) {
    if (closeIds.current.has(requestId)) return;
    closeIds.current.add(requestId); activeClose.current = requestId; setClosing(true);
    try {
      await initialized.current;
      await new Promise(resolve => setTimeout(resolve, 0));
      if (activeClose.current !== requestId) return;
      await flush();
      if (activeClose.current !== requestId) return;
      await invokeDesktop("session_state_finish_close", { requestId });
    } catch (reason) {
      if (activeClose.current !== requestId) return;
      setError(formatError(reason));
      try {
        const ownsConfirmation = await invokeDesktop<boolean>("session_state_close_confirmation", { requestId });
        if (!ownsConfirmation || activeClose.current !== requestId) return;
        const exit = await confirmDesktop("未能保存恢复文件。仍要退出？本次编辑可能无法恢复。", "退出确认");
        if (activeClose.current !== requestId) return;
        if (exit) await invokeDesktop("session_state_finish_close", { requestId });
        else { await invokeDesktop("session_state_cancel_close", { requestId }); activeClose.current = null; setClosing(false); }
      } catch (failure) {
        if (activeClose.current !== requestId) return;
        setError(formatError(failure));
        try { await invokeDesktop("session_state_cancel_close", { requestId }); }
        catch (cancelFailure) { if (activeClose.current === requestId) setError(formatError(cancelFailure)); }
        if (activeClose.current === requestId) { activeClose.current = null; setClosing(false); }
      }
    }
  }
  useEffect(() => {
    if (!desktop || started.current) return;
    started.current = true;
    // This listener and writer live for the app lifetime, including StrictMode's
    // effect replay. Cleanup must not discard writes or miss native close events.
    initialized.current = (async () => {
      try {
        await listenSessionCloseCancelled((id, reason) => { if (activeClose.current === id) { activeClose.current = null; setClosing(false); if (reason) setError(reason); } });
        await listenSessionClose(id => { void close(id); });
        const result = await invokeDesktop<{ state: unknown }>("session_state_load");
        if (result.state !== null) await callbacks.current.restore(validateSessionState(result.state));
        enabled.current = true;
      } catch (reason) { setError(formatError(reason)); setDisabled(true); }
      finally { setReady(true); }
    })();
  }, [desktop]);
  useEffect(() => {
    if (!desktop || !ready || closing || !enabled.current) return;
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => { timer.current = null; void flush().catch(reason => setError(formatError(reason))); }, 300);
  }, [snapshot, ready, closing, desktop]);
  async function reset() {
    if (!(await confirmDesktop("删除已有恢复文件，并用当前草稿重新开始自动保存？", "重置恢复文件"))) return;
    try {
      await writer.current.flush().catch(() => undefined);
      await invokeDesktop("session_state_clear");
      enabled.current = true; setDisabled(false); setError(""); await flush();
    } catch (reason) { setError(formatError(reason)); }
  }
  return { ready, closing, error, disabled, reset };
}
