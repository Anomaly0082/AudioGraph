import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
const require = createRequire(import.meta.url);
const { copyGraphSelection, pasteGraphSelection } = require("../../../build/desktop-model-tests/graph-clipboard.js");
const { positionKey } = require("../../../build/desktop-model-tests/graph-canvas-layout.js");

test("clipboard remaps internal edges and exports while preserving unknown plugin data", () => {
  const graph = { schema_version: 1,
    nodes: [ { id: "__proto__", type: "missing.plugin", parameters: { nested: { data: [1, 2] } } }, { id: "constructor", type: "future" }, { id: "outside", type: "other" } ],
    connections: [ { from: { node: "__proto__", port: "future" }, to: { node: "constructor", port: "input" }, extra: [1] }, { from: { node: "outside", port: "out" }, to: { node: "constructor", port: "input" } } ],
    exports: [ { name: "result", node: "constructor", port: "future", extra: true } ],
  };
  const positions = { [positionKey(graph.nodes[0])]: { x: 100, y: 200 }, [positionKey(graph.nodes[1])]: { x: 400, y: 200 } };
  const clipboard = copyGraphSelection(graph, ["__proto__", "constructor"], positions);
  assert.equal(clipboard.connections.length, 1);
  const before = JSON.stringify({ graph, positions, clipboard });
  const pasted = pasteGraphSelection(graph, clipboard, positions);
  assert.deepEqual(pasted.selectedIds, ["__proto___copy", "constructor_copy"]);
  assert.equal(pasted.graph.connections.at(-1).from.node, pasted.selectedIds[0]);
  assert.equal(pasted.graph.connections.at(-1).to.node, pasted.selectedIds[1]);
  assert.equal(pasted.graph.exports.at(-1).name, "result_copy");
  assert.deepEqual(pasted.positions[positionKey(pasted.graph.nodes[3])], { x: 132, y: 232 });
  assert.deepEqual(pasted.graph.nodes[3].parameters, graph.nodes[0].parameters);
  assert.equal(JSON.stringify({ graph, positions, clipboard }), before);
  const twice = pasteGraphSelection(pasted.graph, clipboard, pasted.positions, 64);
  assert.deepEqual(twice.selectedIds, ["__proto___copy2", "constructor_copy2"]);
  assert.equal(twice.graph.exports.at(-1).name, "result_copy2");
  pasted.graph.nodes[3].parameters.nested.data.push(3);
  assert.deepEqual(clipboard.nodes[0].parameters.nested.data, [1, 2]);
  graph.nodes[0].parameters.nested.data.push(9);
  assert.deepEqual(clipboard.nodes[0].parameters.nested.data, [1, 2]);
});

test("empty selection is harmless and clipboard external edges are never pasted", () => {
  const graph = { schema_version: 1, nodes: [{ id: "a", type: "x" }], connections: [] };
  const empty = copyGraphSelection(graph, [], {});
  assert.deepEqual(pasteGraphSelection(graph, empty, {}).graph, graph);
  const clipboard = copyGraphSelection(graph, ["a"], {});
  clipboard.connections.push({ from: { node: "a", port: "x" }, to: { node: "outside", port: "x" } });
  assert.equal(pasteGraphSelection(graph, clipboard, {}).graph.connections.length, 0);
});
