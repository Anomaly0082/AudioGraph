import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { createAudioServer } from "../../src/server.mjs";

const client = {
  async request(request) {
    const base = { schema_version: 1, id: "fixture", success: true };
    switch (request.op) {
      case "capabilities":
        return { ...base, data: { fixture: true } };
      case "nodes.list":
        return { ...base, data: { nodes: [] } };
      case "nodes.describe":
        return request.type === "missing"
          ? { ...base, success: false, errors: [{ code: "unknown_node_type", message: "missing" }] }
          : { ...base, data: { node: { type: request.type } } };
      case "audio.inspect":
        return { ...base, data: { echoed: request.op, path: request.path, sample_rate: 44_100,
          channels: 2, frame_count: 4410, duration_seconds: 0.1, encoding: "pcm_s16le" } };
      case "graph.validate":
        return { ...base, data: { valid: true, received_graph: request.graph } };
      case "tasks.start":
        return { ...base, data: { task_id: "task-fixture", state: "running" } };
      case "tasks.status":
        return { ...base, data: { task_id: request.task_id, state: "failed", errors: [{ code: "node_failed" }] } };
      case "tasks.cancel":
        return { ...base, data: { task_id: request.task_id, state: "cancelled" } };
      case "tasks.result":
        return { ...base, data: { task_id: request.task_id, state: "failed", errors: [{ code: "node_failed" }] } };
      case "tasks.release":
        return { ...base, data: { task_id: request.task_id, released: true } };
      default:
        throw new Error(`fixture received unexpected op ${request.op}`);
    }
  },
};

const server = createAudioServer(client);
const transport = new StdioServerTransport();
await server.connect(transport);
process.stdin.on("end", async () => {
  await server.close();
});
