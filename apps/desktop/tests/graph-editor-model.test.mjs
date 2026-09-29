import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";

const require = createRequire(import.meta.url);
const {
  addGraphNode,
  removeGraphNode,
  setNodeParameter,
  connectGraphPorts,
  rewireGraphPorts,
  removeGraphConnection,
  setGraphExport,
  removeGraphExport,
  isNodeAvailableInMode,
} = require("../../../build/desktop-model-tests/graph-editor-model.js");
const { parseGraph } = require("../../../build/desktop-model-tests/model.js");

const descriptor = (typeId, domain = "synchronous", inputs = [], outputs = [], parameters = []) => ({
  typeId,
  displayName: typeId,
  execution_domain: domain,
  inputs,
  outputs,
  parameters,
});
const audioInput = { id: "audio", type: "Audio", required: true };
const audioOutput = { id: "audio", type: "Audio" };
const numberOutput = { id: "value", type: "Number" };
const catalog = [
  descriptor("source", "synchronous", [], [audioOutput, numberOutput]),
  descriptor("gain", "synchronous", [audioInput], [audioOutput], [
    { id: "gain_db", type: "Number", required: true },
    { id: "bypass", type: "Boolean", default: false },
  ]),
  descriptor("sink", "synchronous", [audioInput], []),
];
const graph = () => ({ schema_version: 1, metadata: { custom: true },
  nodes: [{ id: "source", type: "source", custom: { keep: 1 } },
    { id: "gain", type: "gain", parameters: { gain_db: -6, custom: "keep" } },
    { id: "sink", type: "sink" }],
  connections: [{ from: { node: "source", port: "audio" },
    to: { node: "gain", port: "audio" }, custom: "edge" }],
  exports: [{ name: "audio", node: "source", port: "audio", custom: "export" }],
});

test("mode availability exactly matches backend execution domains", () => {
  assert.equal(isNodeAvailableInMode(catalog[0], "offline"), true);
  assert.equal(isNodeAvailableInMode(catalog[0], "streaming"), false);
  assert.equal(isNodeAvailableInMode(descriptor("stream", "streaming"), "streaming"), true);
  assert.equal(isNodeAvailableInMode(descriptor("rt", "realtime"), "realtime"), true);
  assert.equal(isNodeAvailableInMode(descriptor("async", "asynchronous"), "offline"), false);
  assert.equal(isNodeAvailableInMode(descriptor("stream", "streaming"), "realtime"), false);
});

test("empty graph is an editor draft but strict parsing rejects it", () => {
  const source = JSON.stringify({ schema_version: 1, nodes: [], connections: [] });
  assert.deepEqual(parseGraph(source, { allowEmpty: true }).nodes, []);
  assert.throws(() => parseGraph(source), /非空 nodes/);
});

test("adding nodes uses only declared defaults and unique ids", () => {
  const before = graph();
  const snapshot = structuredClone(before);
  const first = addGraphNode(before, catalog[1]);
  const second = addGraphNode(first.graph, catalog[1]);
  assert.equal(first.nodeId, "gain_2");
  assert.equal(second.nodeId, "gain_3");
  assert.deepEqual(first.graph.nodes.at(-1), { id: "gain_2", type: "gain", parameters: { bypass: false } });
  assert.deepEqual(addGraphNode(before, catalog[2]).graph.nodes.at(-1), { id: "sink_2", type: "sink" });
  assert.deepEqual(before, snapshot);
  assert.notEqual(first.graph.nodes, before.nodes);
  assert.equal(second.graph.metadata, before.metadata);
  const nestedDefault = { levels: [1, 2] };
  const nestedDescriptor = descriptor("custom", "synchronous", [], [], [
    { id: "settings", type: "Object", default: nestedDefault },
  ]);
  const nested = addGraphNode(before, nestedDescriptor).graph.nodes.at(-1).parameters.settings;
  assert.deepEqual(nested, nestedDefault);
  assert.notEqual(nested, nestedDefault);
  assert.notEqual(nested.levels, nestedDefault.levels);
});

test("parameter edits preserve unrelated fields and remove omitted values", () => {
  const before = graph();
  const snapshot = structuredClone(before);
  const changed = setNodeParameter(before, "gain", "gain_db", -3);
  assert.deepEqual(changed.nodes[1].parameters, { gain_db: -3, custom: "keep" });
  assert.deepEqual(changed.nodes[0], before.nodes[0]);
  const removed = setNodeParameter(changed, "gain", "gain_db", undefined);
  assert.deepEqual(removed.nodes[1].parameters, { custom: "keep" });
  assert.deepEqual(setNodeParameter(before, "sink", "new", undefined).nodes[2], before.nodes[2]);
  const special = setNodeParameter(before, "sink", "__proto__", "retained");
  assert.equal(Object.hasOwn(special.nodes[2].parameters, "__proto__"), true);
  assert.equal(special.nodes[2].parameters.__proto__, "retained");
  assert.equal(Object.getPrototypeOf(special.nodes[2].parameters), Object.prototype);
  const removedSpecial = setNodeParameter(special, "sink", "__proto__", undefined);
  assert.equal(Object.hasOwn(removedSpecial.nodes[2], "parameters"), false);
  assert.deepEqual(before, snapshot);
  assert.throws(() => setNodeParameter(before, "missing", "x", 1), /未知节点/);
});

test("connections enforce direction, port type, occupied inputs, duplicate edges, cycles and self links", () => {
  const before = graph();
  const snapshot = structuredClone(before);
  const from = { node: "gain", port: "audio" };
  const to = { node: "sink", port: "audio" };
  const linked = connectGraphPorts(before, catalog, from, to);
  assert.equal(linked.connections.length, 2);
  assert.deepEqual(before, snapshot);
  assert.throws(() => connectGraphPorts(linked, catalog, from, to), /连接已存在/);
  assert.throws(() => connectGraphPorts(linked, catalog,
    { node: "source", port: "audio" }, to), /已连接/);
  assert.throws(() => connectGraphPorts(before, catalog,
    { node: "sink", port: "audio" }, to), /不能连接到自身/);
  assert.throws(() => connectGraphPorts(before, catalog,
    { node: "source", port: "missing" }, to), /未知输出端口/);
  assert.throws(() => connectGraphPorts(before, catalog,
    { node: "source", port: "audio" }, { node: "sink", port: "missing" }), /未知输入端口/);
  assert.throws(() => connectGraphPorts(before, catalog,
    { node: "source", port: "value" }, to), /类型不匹配/);
  const mixed = { ...before, nodes: [...before.nodes, { id: "stream", type: "stream" }] };
  const mixedCatalog = [...catalog, descriptor("stream", "streaming", [audioInput], [audioOutput])];
  assert.throws(() => connectGraphPorts(mixed, mixedCatalog,
    { node: "source", port: "audio" }, { node: "stream", port: "audio" }), /执行域/);
  assert.throws(() => connectGraphPorts(before, catalog,
    { node: "missing", port: "audio" }, to), /未知节点/);
  assert.throws(() => connectGraphPorts(before, catalog,
    { node: "source", port: "audio" }, { node: "missing", port: "audio" }), /未知节点/);
  const backward = { ...before, connections: [] };
  assert.throws(() => connectGraphPorts(backward, catalog,
    { node: "sink", port: "audio" }, { node: "gain", port: "audio" }), /未知输出端口/);
  const cycleCatalog = [...catalog, descriptor("return", "synchronous", [audioInput], [audioOutput])];
  const cycleGraph = { ...linked, nodes: [...linked.nodes, { id: "return", type: "return" }],
    connections: [...linked.connections,
      { from: { node: "gain", port: "audio" }, to: { node: "return", port: "audio" } }] };
  assert.throws(() => connectGraphPorts(cycleGraph, cycleCatalog,
    { node: "return", port: "audio" }, { node: "gain", port: "audio" }), /已连接/);
  const freeCycleGraph = { ...cycleGraph,
    connections: cycleGraph.connections.filter((edge) => edge.to.node !== "gain") };
  assert.throws(() => connectGraphPorts(freeCycleGraph, cycleCatalog,
    { node: "return", port: "audio" }, { node: "gain", port: "audio" }), /环路/);
});

test("one output can feed several inputs and removals preserve unrelated metadata", () => {
  const before = graph();
  const expanded = connectGraphPorts(before, catalog,
    { node: "source", port: "audio" }, { node: "sink", port: "audio" });
  assert.equal(expanded.connections.length, 2);
  const disconnected = removeGraphConnection(expanded, 1);
  assert.deepEqual(disconnected.connections, before.connections);
  assert.equal(disconnected.connections[0].custom, "edge");
  assert.throws(() => removeGraphConnection(before, 1), /索引无效/);
  const removed = removeGraphNode(expanded, "source");
  assert.deepEqual(removed.connections, []);
  assert.deepEqual(removed.exports, []);
  assert.deepEqual(removed.nodes.map((node) => node.id), ["gain", "sink"]);
  assert.deepEqual(removeGraphNode({ schema_version: 1,
    nodes: [{ id: "only", type: "source" }], connections: [] }, "only").nodes, []);
  assert.throws(() => removeGraphNode(before, "absent"), /未知节点/);
  assert.equal(before.exports[0].custom, "export");
});

test("rewiring moves an edge, preserves its metadata, and keeps source fan-out", () => {
  const before = graph();
  const snapshot = structuredClone(before);
  const branched = connectGraphPorts(before, catalog,
    { node: "source", port: "audio" }, { node: "sink", port: "audio" });
  const moved = rewireGraphPorts(branched, catalog,
    { node: "source", port: "audio" }, { node: "sink", port: "audio" }, 0);
  assert.equal(moved.connections.length, 1);
  assert.deepEqual(moved.connections[0], {
    from: { node: "source", port: "audio" },
    to: { node: "sink", port: "audio" }, custom: "edge",
  });
  assert.deepEqual(before, snapshot);
  assert.equal(branched.connections[0].custom, "edge");

  const fanout = rewireGraphPorts(before, catalog,
    { node: "source", port: "audio" }, { node: "sink", port: "audio" });
  assert.equal(fanout.connections.length, 2);
  assert.equal(fanout.connections[0], before.connections[0]);
});

test("rewiring replaces an occupied input and exact edges are a metadata-preserving no-op", () => {
  const before = connectGraphPorts(graph(), catalog,
    { node: "gain", port: "audio" }, { node: "sink", port: "audio" });
  const replaced = rewireGraphPorts(before, catalog,
    { node: "source", port: "audio" }, { node: "sink", port: "audio" });
  assert.equal(replaced.connections.length, 2);
  assert.equal(replaced.connections[0], before.connections[0]);
  assert.deepEqual(replaced.connections[1], {
    from: { node: "source", port: "audio" },
    to: { node: "sink", port: "audio" },
  });
  assert.equal(rewireGraphPorts(replaced, catalog,
    { node: "source", port: "audio" }, { node: "gain", port: "audio" }), replaced);
  assert.equal(rewireGraphPorts(replaced, catalog,
    { node: "source", port: "audio" }, { node: "gain", port: "audio" }, 0), replaced);
  assert.equal(replaced.connections[0].custom, "edge");
});

test("failed rewires are atomic, and a stale original index is rejected", () => {
  const before = graph();
  const snapshot = structuredClone(before);
  assert.throws(() => rewireGraphPorts(before, catalog,
    { node: "gain", port: "audio" }, { node: "sink", port: "audio" }, 0), /原连接已更改/);
  assert.throws(() => rewireGraphPorts(before, catalog,
    { node: "source", port: "audio" }, { node: "sink", port: "audio" }, 4), /索引无效/);
  assert.throws(() => rewireGraphPorts(before, catalog,
    { node: "source", port: "value" }, { node: "gain", port: "audio" }), /类型不匹配/);
  assert.deepEqual(before, snapshot);

  const cycleCatalog = [...catalog, descriptor("return", "synchronous", [audioInput], [audioOutput])];
  const cycleGraph = { ...before,
    nodes: [...before.nodes, { id: "return", type: "return" }],
    connections: [...before.connections,
      { from: { node: "gain", port: "audio" }, to: { node: "return", port: "audio" } }],
  };
  const cycleSnapshot = structuredClone(cycleGraph);
  assert.throws(() => rewireGraphPorts(cycleGraph, cycleCatalog,
    { node: "return", port: "audio" }, { node: "gain", port: "audio" }), /环路/);
  assert.deepEqual(cycleGraph, cycleSnapshot);
});

test("exports require unique names and can be removed by index", () => {
  const before = graph();
  const snapshot = structuredClone(before);
  const added = setGraphExport(before, "gain", "audio", "processed");
  assert.deepEqual(added.exports.map((item) => item.name), ["audio", "processed"]);
  assert.equal(added.exports[0].custom, "export");
  assert.deepEqual(removeGraphExport(added, 1).exports, before.exports);
  assert.throws(() => setGraphExport(before, "gain", "audio", "audio"), /名称重复/);
  assert.throws(() => setGraphExport(before, "gain", "audio", "  "), /不能为空/);
  assert.throws(() => setGraphExport(before, "missing", "audio", "new"), /未知节点/);
  assert.throws(() => removeGraphExport(before, -1), /索引无效/);
  assert.deepEqual(before, snapshot);
});
