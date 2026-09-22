import { appendFileSync } from "node:fs";

const scenario = process.argv[2] ?? "normal";
const marker = process.argv[3];
let buffered = "";
let requestsAfterHandshake = [];
if (scenario === "stubborn") setInterval(() => {}, 1_000);

function record(request) {
  if (marker) appendFileSync(marker, `${request.op}\n`, "utf8");
}

function reply(id, data = {}) {
  process.stdout.write(`${JSON.stringify({ schema_version: 1, id, success: true, data })}\n`);
}

function onRequest(request) {
  record(request);
  if (request.op === "capabilities") {
    reply(request.id, {
      operations: ["capabilities", "nodes.list", "nodes.describe", "graph.validate", "tasks.start",
        "tasks.status", "tasks.cancel", "tasks.result", "tasks.release"],
      modes: ["offline", "streaming"],
      permissions: { devices: false, monitor: false },
    });
    return;
  }

  if (scenario === "reorder") {
    requestsAfterHandshake.push(request);
    if (requestsAfterHandshake.length === 2) {
      const [first, second] = requestsAfterHandshake;
      reply(second.id, { echoed: second.op });
      setTimeout(() => reply(first.id, { echoed: first.op }), 5);
    }
    return;
  }
  if (scenario === "oversize") {
    process.stdout.write(`${"x".repeat(8 * 1024 * 1024 + 1)}\n`);
    return;
  }
  if (scenario === "timeout") return;
  if (scenario === "eof") {
    process.exit(23);
    return;
  }
  if (scenario === "bad-json") {
    process.stdout.write("{not-json}\n");
    return;
  }
  reply(request.id, { echoed: request.op });
}

process.stdin.setEncoding("utf8");
process.stdin.on("data", chunk => {
  buffered += chunk;
  for (;;) {
    const newline = buffered.indexOf("\n");
    if (newline < 0) break;
    const line = buffered.slice(0, newline).replace(/\r$/, "");
    buffered = buffered.slice(newline + 1);
    if (line.length > 0) onRequest(JSON.parse(line));
  }
});
