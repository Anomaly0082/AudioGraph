import assert from "node:assert/strict";
import { createRequire } from "node:module";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

const require = createRequire(import.meta.url);
const { default: NodeInspector, parameterDraft, parseParameterDraft } =
  require("../../../build/desktop-model-tests/components/NodeInspector.js");

const noop = () => {};
const number = { id: "gain_db", type: "number", minimum: -24, maximum: 12 };
const boolean = { id: "enabled", type: "boolean" };

test("parameter conversion preserves booleans and rejects empty or invalid numbers", () => {
  assert.equal(parameterDraft(boolean, undefined), false);
  assert.deepEqual(parseParameterDraft(boolean, false), { ok: true, value: false });
  assert.equal(parseParameterDraft(boolean, "").ok, false);
  assert.equal(parseParameterDraft(number, "").ok, false);
  assert.equal(parseParameterDraft(number, "  ").ok, false);
  assert.equal(parseParameterDraft(number, "NaN").ok, false);
  assert.equal(parseParameterDraft(number, "13").ok, false);
  assert.deepEqual(parseParameterDraft(number, "-6"), { ok: true, value: -6 });
  assert.deepEqual(parseParameterDraft({ ...number, integer_only: true }, "1.5"),
    { ok: false, error: "请输入整数。" });
});

test("inspector shows missing required fields, default, and clear only for own values", () => {
  const parameters = Object.create({ gain_db: 9 });
  parameters.enabled = false;
  const html = renderToStaticMarkup(React.createElement(NodeInspector, {
    node: { id: "n", type: "sample", parameters },
    descriptor: {
      typeId: "sample", displayName: "测试", execution_domain: "synchronous",
      inputs: [], outputs: [], parameters: [
        { ...number, required: true, default: -6 },
        boolean,
      ],
    },
    devices: { inputs: [], outputs: [] }, canListDevices: false,
    onListDevices: noop, onParameter: noop, onDelete: noop,
    exports: [], onExport: noop, onRemoveExport: noop,
  }));
  assert.match(html, /gain_db<span class="inspector-required"[^>]*> \*<\/span>/);
  assert.match(html, /未设置 · 默认 -6/);
  assert.match(html, /value="-6"/);
  assert.equal((html.match(/>清除<\/button>/g) ?? []).length, 1);
  const checkbox = html.match(/<input[^>]*type="checkbox"[^>]*>/)?.[0];
  assert.ok(checkbox);
  assert.doesNotMatch(checkbox, /checked/);
});
