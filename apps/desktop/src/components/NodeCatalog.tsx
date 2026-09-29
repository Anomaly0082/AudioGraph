import type { NodeInfo } from "../types/desktop";

export default function NodeCatalog({
  nodes,
  search,
  onSearch,
  selectedId,
  onSelect,
}: {
  nodes: NodeInfo[];
  search: string;
  onSearch: (value: string) => void;
  selectedId: string;
  onSelect: (id: string) => void;
}) {
  const filtered = nodes.filter((item) =>
    `${item.typeId} ${item.displayName}`
      .toLowerCase()
      .includes(search.toLowerCase()),
  );
  const node = nodes.find((item) => item.typeId === selectedId);
  const capability = node?.realtime_capabilities;
  return (
    <section className="panel capabilities-panel" aria-labelledby="nodes-title">
      <div className="section-heading">
        <h2 id="nodes-title">节点目录</h2>
        <span className="badge">{nodes.length} 个</span>
      </div>
      <input
        aria-label="搜索节点"
        placeholder="搜索名称或类型"
        value={search}
        onChange={(event) => onSearch(event.target.value)}
      />
      <div className="node-picker">
        {filtered.map((item) => (
          <button
            key={item.typeId}
            type="button"
            className={item.typeId === selectedId ? "selected" : ""}
            onClick={() => onSelect(item.typeId)}
          >
            <span>{item.displayName}</span>
            <code>{item.typeId}</code>
          </button>
        ))}
      </div>
      {nodes.length > 0 && filtered.length === 0 && (
        <p className="hint">没有匹配的节点。</p>
      )}
      {!nodes.length && (
        <p className="empty-state">打开工作区后，读取真实节点能力。</p>
      )}
      {node && (
        <div className="node-detail">
          <h3>{node.displayName}</h3>
          <code>{node.typeId}</code>
          <span className="badge">{node.execution_domain}</span>
          <p>{node.description}</p>
          {capability && (
            <div className="parameter-card">
              <h4>实时约束</h4>
              <small>
                {capability.format.sample_rate} Hz ·{" "}
                {capability.format.channels} 声道
              </small>
              <small>最大块长 {capability.maximum_block_frames} 帧</small>
            </div>
          )}
          <h4>输入与输出</h4>
          <ul className="port-list">
            {node.inputs.map((port) => (
              <li key={`in-${port.id}`}>
                <span>输入</span>
                <code>{port.id}</code>
                <b>{port.type}</b>
                {port.required && <small>必需</small>}
              </li>
            ))}
            {node.outputs.map((port) => (
              <li key={`out-${port.id}`}>
                <span>输出</span>
                <code>{port.id}</code>
                <b>{port.type}</b>
              </li>
            ))}
          </ul>
          <h4>参数</h4>
          {!node.parameters?.length && <p className="hint">无可调参数。</p>}
          {node.parameters?.map((parameter) => (
            <div className="parameter-card" key={parameter.id}>
              <div>
                <code>{parameter.id}</code>
                <span>
                  {parameter.type}
                  {parameter.integer_only ? " · 整数" : ""}
                  {parameter.required ? " · 必填" : ""}
                </span>
              </div>
              <p>{parameter.description}</p>
              {parameter.default !== undefined && (
                <small>默认 {JSON.stringify(parameter.default)}</small>
              )}
              {(parameter.minimum !== undefined ||
                parameter.maximum !== undefined) && (
                <small>
                  范围 {parameter.minimum ?? "不限"} ～{" "}
                  {parameter.maximum ?? "不限"} {parameter.unit}
                </small>
              )}
              {parameter.enum && <small>{parameter.enum.join(" / ")}</small>}
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
