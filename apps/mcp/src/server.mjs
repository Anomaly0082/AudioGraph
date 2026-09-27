import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import * as z from 'zod/v4';

export const TOOL_NAMES = Object.freeze([
  'audio_capabilities',
  'audio_list_nodes',
  'audio_describe_node',
  'audio_inspect',
  'audio_validate_graph',
  'audio_start_task',
  'audio_task_status',
  'audio_cancel_task',
  'audio_task_result',
  'audio_release_task',
]);

const emptyInput = z.strictObject({});
const endpointSchema = z.strictObject({
  node: z.string().min(1).describe('Node ID.'),
  port: z.string().min(1).describe('Port ID.'),
});
const graphSchema = z.strictObject({
  schema_version: z.literal(1),
  nodes: z.array(z.strictObject({
    id: z.string().min(1).describe('Unique node ID within this graph.'),
    type: z.string().min(1).describe('Registered node type ID from audio_list_nodes.'),
    parameters: z.record(z.string(), z.unknown()).optional()
      .describe('Node parameters. Values are preserved exactly and validated by the C++ registry.'),
  })).min(1),
  connections: z.array(z.strictObject({
    from: endpointSchema,
    to: endpointSchema,
  })),
  exports: z.array(z.strictObject({
    name: z.string().min(1).describe('Result export name.'),
    node: z.string().min(1).describe('Source node ID.'),
    port: z.string().min(1).describe('Source port ID.'),
  })).optional(),
});
const graphInput = z.strictObject({
  mode: z.enum(['offline', 'streaming']).describe('Execution mode exposed by this MCP adapter.'),
  graph: graphSchema.describe('Inline Graph v1 JSON object. This is not a file path or executable script.'),
  options: z.strictObject({
    block_frames: z.number().int().min(1).max(65536).optional()
      .describe('Streaming block size. Only valid when mode is streaming.'),
  }).optional().describe('Execution options; block_frames is streaming-only.'),
});
const typeInput = z.strictObject({
  type: z.string().min(1).max(256).describe('Registered node type ID.'),
});
const taskInput = z.strictObject({
  task_id: z.string().min(1).max(256).describe('Task ID returned by audio_start_task.'),
});
const inspectInput = z.strictObject({
  path: z.string().min(1).max(4096)
    .describe('Explicit path to an existing PCM16 WAV file inside the fixed host workspace.'),
});

const errorDetail = z.looseObject({
  code: z.string(),
  message: z.string(),
  node_id: z.string().optional(),
  port_id: z.string().optional(),
  parameter_id: z.string().optional(),
  field_path: z.string().optional(),
});
const responseOutput = z.looseObject({
  schema_version: z.number().int().optional(),
  id: z.union([z.string(), z.null()]).optional(),
  success: z.boolean(),
  data: z.unknown().optional(),
  errors: z.array(errorDetail).optional(),
});

const annotations = (readOnlyHint, destructiveHint = false, idempotentHint = true) => ({
  readOnlyHint,
  destructiveHint,
  idempotentHint,
  openWorldHint: false,
});

function toolResult(payload, isError = payload?.success === false) {
  return {
    content: [{ type: 'text', text: JSON.stringify(payload, null, 2) }],
    structuredContent: payload,
    ...(isError ? { isError: true } : {}),
  };
}

function localError(error) {
  const detail = {
    code: typeof error?.code === 'string' ? error.code : 'mcp_backend_error',
    message: error instanceof Error ? error.message : 'The controlled audio backend request failed.',
  };
  if (typeof error?.uncertain === 'boolean') detail.uncertain = error.uncertain;
  if (typeof error?.forced === 'boolean') detail.forced = error.forced;
  return { success: false, errors: [detail] };
}

function compactCapabilities(envelope) {
  if (!envelope?.success || !envelope.data) return envelope;
  const { policy, limits } = envelope.data;
  if (policy?.allow_devices !== false || policy?.allow_monitor !== false) {
    return {
      schema_version: envelope.schema_version,
      id: envelope.id,
      success: false,
      errors: [{
        code: 'backend_scope_violation',
        message: 'The backend enabled device or monitor access, which this MCP adapter does not expose.',
      }],
    };
  }
  return {
    ...envelope,
    data: {
      modes: ['offline', 'streaming'],
      policy,
      limits,
      tools: TOOL_NAMES,
      scope: {
        devices: false,
        realtime: false,
        workflow_training: false,
        graph_input: 'inline_json_only',
        note: 'This adapter only orchestrates controlled Graph v1 offline and streaming tasks. It cannot run shell/Python, load plugins, change the host workspace, enumerate devices, or start realtime audio.',
      },
    },
  };
}

function compactNodes(envelope) {
  if (!envelope?.success || !Array.isArray(envelope.data?.nodes)) return envelope;
  const nodes = envelope.data.nodes
    .filter(node => node?.execution_domain === 'synchronous' || node?.execution_domain === 'streaming')
    .map(node => ({
      type: node.typeId,
      display_name: node.displayName,
      description: node.description,
      execution_domain: node.execution_domain,
      stream_role: node.stream_role,
      inputs: Array.isArray(node.inputs)
        ? node.inputs.map(({ id, type, required }) => ({ id, type, required }))
        : [],
      outputs: Array.isArray(node.outputs)
        ? node.outputs.map(({ id, type, required }) => ({ id, type, required }))
        : [],
      parameters: Array.isArray(node.parameters)
        ? node.parameters.map(({ id, type, required }) => ({ id, type, required }))
        : [],
    }));
  return { ...envelope, data: { nodes } };
}

function exposedNode(envelope) {
  if (!envelope?.success) return envelope;
  const domain = envelope.data?.node?.execution_domain;
  if (domain === 'synchronous' || domain === 'streaming') return envelope;
  return {
    schema_version: envelope.schema_version,
    id: envelope.id,
    success: false,
    errors: [{
      code: 'node_not_exposed',
      message: 'This node is not exposed by the offline/streaming MCP adapter.',
    }],
  };
}

function register(server, client, name, config, makeRequest, transform = value => value, classifyError) {
  server.registerTool(name, { ...config, outputSchema: responseOutput }, async input => {
    try {
      const envelope = transform(await client.request(makeRequest(input)));
      const isError = classifyError ? classifyError(envelope) : envelope?.success === false;
      return toolResult(envelope, isError);
    } catch (error) {
      return toolResult(localError(error), true);
    }
  });
}

/**
 * Build an unconnected MCP server around a controlled backend client.
 * The caller owns client startup/shutdown and transport lifecycle.
 */
export function createAudioServer(client) {
  if (!client || typeof client.request !== 'function') {
    throw new TypeError('createAudioServer requires a client with request(request).');
  }

  const server = new McpServer(
    { name: 'audiograph-controlled', version: '0.1.0' },
    { capabilities: { tools: {} } },
  );

  register(server, client, 'audio_capabilities', {
    title: 'Audio adapter capabilities',
    description: 'Return this adapter\'s offline/streaming scope, host policy, and limits. Node details are intentionally omitted; call audio_list_nodes.',
    inputSchema: emptyInput,
    annotations: annotations(true),
  }, () => ({ op: 'capabilities' }), compactCapabilities);

  register(server, client, 'audio_list_nodes', {
    title: 'List audio graph nodes',
    description: 'List compact summaries for nodes usable by this offline/streaming adapter. Call audio_describe_node before constructing parameters.',
    inputSchema: emptyInput,
    annotations: annotations(true),
  }, () => ({ op: 'nodes.list' }), compactNodes);

  register(server, client, 'audio_describe_node', {
    title: 'Describe an audio graph node',
    description: 'Return the complete registered descriptor for one node type. Registration does not guarantee compatibility with every exposed mode.',
    inputSchema: typeInput,
    annotations: annotations(true),
  }, ({ type }) => ({ op: 'nodes.describe', type }), exposedNode);

  register(server, client, 'audio_inspect', {
    title: 'Inspect a PCM16 WAV file',
    description: 'Read validated WAV header metadata for one explicit existing file inside the fixed host workspace. Returns no audio samples and does not infer a path from natural language.',
    inputSchema: inspectInput,
    annotations: annotations(true),
  }, ({ path }) => ({ op: 'audio.inspect', path }));

  register(server, client, 'audio_validate_graph', {
    title: 'Validate an audio graph',
    description: 'Validate an inline Graph v1 for offline or streaming execution, including the host file boundary. Does not create outputs or start a task.',
    inputSchema: graphInput,
    annotations: annotations(true),
  }, input => ({ op: 'graph.validate', ...input }));

  register(server, client, 'audio_start_task', {
    title: 'Start an audio graph task',
    description: 'Submit an inline offline/streaming Graph v1 exactly once. A graph may write new files inside the fixed host workspace. This call never auto-retries; MCP request cancellation does not cancel a submitted task, so use audio_cancel_task with its task_id.',
    inputSchema: graphInput,
    annotations: annotations(false, true, false),
  }, input => ({ op: 'tasks.start', ...input }));

  register(server, client, 'audio_task_status', {
    title: 'Get audio task status',
    description: 'Read task state and diagnostics. A successful status lookup may report state failed or cancelled; that is task state, not a failed status query.',
    inputSchema: taskInput,
    annotations: annotations(true),
  }, ({ task_id }) => ({ op: 'tasks.status', task_id }));

  register(server, client, 'audio_cancel_task', {
    title: 'Cancel an audio task',
    description: 'Request cooperative cancellation of a submitted task. Cancellation does not roll back files already created by nodes.',
    inputSchema: taskInput,
    annotations: annotations(false, true, true),
  }, ({ task_id }) => ({ op: 'tasks.cancel', task_id }));

  register(server, client, 'audio_task_result', {
    title: 'Get audio task result',
    description: 'Get the terminal task result. Pending tasks return task_not_finished. A failed or cancelled terminal task is returned with isError while preserving node/field diagnostics.',
    inputSchema: taskInput,
    annotations: annotations(true),
  }, ({ task_id }) => ({ op: 'tasks.result', task_id }), value => value,
  envelope => envelope?.success === false || envelope?.data?.state === 'failed' || envelope?.data?.state === 'cancelled');

  register(server, client, 'audio_release_task', {
    title: 'Release an audio task record',
    description: 'Release a terminal task record from backend memory. This does not delete output files.',
    inputSchema: taskInput,
    annotations: annotations(false, true, true),
  }, ({ task_id }) => ({ op: 'tasks.release', task_id }));

  return server;
}
