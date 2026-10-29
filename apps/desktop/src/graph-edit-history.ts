import type { CanvasView } from "./graph-canvas-layout";

export type GraphSnapshot = { text: string; view: CanvasView };
const MAX_STEPS = 50;
const MAX_BYTES = 8 * 1024 * 1024;
const clone = <T>(value: T): T => structuredClone(value);

/** In-memory document history. A group identifies one continuous editing gesture. */
export function createGraphHistory(initial: GraphSnapshot) {
  let entries = [clone(initial)];
  let cursor = 0;
  let lastGroup: string | undefined;
  const current = () => clone(entries[cursor]);
  const bytes = () => entries.reduce((sum, item) =>
    sum + new TextEncoder().encode(JSON.stringify(item)).length, 0);
  const trim = () => {
    // Keep the current document even when a single snapshot exceeds the budget.
    while (entries.length > 1 && (entries.length > MAX_STEPS + 1 || bytes() > MAX_BYTES)) {
      entries.shift();
      cursor--;
    }
  };
  return {
    get current() { return current(); },
    get canUndo() { return cursor > 0; },
    get canRedo() { return cursor < entries.length - 1; },
    commit(snapshot: GraphSnapshot, group?: string) {
      if (JSON.stringify(snapshot) === JSON.stringify(entries[cursor])) return current();
      const merge = group !== undefined && group === lastGroup && cursor > 0;
      entries = entries.slice(0, cursor + 1);
      if (merge) entries[cursor] = clone(snapshot);
      else { entries.push(clone(snapshot)); cursor++; }
      lastGroup = group;
      trim();
      return current();
    },
    undo() {
      if (cursor > 0) cursor--;
      lastGroup = undefined;
      return current();
    },
    redo() {
      if (cursor < entries.length - 1) cursor++;
      lastGroup = undefined;
      return current();
    },
    reset(snapshot: GraphSnapshot) {
      entries = [clone(snapshot)]; cursor = 0; lastGroup = undefined;
      return current();
    },
    replaceView(view: CanvasView) {
      entries[cursor] = { text: entries[cursor].text, view: clone(view) };
      return current();
    },
  };
}
