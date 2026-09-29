import type { GraphDocument } from "../model";
import type { NodeInfo } from "../types/desktop";
import { graphConnections } from "../presentation";
import Disclosure from "./Disclosure";

export default function GraphOverview({
  graph,
  nodes = [],
}: {
  graph: GraphDocument;
  nodes?: NodeInfo[];
}) {
  return (
    <div className="graph-overview">
      <div className="node-chips" aria-label="节点概览">
        {graph.nodes.map((node) => (
          <div className="node-chip" key={node.id}>
            <strong>
              {nodes.find((item) => item.typeId === node.type)?.displayName ??
                node.type}
            </strong>
            <small>{node.id}</small>
          </div>
        ))}
      </div>
      <p className="hint">
        {graph.nodes.length} 个节点 · {graph.connections.length} 条连接
      </p>
      <Disclosure label="连接">
        {graph.connections.length ? (
          <ul className="connection-list">
            {graphConnections(graph).map((link, index) => (
              <li key={index}>
                <code>{link}</code>
              </li>
            ))}
          </ul>
        ) : (
          <p className="hint">没有节点间连接。</p>
        )}
      </Disclosure>
    </div>
  );
}
