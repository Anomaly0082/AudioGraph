import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
import test from "node:test";
import React from "react";
import { renderWithoutDecorativeIcons as renderToStaticMarkup } from "./markup-helpers.mjs";

const require = createRequire(import.meta.url);
const { panAfterDrag, screenToCanvas, zoomAround, wirePath, portColor } =
  require("../../../build/desktop-model-tests/canvas-interaction.js");
const GraphCanvas = require("../../../build/desktop-model-tests/components/GraphCanvas.js").default;

test("middle-button camera movement uses screen pixels at every zoom", () => {
  const origin = { x: -80, y: 45 };
  const result = panAfterDrag(origin, { x: 120, y: 250 }, { x: 165, y: 210 });
  assert.deepEqual(result, { x: -35, y: 5 });
  assert.deepEqual(origin, { x: -80, y: 45 });
  assert.deepEqual(screenToCanvas({ x: 85, y: 65 }, result, 2), { x: 60, y: 30 });
});

test("zoom keeps the canvas point under the viewport anchor fixed", () => {
  const pan = { x: -100, y: 25 };
  const anchor = { x: 320, y: 180 };
  const before = screenToCanvas(anchor, pan, 0.75);
  const nextPan = zoomAround(pan, 0.75, 1.4, anchor);
  assert.deepEqual(screenToCanvas(anchor, nextPan, 1.4), before);
  assert.deepEqual(zoomAround(pan, 1, 1, anchor), pan);
});

test("wire geometry remains smooth when a target is left of its source", () => {
  assert.equal(wirePath({ x: 20, y: 10 }, { x: 220, y: 50 }),
    "M 20 10 C 124.8 10, 115.2 50, 220 50");
  const reversed = wirePath({ x: 220, y: 50 }, { x: 20, y: 10 });
  assert.match(reversed, /^M 220 50 C [\d.]+ 50, -[\d.]+ 10, 20 10$/);
});

test("socket colors are stable by port type", () => {
  assert.equal(portColor("Audio"), portColor("AudioBlock"));
  assert.notEqual(portColor("Audio"), portColor("Boolean"));
  assert.equal(portColor("unrecognized"), "#708197");
});

test("socket types retain distinct colors and readable contrast on light graph surfaces", () => {
  const base = readFileSync(new URL("../src/styles/base.css", import.meta.url), "utf8");
  const graph = readFileSync(new URL("../src/styles/graph-editor.css", import.meta.url), "utf8");
  assert.match(graph, /\.graph-canvas\s*\{[^}]*background:\s*var\(--surface-subtle\)/);
  const canvasBackground = base.match(/--surface-subtle:\s*(#[\da-f]{6})/i)?.[1];
  assert.ok(canvasBackground, "canvas surface token must be defined");
  const luminance = (hex) => {
    const rgb = hex.slice(1).match(/../g).map((channel) => parseInt(channel, 16) / 255)
      .map((channel) => channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4);
    return rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
  };
  const types = ["Audio", "Text", "FilePath", "Number", "Boolean", "unrecognized"];
  assert.equal(new Set(types.map(portColor)).size, types.length);
  assert.equal(portColor("AudioStream"), portColor("Audio"));
  assert.equal(portColor("file_path"), portColor("FilePath"));
  for (const type of types) {
    for (const background of ["#ffffff", canvasBackground]) {
      const levels = [luminance(portColor(type)), luminance(background)].sort((a, b) => b - a);
      const contrast = (levels[0] + 0.05) / (levels[1] + 0.05);
      assert.ok(contrast >= 3, `${type} contrast ${contrast.toFixed(2)} on ${background} must be at least 3:1`);
    }
  }
});

test("canvas renders colored socket wires and camera transform", () => {
  const graph = { schema_version: 1,
    nodes: [{ id: "source", type: "source" }, { id: "sink", type: "sink" }],
    connections: [{ from: { node: "source", port: "audio" }, to: { node: "sink", port: "audio" } }],
  };
  const nodes = [
    { typeId: "source", displayName: "Source", execution_domain: "synchronous",
      inputs: [], outputs: [{ id: "audio", type: "Audio" }] },
    { typeId: "sink", displayName: "Sink", execution_domain: "synchronous",
      inputs: [{ id: "audio", type: "Audio" }], outputs: [] },
  ];
  const html = renderToStaticMarkup(React.createElement(GraphCanvas, {
    graph, nodes,
    view: { positions: {}, selectedId: "", zoom: 1.25, pan: { x: -30, y: 40 } },
    onView: () => {}, onConnect: () => {}, onDisconnect: () => {},
  }));
  assert.match(html, /transform:translate\(-30px, 40px\) scale\(1\.25\)/);
  assert.match(html, /--wire-color:#a57525/);
  assert.match(html, /--socket-color:#a57525/);
  assert.match(html, /class="edge-line" d="M /);
  assert.doesNotMatch(html, /marker-end=/);
  assert.match(html, /画布，中键拖动平移/);
  assert.match(html, /data-port-side="input"/);
  assert.match(html, /data-port-side="output"/);
});

const connectedGraph = {
  schema_version: 1,
  nodes: [{ id: "source", type: "source" }, { id: "sink", type: "sink" }],
  connections: [{
    from: { node: "source", port: "audio" },
    to: { node: "sink", port: "audio" },
  }],
};
const nodeCatalog = [
  { typeId: "source", displayName: "Source", execution_domain: "synchronous",
    inputs: [], outputs: [{ id: "audio", type: "Audio" }] },
  { typeId: "sink", displayName: "Sink", execution_domain: "synchronous",
    inputs: [{ id: "audio", type: "Audio" }], outputs: [] },
];
function renderCanvas(nodes, extra = {}) {
  return renderToStaticMarkup(React.createElement(GraphCanvas, {
    graph: connectedGraph,
    nodes,
    view: { positions: {}, selectedId: "", zoom: 1, pan: { x: 0, y: 0 } },
    onView: () => {}, onConnect: () => {}, onDisconnect: () => {},
    ...extra,
  }));
}
function buttonsWithClass(html, className) {
  return [...html.matchAll(/<button\b[^>]*>/g)]
    .map(([tag]) => tag)
    .filter((tag) => tag.includes(`class="${className}`));
}

test("without a catalog, inferred ports stay visible but wiring is disabled", () => {
  const html = renderCanvas([]);
  const ports = buttonsWithClass(html, "canvas-port");
  assert.equal(ports.length, 2);
  assert.ok(ports.every((tag) => tag.includes('disabled=""')));
  assert.match(html, /data-port-node="source"[^>]*data-port-id="audio"[^>]*data-port-side="output"/);
  assert.match(html, /data-port-node="sink"[^>]*data-port-id="audio"[^>]*data-port-side="input"/);
  assert.match(html, /class="edge-line" d="M /);
  assert.match(html, /打开工作区后可编辑连线/);
  assert.ok(buttonsWithClass(html, "canvas-node-header").every((tag) => !tag.includes("disabled")));
  assert.match(html, /<button>整理布局<\/button>/);
});

test("a catalog enables ports, including declared unconnected ports", () => {
  const graph = { ...connectedGraph, connections: [] };
  const html = renderCanvas(nodeCatalog, { graph });
  const ports = buttonsWithClass(html, "canvas-port");
  assert.equal(ports.length, 2);
  assert.ok(ports.every((tag) => !tag.includes("disabled")));
  assert.match(html, /data-port-side="output"/);
  assert.match(html, /data-port-side="input"/);
  assert.match(html, /中键平移 · 拖动端口连线/);
});

test("an explicit connection reason disables wiring even with a cached catalog", () => {
  const html = renderCanvas(nodeCatalog, {
    connectionEditReason: "打开工作区后可编辑连线",
  });
  const ports = buttonsWithClass(html, "canvas-port");
  assert.equal(ports.length, 2);
  assert.ok(ports.every((tag) => tag.includes('disabled=""')));
  assert.match(html, /打开工作区后可编辑连线/);
  assert.ok(buttonsWithClass(html, "canvas-node-header").every((tag) => !tag.includes("disabled")));
  assert.match(html, /<button>整理布局<\/button>/);
});
