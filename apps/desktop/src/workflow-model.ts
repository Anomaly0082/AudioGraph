import type { GraphSubmission } from "./types/desktop";

export function isCurrentValidationResponse(
  expectedSessionId: string,
  expectedKey: string,
  currentSessionId: string | null,
  currentKey: string,
): boolean {
  return expectedSessionId === currentSessionId && expectedKey === currentKey;
}

// A task always owns the exact Graph/options submitted, even as the editor changes.
export function cloneSubmission(submission: GraphSubmission): GraphSubmission {
  const snapshot = JSON.parse(JSON.stringify(submission)) as GraphSubmission;
  const freeze = (value: unknown): void => {
    if (value === null || typeof value !== "object" || Object.isFrozen(value))
      return;
    for (const child of Object.values(value)) freeze(child);
    Object.freeze(value);
  };
  freeze(snapshot);
  return snapshot;
}
