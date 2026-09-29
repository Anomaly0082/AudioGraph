import type { GraphDocument, GraphNode } from "./model";
import type { NodeInfo } from "./types/desktop";

export type Point = { x: number; y: number };
export type CanvasView = {
  positions: Record<string, Point>;
  selectedId: string;
  zoom: number;
  pan: Point;
};
export const initialCanvasView = (): CanvasView => ({
  positions: {},
  selectedId: "",
  zoom: 1,
  pan: { x: 0, y: 0 },
});
export const CARD_WIDTH = 248;
export const HEADER_HEIGHT = 66;
export const PORT_HEIGHT = 30;
// JSON IDs can contain prototype names. Own-key lookups and composite keys avoid
// treating Object.prototype properties as saved layout or graph vertices.
export const positionKey = (node: GraphNode) =>
  JSON.stringify([node.id, node.type]);

export function nodePorts(
  graph: GraphDocument,
  node: GraphNode,
  descriptor?: NodeInfo,
) {
  const infer = (side: "from" | "to") => {
    const ids = graph.connections
      .filter((edge) => edge[side].node === node.id)
      .map((edge) => edge[side].port);
    if (side === "from")
      ids.push(
        ...(graph.exports ?? [])
          .filter((item) => item.node === node.id)
          .map((item) => item.port),
      );
    return [...new Set(ids)].map((id) => ({
      id,
      type: "未知",
      required: false,
    }));
  };
  // Include unrecognized existing ports, so opening an invalid/future Graph never hides its edges.
  const merge = (
    ports: NodeInfo["inputs"],
    fallback: ReturnType<typeof infer>,
  ) => [
    ...ports,
    ...fallback.filter((port) => !ports.some((known) => known.id === port.id)),
  ];
  return {
    inputs: merge(descriptor?.inputs ?? [], infer("to")),
    outputs: merge(descriptor?.outputs ?? [], infer("from")),
  };
}

export function autoLayout(
  graph: GraphDocument,
  descriptors: NodeInfo[],
): Record<string, Point> {
  const ids = new Set(graph.nodes.map((node) => node.id));
  const indegree = new Map(graph.nodes.map((node) => [node.id, 0]));
  const successors = new Map(
    graph.nodes.map((node) => [node.id, new Set<string>()]),
  );
  const ranks = new Map(graph.nodes.map((node) => [node.id, 0]));
  for (const edge of graph.connections) {
    if (!ids.has(edge.from.node) || !ids.has(edge.to.node)) continue;
    const targets = successors.get(edge.from.node)!;
    if (!targets.has(edge.to.node)) {
      targets.add(edge.to.node);
      indegree.set(edge.to.node, indegree.get(edge.to.node)! + 1);
    }
  }
  const queue = graph.nodes
    .filter((node) => indegree.get(node.id) === 0)
    .map((node) => node.id);
  const visited = new Set<string>();
  for (let cursor = 0; cursor < queue.length; cursor++) {
    const id = queue[cursor];
    visited.add(id);
    for (const target of successors.get(id)!) {
      ranks.set(target, Math.max(ranks.get(target)!, ranks.get(id)! + 1));
      indegree.set(target, indegree.get(target)! - 1);
      if (indegree.get(target) === 0) queue.push(target);
    }
  }
  const nextY = new Map<number, number>();
  const result: Record<string, Point> = {};
  for (const node of graph.nodes) {
    const rank = visited.has(node.id) ? ranks.get(node.id)! : 0;
    const ports = nodePorts(
      graph,
      node,
      descriptors.find((item) => item.typeId === node.type),
    );
    const height =
      HEADER_HEIGHT +
      Math.max(ports.inputs.length, ports.outputs.length, 1) * PORT_HEIGHT +
      18;
    result[positionKey(node)] = {
      x: 36 + rank * (CARD_WIDTH + 100),
      y: nextY.get(rank) ?? 36,
    };
    nextY.set(rank, (nextY.get(rank) ?? 36) + height + 54);
  }
  return result;
}

export function resolvedPositions(
  graph: GraphDocument,
  descriptors: NodeInfo[],
  saved: Record<string, Point>,
) {
  const defaults = autoLayout(graph, descriptors);
  for (const node of graph.nodes) {
    const key = positionKey(node);
    const point = Object.hasOwn(saved, key) ? saved[key] : undefined;
    if (point && Number.isFinite(point.x) && Number.isFinite(point.y))
      defaults[key] = { x: Math.max(12, point.x), y: Math.max(12, point.y) };
  }
  return defaults;
}
