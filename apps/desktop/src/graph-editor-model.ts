import type { GraphDocument, Mode } from "./model";
import type { NodeInfo } from "./types/desktop";

export type GraphEndpoint = { node: string; port: string };

function requireNode(graph: GraphDocument, nodeId: string) {
  const node = graph.nodes.find((item) => item.id === nodeId);
  if (!node) throw new Error(`未知节点：${nodeId}`);
  return node;
}

function requireIndex(index: number, length: number, kind: string): void {
  if (!Number.isInteger(index) || index < 0 || index >= length)
    throw new Error(`${kind}索引无效：${index}`);
}

export function isNodeAvailableInMode(
  descriptor: NodeInfo,
  mode: Mode,
): boolean {
  const domain = {
    offline: "synchronous",
    streaming: "streaming",
    realtime: "realtime",
  }[mode];
  return descriptor.execution_domain === domain;
}

export function addGraphNode(
  graph: GraphDocument,
  descriptor: NodeInfo,
): { graph: GraphDocument; nodeId: string } {
  if (!descriptor.typeId || descriptor.typeId.includes("\0"))
    throw new Error("节点类型不能为空。");
  const existing = new Set(graph.nodes.map((node) => node.id));
  let nodeId = descriptor.typeId;
  for (let suffix = 2; existing.has(nodeId); suffix++)
    nodeId = `${descriptor.typeId}_${suffix}`;
  const defaults = Object.fromEntries(
    (descriptor.parameters ?? [])
      .filter(
        (parameter) =>
          Object.hasOwn(parameter, "default") &&
          parameter.default !== undefined,
      )
      .map((parameter) => [parameter.id, structuredClone(parameter.default)]),
  );
  const node = {
    id: nodeId,
    type: descriptor.typeId,
    ...(Object.keys(defaults).length ? { parameters: defaults } : {}),
  };
  return { graph: { ...graph, nodes: [...graph.nodes, node] }, nodeId };
}

export function removeGraphNode(
  graph: GraphDocument,
  nodeId: string,
): GraphDocument {
  requireNode(graph, nodeId);
  return {
    ...graph,
    nodes: graph.nodes.filter((node) => node.id !== nodeId),
    connections: graph.connections.filter(
      (edge) => edge.from.node !== nodeId && edge.to.node !== nodeId,
    ),
    ...(graph.exports === undefined
      ? {}
      : { exports: graph.exports.filter((item) => item.node !== nodeId) }),
  };
}

export function setNodeParameter(
  graph: GraphDocument,
  nodeId: string,
  parameterId: string,
  value: unknown,
): GraphDocument {
  requireNode(graph, nodeId);
  if (!parameterId || parameterId.includes("\0"))
    throw new Error("参数名称不能为空。");
  return {
    ...graph,
    nodes: graph.nodes.map((node) => {
      if (node.id !== nodeId) return node;
      const parameters = { ...node.parameters };
      if (value === undefined) delete parameters[parameterId];
      else
        Object.defineProperty(parameters, parameterId, {
          value,
          enumerable: true,
          configurable: true,
          writable: true,
        });
      const updated = { ...node };
      if (Object.keys(parameters).length) updated.parameters = parameters;
      else delete updated.parameters;
      return updated;
    }),
  };
}

export function connectGraphPorts(
  graph: GraphDocument,
  descriptors: NodeInfo[],
  from: GraphEndpoint,
  to: GraphEndpoint,
): GraphDocument {
  const source = requireNode(graph, from.node);
  const target = requireNode(graph, to.node);
  if (from.node === to.node) throw new Error("节点不能连接到自身。");
  const sourceDescriptor = descriptors.find(
    (item) => item.typeId === source.type,
  );
  const targetDescriptor = descriptors.find(
    (item) => item.typeId === target.type,
  );
  if (!sourceDescriptor) throw new Error(`未知节点类型：${source.type}`);
  if (!targetDescriptor) throw new Error(`未知节点类型：${target.type}`);
  if (sourceDescriptor.execution_domain !== targetDescriptor.execution_domain)
    throw new Error("不同执行域的节点不能连接。");
  const output = sourceDescriptor.outputs.find((port) => port.id === from.port);
  if (!output) throw new Error(`未知输出端口：${from.node}.${from.port}`);
  const input = targetDescriptor.inputs.find((port) => port.id === to.port);
  if (!input) throw new Error(`未知输入端口：${to.node}.${to.port}`);
  if (output.type !== input.type)
    throw new Error(`端口类型不匹配：${output.type} → ${input.type}`);
  if (
    graph.connections.some(
      (edge) =>
        edge.from.node === from.node &&
        edge.from.port === from.port &&
        edge.to.node === to.node &&
        edge.to.port === to.port,
    )
  )
    throw new Error("连接已存在。");
  if (
    graph.connections.some(
      (edge) => edge.to.node === to.node && edge.to.port === to.port,
    )
  )
    throw new Error(`输入端口已连接：${to.node}.${to.port}`);

  const visited = new Set<string>();
  const stack = [to.node];
  while (stack.length) {
    const current = stack.pop()!;
    if (current === from.node) throw new Error("连接会形成环路。");
    if (visited.has(current)) continue;
    visited.add(current);
    for (const edge of graph.connections) {
      if (edge.from.node === current) stack.push(edge.to.node);
    }
  }
  return {
    ...graph,
    connections: [...graph.connections, { from: { ...from }, to: { ...to } }],
  };
}

export function rewireGraphPorts(
  graph: GraphDocument,
  descriptors: NodeInfo[],
  from: GraphEndpoint,
  to: GraphEndpoint,
  originalIndex?: number,
): GraphDocument {
  let original: GraphDocument["connections"][number] | undefined;
  if (originalIndex !== undefined) {
    requireIndex(originalIndex, graph.connections.length, "连接");
    original = graph.connections[originalIndex];
    if (original.from.node !== from.node || original.from.port !== from.port)
      throw new Error("原连接已更改，无法重新连接。");
    if (original.to.node === to.node && original.to.port === to.port)
      return graph;
  }

  const exactEdge = graph.connections.find(
    (edge) =>
      edge.from.node === from.node &&
      edge.from.port === from.port &&
      edge.to.node === to.node &&
      edge.to.port === to.port,
  );
  if (exactEdge && originalIndex === undefined) return graph;

  const remaining = graph.connections.filter(
    (edge, index) =>
      index !== originalIndex &&
      !(edge.to.node === to.node && edge.to.port === to.port),
  );
  // Validation runs against a temporary graph. A rejection cannot alter the caller's graph.
  const connected = connectGraphPorts(
    { ...graph, connections: remaining },
    descriptors,
    from,
    to,
  );
  if (!original) return connected;
  return {
    ...connected,
    connections: [
      ...connected.connections.slice(0, -1),
      { ...original, from: { ...from }, to: { ...to } },
    ],
  };
}

export function removeGraphConnection(
  graph: GraphDocument,
  index: number,
): GraphDocument {
  requireIndex(index, graph.connections.length, "连接");
  return {
    ...graph,
    connections: graph.connections.filter((_, position) => position !== index),
  };
}

export function setGraphExport(
  graph: GraphDocument,
  nodeId: string,
  portId: string,
  name: string,
): GraphDocument {
  requireNode(graph, nodeId);
  if (!portId || portId.includes("\0")) throw new Error("导出端口不能为空。");
  if (!name.trim() || name.includes("\0"))
    throw new Error("导出名称不能为空。");
  if (graph.exports?.some((item) => item.name === name))
    throw new Error(`导出名称重复：${name}`);
  return {
    ...graph,
    exports: [...(graph.exports ?? []), { name, node: nodeId, port: portId }],
  };
}

export function removeGraphExport(
  graph: GraphDocument,
  index: number,
): GraphDocument {
  requireIndex(index, graph.exports?.length ?? 0, "导出");
  return {
    ...graph,
    exports: graph.exports!.filter((_, position) => position !== index),
  };
}
