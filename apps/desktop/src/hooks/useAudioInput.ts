import { useEffect, useRef, useState } from "react";
import { chooseAudioFile } from "../api/desktop";
import { formatError } from "../model";
import type { AudioInspection, Connection } from "../types/desktop";

type Input = {
  path: string;
  setPath: (path: string) => void;
  connection: Connection | null;
  inspect: (path: string) => Promise<AudioInspection>;
};

export function useAudioInput({ path, setPath, connection, inspect }: Input) {
  const [inspection, setInspection] = useState<AudioInspection | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const latest = useRef({ path, sessionId: connection?.sessionId });
  const version = useRef(0);
  latest.current = { path, sessionId: connection?.sessionId };
  useEffect(() => {
    version.current++;
    setInspection(null);
    setError("");
    setBusy(false);
  }, [path, connection?.sessionId]);
  useEffect(
    () => () => {
      version.current++;
    },
    [],
  );

  async function choose() {
    const sessionId = connection?.sessionId;
    try {
      const selected = await chooseAudioFile(connection?.workspace);
      if (selected && latest.current.sessionId === sessionId) setPath(selected);
    } catch (reason) {
      setError(formatError(reason));
    }
  }
  async function check() {
    if (!connection || !path.trim() || busy) return;
    const request = ++version.current;
    const current = latest.current;
    setBusy(true);
    setError("");
    try {
      const value = await inspect(path.trim());
      if (
        version.current === request &&
        current.path === latest.current.path &&
        current.sessionId === latest.current.sessionId
      ) {
        setInspection(value);
      }
    } catch (reason) {
      if (version.current === request) setError(formatError(reason));
    } finally {
      if (version.current === request) setBusy(false);
    }
  }
  return { path, setPath, inspection, error, busy, choose, check };
}
export type AudioInput = ReturnType<typeof useAudioInput>;
