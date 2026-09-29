import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const {
  autoLayout,
  nodePorts,
  positionKey,
  resolvedPositions,
  CARD_WIDTH,
  HEADER_HEIGHT,
  PORT_HEIGHT,
} = require("../../../build/desktop-model-tests/graph-canvas-layout.js");

const descriptor = (typeId, inputs = [], outputs = []) => ({
  typeId,
  displayName: typeId,
  execution_domain: "synchronous",
  inputs,
  outputs,
});

test("existing edges and exports remain visible when catalog data is absent or stale", () => {
  const source = { id: "source", type: "future_source" };
  const target = { id: "target", type: "future_target" };
  const graph = {
    schema_version: 1,
    nodes: [source, target],
    connections: [
      { from: { node: "source", port: "old" }, to: { node: "target", port: "input" } },
      { from: { node: "source", port: "old" }, to: { node: "target", port: "new" } },
    ],
    exports: [{ name: "extra", node: "source", port: "export_only" }],
  };
  assert.deepEqual(nodePorts(graph, source).outputs.map(({ id }) => id), ["old", "export_only"]);
  assert.deepEqual(nodePorts(graph, target).inputs.map(({ id }) => id), ["input", "new"]);
  const stale = descriptor("future_source", [], [{ id: "current", type: "Audio" }]);
  assert.deepEqual(nodePorts(graph, source, stale).outputs.map(({ id }) => id),
    ["current", "old", "export_only"]);
});

test("layout places dependent nodes after sources and uses port count for vertical spacing", () => {
  const source = { id: "source", type: "source" };
  const second = { id: "second", type: "source" };
  const sink = { id: "sink", type: "sink" };
  const graph = {
    schema_version: 1,
    nodes: [source, second, sink],
    connections: [{ from: { node: "source", port: "out" }, to: { node: "sink", port: "in" } }],
  };
  const catalog = [descriptor("source", [], Array.from({ length: 5 }, (_, index) =>
    ({ id: index === 0 ? "out" : `out${index}`, type: "Audio" }))), descriptor("sink")];
  const layout = autoLayout(graph, catalog);
  assert.ok(layout[positionKey(sink)].x >= layout[positionKey(source)].x + CARD_WIDTH);
  assert.equal(layout[positionKey(second)].y - layout[positionKey(source)].y,
    HEADER_HEIGHT + 5 * PORT_HEIGHT + 18 + 54);
});

test("saved positions use own keys, finite coordinates, and graph identity", () => {
  const node = { id: "constructor", type: "toString" };
  const otherType = { id: "constructor", type: "different" };
  const graph = { schema_version: 1, nodes: [node], connections: [] };
  const base = autoLayout(graph, []);
  assert.deepEqual(resolvedPositions(graph, [], Object.create({ [positionKey(node)]: { x: 999, y: 999 } })), base);
  assert.deepEqual(resolvedPositions(graph, [], { [positionKey(node)]: { x: Infinity, y: 50 } }), base);
  assert.deepEqual(resolvedPositions(graph, [], { [positionKey(otherType)]: { x: 999, y: 999 } }), base);
  assert.deepEqual(resolvedPositions(graph, [], { [positionKey(node)]: { x: -10, y: 0 } })[positionKey(node)],
    { x: 12, y: 12 });
});
