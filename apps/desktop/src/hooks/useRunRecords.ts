import { useEffect, useRef, useState } from "react";
import { invokeDesktop } from "../api/desktop";
import { formatError } from "../model";
import { matchesRunSelection } from "../run-record-model";
import type { RunFileCheck, RunList, RunRecord } from "../types/run-record";

export function useRunRecords(
  sessionId: string | undefined,
  visible: boolean,
  active: boolean,
  revision?: string,
) {
  const [listing, setListing] = useState<RunList>({
    records: [],
    warnings: [],
    truncated: false,
  });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [record, setRecord] = useState<RunRecord | null>(null);
  const [checks, setChecks] = useState<RunFileCheck[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState("");
  const scope = useRef(sessionId);
  scope.current = sessionId;
  const selected = useRef<string | null>(null);
  const listEpoch = useRef(0),
    detailEpoch = useRef(0),
    checkEpoch = useRef(0);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  async function load(id: string, session = scope.current) {
    if (!session) return;
    const token = ++detailEpoch.current;
    try {
      const value = await invokeDesktop<RunRecord>("run_records_load", {
        sessionId: session,
        id,
      });
      if (
        mounted.current &&
        token === detailEpoch.current &&
        matchesRunSelection(scope.current, selected.current, session, id)
      )
        setRecord(value);
    } catch (reason) {
      if (
        mounted.current &&
        token === detailEpoch.current &&
        matchesRunSelection(scope.current, selected.current, session, id)
      )
        setError(formatError(reason));
    }
  }
  function select(id: string) {
    selected.current = id;
    setSelectedId(id);
    setRecord(null);
    setChecks(null);
    setChecking(false);
    ++checkEpoch.current;
    setError("");
    void load(id);
  }
  async function refresh() {
    const session = scope.current;
    if (!session) return;
    const token = ++listEpoch.current;
    setLoading(true);
    try {
      const value = await invokeDesktop<RunList>("run_records_list", {
        sessionId: session,
      });
      if (
        !mounted.current ||
        token !== listEpoch.current ||
        scope.current !== session
      )
        return;
      setListing(value);
      setError("");
      if (!selected.current && value.records.length) {
        const latestRoot =
          value.records.find((r) => !r.parent_id) ?? value.records[0];
        select(latestRoot.id);
      } else if (selected.current) await load(selected.current, session);
    } catch (reason) {
      if (
        mounted.current &&
        token === listEpoch.current &&
        scope.current === session
      )
        setError(formatError(reason));
    } finally {
      if (
        mounted.current &&
        token === listEpoch.current &&
        scope.current === session
      )
        setLoading(false);
    }
  }
  async function checkFiles() {
    const session = scope.current,
      id = selected.current;
    if (!session || !id || checking) return;
    const token = ++checkEpoch.current;
    setChecking(true);
    setError("");
    try {
      const value = await invokeDesktop<RunFileCheck[]>(
        "run_records_check_files",
        { sessionId: session, id },
      );
      if (
        mounted.current &&
        token === checkEpoch.current &&
        matchesRunSelection(scope.current, selected.current, session, id)
      )
        setChecks(value);
    } catch (reason) {
      if (
        mounted.current &&
        token === checkEpoch.current &&
        matchesRunSelection(scope.current, selected.current, session, id)
      )
        setError(formatError(reason));
    } finally {
      if (mounted.current && token === checkEpoch.current) setChecking(false);
    }
  }
  useEffect(() => {
    ++listEpoch.current;
    ++detailEpoch.current;
    ++checkEpoch.current;
    selected.current = null;
    setSelectedId(null);
    setRecord(null);
    setChecks(null);
    setChecking(false);
    setListing({ records: [], warnings: [], truncated: false });
    setError("");
    setLoading(false);
  }, [sessionId]);
  useEffect(() => {
    if (!visible || !sessionId) return;
    void refresh();
    if (!active) return;
    const timer = window.setInterval(() => {
      void refresh();
    }, 2500);
    return () => window.clearInterval(timer);
  }, [sessionId, visible, active, revision]);
  return {
    ...listing,
    selectedId,
    record,
    checks,
    loading,
    checking,
    error,
    select,
    refresh,
    checkFiles,
  };
}
export type RunRecords = ReturnType<typeof useRunRecords>;
