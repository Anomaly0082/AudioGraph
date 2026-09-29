import { useEffect, useState } from "react";
import type { GraphDocument, GraphNode } from "../model";
import type { DeviceCatalog, NodeInfo } from "../types/desktop";

type Parameter = NonNullable<NodeInfo["parameters"]>[number];

type Props = {
  node: GraphNode;
  descriptor: NodeInfo | undefined;
  devices: DeviceCatalog;
  canListDevices: boolean;
  onListDevices: () => void;
  onParameter: (id: string, value: unknown) => void;
  onDelete: () => void;
  exports: NonNullable<GraphDocument["exports"]>;
  onExport: (port: string, name: string) => void;
  onRemoveExport: (index: number) => void;
};

function printable(value: unknown): string {
  if (value === undefined) return "未设置";
  try {
    return JSON.stringify(value, null, 2) ?? String(value);
  } catch {
    return String(value);
  }
}

function editable(parameter: Parameter, value: unknown): boolean {
  const type = parameter.type.toLowerCase();
  if (value === undefined)
    return ["number", "integer", "text", "file_path", "boolean"].includes(type);
  if (type === "number" || type === "integer")
    return typeof value === "number" && Number.isFinite(value);
  if (type === "boolean") return typeof value === "boolean";
  if (type === "text" || type === "file_path") return typeof value === "string";
  return false;
}

export function parameterDraft(
  parameter: Parameter,
  value: unknown,
): string | boolean {
  const initial = value === undefined ? parameter.default : value;
  if (parameter.type.toLowerCase() === "boolean")
    return typeof initial === "boolean" ? initial : false;
  return initial === undefined ? "" : String(initial);
}

export function parseParameterDraft(
  parameter: Parameter,
  draft: string | boolean,
):
  | { ok: true; value: string | number | boolean }
  | { ok: false; error: string } {
  const type = parameter.type.toLowerCase();
  if (type === "boolean") {
    return typeof draft === "boolean"
      ? { ok: true, value: draft }
      : { ok: false, error: "值的类型无效。" };
  }
  if (typeof draft !== "string") return { ok: false, error: "值的类型无效。" };
  if (type === "number" || type === "integer") {
    if (!draft.trim()) return { ok: false, error: "请输入数值。" };
    const number = Number(draft);
    if (!Number.isFinite(number))
      return { ok: false, error: "请输入有效数值。" };
    if (
      (type === "integer" || parameter.integer_only) &&
      !Number.isInteger(number)
    )
      return { ok: false, error: "请输入整数。" };
    if (parameter.minimum !== undefined && number < parameter.minimum)
      return { ok: false, error: `不能小于 ${parameter.minimum}。` };
    if (parameter.maximum !== undefined && number > parameter.maximum)
      return { ok: false, error: `不能大于 ${parameter.maximum}。` };
    return { ok: true, value: number };
  }
  if (type === "file_path" && !draft.trim())
    return { ok: false, error: "请输入路径。" };
  if (parameter.id === "device_id" && !draft.trim())
    return { ok: false, error: "请选择设备。" };
  if (parameter.enum?.length && !parameter.enum.includes(draft))
    return { ok: false, error: "请选择列表中的值。" };
  return { ok: true, value: draft };
}

function ParameterField({
  nodeId,
  nodeType,
  parameter,
  value,
  hasValue,
  devices,
  canListDevices,
  onListDevices,
  onParameter,
}: {
  nodeId: string;
  nodeType: string;
  parameter: Parameter;
  value: unknown;
  hasValue: boolean;
  devices: DeviceCatalog;
  canListDevices: boolean;
  onListDevices: () => void;
  onParameter: Props["onParameter"];
}) {
  const kind = parameter.type.toLowerCase();
  const initial = value === undefined ? parameter.default : value;
  const [draft, setDraft] = useState<string | boolean>(() =>
    parameterDraft(parameter, value),
  );
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setDraft(parameterDraft(parameter, value));
    setError(null);
  }, [nodeId, parameter.id, parameter.default, value]);

  const isDevice =
    parameter.id === "device_id" &&
    (nodeType === "realtime_input" || nodeType === "realtime_output") &&
    (kind === "text" || kind === "file_path");
  const choices =
    nodeType === "realtime_input" ? devices.inputs : devices.outputs;
  const currentDeviceKnown = choices.some((device) => device.id === draft);
  const canEdit = editable(parameter, value) && editable(parameter, initial);
  const enumOptions = parameter.enum ?? [];
  const candidate = parseParameterDraft(parameter, draft);

  function apply() {
    if (!candidate.ok) {
      setError(candidate.error);
      return;
    }
    setError(null);
    if (candidate.value !== value) onParameter(parameter.id, candidate.value);
  }

  return (
    <div className="inspector-field">
      <div className="inspector-field-head">
        <label htmlFor={`node-parameter-${nodeId}-${parameter.id}`}>
          {parameter.id}
          {parameter.required && (
            <span className="inspector-required" title="必填">
              {" "}
              *
            </span>
          )}
        </label>
        {parameter.unit && <small>{parameter.unit}</small>}
      </div>
      {!canEdit ? (
        <pre className="inspector-readonly">{printable(value)}</pre>
      ) : (
        <>
          <div className="inspector-field-edit">
            {isDevice ? (
              <select
                id={`node-parameter-${nodeId}-${parameter.id}`}
                value={String(draft)}
                onChange={(event) => {
                  setDraft(event.target.value);
                  setError(null);
                }}
              >
                <option value="">请选择设备</option>
                {String(draft) && !currentDeviceKnown && (
                  <option value={String(draft)}>
                    {String(draft)}（当前值）
                  </option>
                )}
                {choices.map((device) => (
                  <option value={device.id} key={device.id}>
                    {device.name}
                    {device.is_default ? "（默认）" : ""}
                  </option>
                ))}
              </select>
            ) : enumOptions.length ? (
              <select
                id={`node-parameter-${nodeId}-${parameter.id}`}
                value={String(draft)}
                onChange={(event) => {
                  setDraft(event.target.value);
                  setError(null);
                }}
              >
                <option value="">请选择</option>
                {String(draft) && !enumOptions.includes(String(draft)) && (
                  <option value={String(draft)}>
                    {String(draft)}（当前值）
                  </option>
                )}
                {enumOptions.map((option) => (
                  <option value={option} key={option}>
                    {option}
                  </option>
                ))}
              </select>
            ) : kind === "boolean" ? (
              <input
                id={`node-parameter-${nodeId}-${parameter.id}`}
                type="checkbox"
                checked={Boolean(draft)}
                onChange={(event) => {
                  setDraft(event.target.checked);
                  setError(null);
                }}
              />
            ) : (
              <input
                id={`node-parameter-${nodeId}-${parameter.id}`}
                type={
                  kind === "number" || kind === "integer" ? "number" : "text"
                }
                step={kind === "integer" || parameter.integer_only ? 1 : "any"}
                min={parameter.minimum}
                max={parameter.maximum}
                value={String(draft)}
                onChange={(event) => {
                  setDraft(event.target.value);
                  setError(null);
                }}
              />
            )}
            <button
              type="button"
              onClick={apply}
              disabled={candidate.ok && candidate.value === value}
            >
              应用
            </button>
          </div>
          {isDevice && (
            <button
              type="button"
              className="text-button inspector-refresh"
              onClick={onListDevices}
              disabled={!canListDevices}
            >
              刷新设备列表
            </button>
          )}
          {error && (
            <small className="inspector-error" role="alert">
              {error}
            </small>
          )}
        </>
      )}
      {hasValue && (
        <button
          type="button"
          className="text-button inspector-clear"
          onClick={() => onParameter(parameter.id, undefined)}
        >
          清除
        </button>
      )}
      {!hasValue && (
        <small className="inspector-missing">
          未设置
          {parameter.default !== undefined
            ? ` · 默认 ${printable(parameter.default)}`
            : parameter.required
              ? " · 必填"
              : ""}
        </small>
      )}
      {parameter.description && (
        <small className="inspector-description">{parameter.description}</small>
      )}
      {(parameter.minimum !== undefined || parameter.maximum !== undefined) && (
        <small className="inspector-description">
          范围 {parameter.minimum ?? "不限"}～{parameter.maximum ?? "不限"}
          {parameter.integer_only ? " · 整数" : ""}
        </small>
      )}
    </div>
  );
}

export default function NodeInspector({
  node,
  descriptor,
  devices,
  canListDevices,
  onListDevices,
  onParameter,
  onDelete,
  exports,
  onExport,
  onRemoveExport,
}: Props) {
  const outputs = descriptor?.outputs ?? [];
  const [port, setPort] = useState(outputs[0]?.id ?? "");
  const [name, setName] = useState("");
  const [exportError, setExportError] = useState<string | null>(null);

  useEffect(() => {
    setPort(outputs[0]?.id ?? "");
    setName("");
    setExportError(null);
  }, [node.id, descriptor]);

  function addExport() {
    const clean = name.trim();
    if (!port || !outputs.some((output) => output.id === port)) {
      setExportError("请选择输出端口。");
      return;
    }
    if (!clean) {
      setExportError("请输入导出名称。");
      return;
    }
    if (exports.some((item) => item.name === clean)) {
      setExportError("导出名称已存在。");
      return;
    }
    onExport(port, clean);
    setName("");
    setExportError(null);
  }

  const knownParameters = new Set(
    descriptor?.parameters?.map((item) => item.id) ?? [],
  );
  const unknownParameters = Object.entries(node.parameters ?? {}).filter(
    ([id]) => !knownParameters.has(id),
  );
  const nodeExports = exports
    .map((item, index) => ({ item, index }))
    .filter(({ item }) => item.node === node.id);

  return (
    <section className="node-inspector" aria-label="节点检查器">
      <div className="inspector-header">
        <div>
          <h2>{descriptor?.displayName ?? node.type}</h2>
          <code>{node.id}</code>
        </div>
        <button type="button" className="danger" onClick={onDelete}>
          删除节点
        </button>
      </div>
      <small className="inspector-type">{node.type}</small>

      <div className="inspector-section">
        <h3>参数</h3>
        {descriptor?.parameters?.map((parameter) => (
          <ParameterField
            key={`${node.id}:${parameter.id}`}
            nodeId={node.id}
            nodeType={node.type}
            parameter={parameter}
            value={
              node.parameters && Object.hasOwn(node.parameters, parameter.id)
                ? node.parameters[parameter.id]
                : undefined
            }
            hasValue={Boolean(
              node.parameters && Object.hasOwn(node.parameters, parameter.id),
            )}
            devices={devices}
            canListDevices={canListDevices}
            onListDevices={onListDevices}
            onParameter={onParameter}
          />
        ))}
        {unknownParameters.map(([id, value]) => (
          <div className="inspector-field" key={id}>
            <span>{id}</span>
            <pre className="inspector-readonly">{printable(value)}</pre>
          </div>
        ))}
        {!descriptor && (
          <p className="hint">节点类型未识别；现有参数仅供查看。</p>
        )}
        {descriptor &&
          !descriptor.parameters?.length &&
          !unknownParameters.length && <p className="hint">无参数。</p>}
      </div>

      <div className="inspector-section">
        <h3>导出</h3>
        {nodeExports.map(({ item, index }) => (
          <div className="inspector-export" key={index}>
            <span>
              <strong>{item.name}</strong>
              <small>{item.port}</small>
            </span>
            <button
              type="button"
              onClick={() => onRemoveExport(index)}
              aria-label={`删除导出 ${item.name}`}
            >
              删除
            </button>
          </div>
        ))}
        {!nodeExports.length && <p className="hint">无导出。</p>}
        {!!outputs.length && (
          <div className="inspector-add-export">
            <label htmlFor="inspector-output-port">输出端口</label>
            <select
              id="inspector-output-port"
              value={port}
              onChange={(event) => setPort(event.target.value)}
            >
              {outputs.map((output) => (
                <option key={output.id} value={output.id}>
                  {output.id}
                </option>
              ))}
            </select>
            <label htmlFor="inspector-export-name">导出名称</label>
            <div className="inspector-field-edit">
              <input
                id="inspector-export-name"
                value={name}
                onChange={(event) => {
                  setName(event.target.value);
                  setExportError(null);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter") addExport();
                }}
              />
              <button type="button" onClick={addExport}>
                添加
              </button>
            </div>
            {exportError && (
              <small className="inspector-error" role="alert">
                {exportError}
              </small>
            )}
          </div>
        )}
      </div>
    </section>
  );
}
