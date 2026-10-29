import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
const require = createRequire(import.meta.url);
const { createGraphHistory } = require("../../../build/desktop-model-tests/graph-edit-history.js");
const snapshot = (text) => ({ text, view: { positions: {}, selectedId: "", zoom: 1, pan: { x: 0, y: 0 } } });

test("history groups gestures, restores views, and clears a branched redo", () => {
  const history = createGraphHistory(snapshot("a"));
  history.commit(snapshot("b"), "typing-1");
  history.commit(snapshot("c"), "typing-1");
  assert.equal(history.undo().text, "a");
  assert.equal(history.redo().text, "c");
  history.commit(snapshot("d"), "typing-2");
  assert.equal(history.undo().text, "c");
  history.commit(snapshot("e"));
  assert.equal(history.canRedo, false);
  const view = snapshot("").view;
  view.zoom = 2;
  history.replaceView(view);
  view.zoom = 9;
  assert.equal(history.current.view.zoom, 2);
  assert.equal(history.undo().text, "c");
  assert.equal(history.redo().view.zoom, 2);
});

test("history owns snapshots, resets, and retains at most fifty undo steps", () => {
  const initial = snapshot("0");
  const history = createGraphHistory(initial);
  initial.view.pan.x = 50;
  history.current.view.pan.x = 70;
  assert.equal(history.current.view.pan.x, 0);
  for (let i = 1; i <= 60; i++) history.commit(snapshot(String(i)));
  let steps = 0;
  while (history.canUndo) { history.undo(); steps++; }
  assert.equal(steps, 50);
  assert.equal(history.current.text, "10");
  history.reset(snapshot("new"));
  assert.equal(history.canUndo, false);
  assert.equal(history.canRedo, false);
});

test("history bounds aggregate snapshot memory", () => {
  const history = createGraphHistory(snapshot("a".repeat(3 * 1024 * 1024)));
  history.commit(snapshot("b".repeat(3 * 1024 * 1024)));
  history.commit(snapshot("c".repeat(3 * 1024 * 1024)));
  assert.equal(history.undo().text[0], "b");
  assert.equal(history.canUndo, false);
});
