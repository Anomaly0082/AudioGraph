import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
const require = createRequire(import.meta.url);
const Disclosure =
  require("../../../build/desktop-model-tests/components/Disclosure.js").default;

test("disclosure uses an accessible view button and keeps hidden form content mounted", () => {
  const html = renderToStaticMarkup(
    React.createElement(
      Disclosure,
      { label: "工作区与可用工具" },
      React.createElement("input", { defaultValue: "unsaved" }),
    ),
  );
  assert.match(
    html,
    /<button[^>]*type="button"[^>]*aria-expanded="false"[^>]*aria-controls="([^"]+)"/,
  );
  assert.match(html, /查看工作区与可用工具/);
  assert.match(html, /class="disclosure-content" hidden=""/);
  assert.match(html, /value="unsaved"/);
  assert.doesNotMatch(html, /<details|<summary/);
});
test("automatic open state shows content with a collapse button", () => {
  const html = renderToStaticMarkup(
    React.createElement(
      Disclosure,
      { label: " Graph JSON", open: true },
      "invalid draft",
    ),
  );
  assert.match(html, /aria-expanded="true"/);
  assert.match(html, /收起 Graph JSON/);
  assert.doesNotMatch(html, /hidden=/);
  assert.match(html, /invalid draft/);
});
test("production pages and components do not reintroduce native triangle disclosures", () => {
  const root = fileURLToPath(new URL("../src/", import.meta.url));
  for (const folder of ["pages", "components"]) {
    for (const name of readdirSync(join(root, folder)).filter((file) =>
      file.endsWith(".tsx"),
    )) {
      const source = readFileSync(join(root, folder, name), "utf8");
      assert.doesNotMatch(
        source,
        /<\/?(?:details|summary)\b/,
        `${folder}/${name}`,
      );
    }
  }
});
