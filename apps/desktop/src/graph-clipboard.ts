import type { GraphDocument } from "./model";
import { positionKey, type Point } from "./graph-canvas-layout";

export type GraphClipboard = {
  nodes: GraphDocument["nodes"];
  connections: GraphDocument["connections"];
  exports: NonNullable<GraphDocument["exports"]>;
  positions: Record<string, Point>;
};
const clone = <T>(value: T): T => structuredClone(value);

export function copyGraphSelection(
  graph: GraphDocument, ids: Iterable<string>, positions: Record<string, Point>,
): GraphClipboard {
  const selected = new Set(ids);
  const nodes = graph.nodes.filter((node) => selected.has(node.id));
  const actualIds = new Set(nodes.map((node) => node.id));
  const layout: Record<string, Point> = {};
  for (const node of nodes) {
    const key = positionKey(node);
    if (Object.hasOwn(positions, key)) layout[key] = clone(positions[key]);
  }
  return clone({ nodes,
    connections: graph.connections.filter((edge) => actualIds.has(edge.from.node) && actualIds.has(edge.to.node)),
    exports: (graph.exports ?? []).filter((item) => actualIds.has(item.node)),
    positions: layout,
  });
}

function unique(base: string, used: Set<string>) {
  let candidate = `${base}_copy`;
  let suffix = 2;
  while (used.has(candidate)) candidate = `${base}_copy${suffix++}`;
  used.add(candidate);
  return candidate;
}

/** Offset is supplied by the caller so repeated pastes can advance without mutating the clipboard. */
export function pasteGraphSelection(
  graph: GraphDocument, clipboard: GraphClipboard, positions: Record<string, Point>, offset = 32,
): { graph: GraphDocument; positions: Record<string, Point>; selectedIds: string[] } {
  const result = clone(graph);
  const layout = clone(positions);
  const usedIds = new Set(graph.nodes.map((node) => node.id));
  const usedNames = new Set((graph.exports ?? []).map((item) => item.name));
  const mapping = new Map<string, string>();
  const nodes = clipboard.nodes.map((original, index) => {
    const node = clone(original);
    node.id = unique(original.id, usedIds);
    mapping.set(original.id, node.id);
    const key = positionKey(original);
    const point = Object.hasOwn(clipboard.positions, key) ? clipboard.positions[key] : { x: 36, y: 36 + index * 160 };
    layout[positionKey(node)] = { x: point.x + offset, y: point.y + offset };
    return node;
  });
  result.nodes.push(...nodes);
  result.connections.push(...clipboard.connections
    .filter((edge) => mapping.has(edge.from.node) && mapping.has(edge.to.node))
    .map((original) => {
      const edge = clone(original);
      edge.from.node = mapping.get(original.from.node)!;
      edge.to.node = mapping.get(original.to.node)!;
      return edge;
    }));
  const exports = clipboard.exports.filter((item) => mapping.has(item.node)).map((original) => {
    const item = clone(original);
    item.node = mapping.get(original.node)!;
    item.name = usedNames.has(item.name) ? unique(item.name, usedNames) : item.name;
    usedNames.add(item.name);
    return item;
  });
  if (exports.length) result.exports = [...(result.exports ?? []), ...exports];
  return { graph: result, positions: layout, selectedIds: nodes.map((node) => node.id) };
}
