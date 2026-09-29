import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent,
} from "react";
import type { GraphDocument } from "../model";
import type { NodeInfo } from "../types/desktop";
import {
  autoLayout,
  CARD_WIDTH,
  HEADER_HEIGHT,
  PORT_HEIGHT,
  nodePorts,
  positionKey,
  resolvedPositions,
  type CanvasView,
  type Point,
} from "../graph-canvas-layout";
import {
  panAfterDrag,
  portColor,
  screenToCanvas,
  wirePath,
  zoomAround,
} from "../canvas-interaction";
import { rewireGraphPorts } from "../graph-editor-model";

type Endpoint = { node: string; port: string };
type Socket = Endpoint & { side: "input" | "output" };
type Pending = { socket: Socket; originalIndex?: number };
type Props = {
  graph: GraphDocument;
  nodes: NodeInfo[];
  view: CanvasView;
  onView: (value: CanvasView) => void;
  disabled?: boolean;
  connectionEditReason?: string;
  onConnect: (from: Endpoint, to: Endpoint, originalIndex?: number) => void;
  onDisconnect: (index: number) => void;
};
type Gesture =
  | {
      kind: "node";
      pointerId: number;
      key: string;
      start: Point;
      origin: Point;
    }
  | { kind: "pan"; pointerId: number; start: Point; origin: Point }
  | { kind: "wire"; pointerId: number; start: Point };
const endpoint = ({ node, port }: Endpoint): Endpoint => ({ node, port });
const socketKey = (socket: Socket) =>
  JSON.stringify([socket.node, socket.port, socket.side]);

export default function GraphCanvas({
  graph,
  nodes,
  view,
  onView,
  disabled,
  connectionEditReason,
  onConnect,
  onDisconnect,
}: Props) {
  // Inferred ports are display-only: deleting the last edge would erase their
  // only source of metadata. Wiring requires the actual backend catalog.
  const wiringReason =
    connectionEditReason ||
    (nodes.length === 0 ? "打开工作区后可编辑连线" : "");
  const wiringDisabled = !!disabled || !!wiringReason;
  const catalogKey = JSON.stringify(nodes);
  const viewport = useRef<HTMLDivElement>(null);
  const gesture = useRef<Gesture | null>(null);
  const [panning, setPanning] = useState(false);
  const [pending, setPending] = useState<Pending | null>(null);
  const pendingRef = useRef<Pending | null>(null);
  const [pointer, setPointer] = useState<Point | null>(null);
  const [hover, setHover] = useState<{ key: string; valid: boolean } | null>(
    null,
  );
  const [edgeIndex, setEdgeIndex] = useState<number | null>(null);
  const pan = view.pan ?? { x: 0, y: 0 };
  const positions = useMemo(
    () => resolvedPositions(graph, nodes, view.positions),
    [graph, nodes, view.positions],
  );
  const cards = graph.nodes.map((node) => {
    const descriptor = nodes.find((item) => item.typeId === node.type);
    const ports = nodePorts(graph, node, descriptor);
    return {
      node,
      descriptor,
      ...ports,
      point: positions[positionKey(node)],
      height:
        HEADER_HEIGHT +
        Math.max(ports.inputs.length, ports.outputs.length, 1) * PORT_HEIGHT +
        18,
    };
  });
  const width = Math.max(
    850,
    ...cards.map((card) => card.point.x + CARD_WIDTH + 80),
  );
  const height = Math.max(
    460,
    ...cards.map((card) => card.point.y + card.height + 80),
  );
  // Camera survives page navigation; only a NEW selection may bring a node into view.
  const previousSelected = useRef(view.selectedId);
  useEffect(() => {
    if (previousSelected.current === view.selectedId) return;
    previousSelected.current = view.selectedId;
    const card = cards.find((item) => item.node.id === view.selectedId);
    const box = viewport.current;
    if (!box || !card || gesture.current?.kind === "node") return;
    const left = card.point.x * view.zoom + pan.x,
      top = card.point.y * view.zoom + pan.y;
    let x = pan.x,
      y = pan.y;
    if (left < 0 || left + CARD_WIDTH * view.zoom > box.clientWidth)
      x = 30 - card.point.x * view.zoom;
    if (top < 0 || top + card.height * view.zoom > box.clientHeight)
      y = 30 - card.point.y * view.zoom;
    if (x !== pan.x || y !== pan.y) onView({ ...view, pan: { x, y } });
  }, [view.selectedId]);
  function clearConnection() {
    pendingRef.current = null;
    setPending(null);
    setPointer(null);
    setHover(null);
    gesture.current = null;
    setPanning(false);
  }
  useEffect(() => {
    clearConnection();
    setEdgeIndex(null);
  }, [graph, disabled, wiringDisabled, catalogKey]);
  function socketPosition(socket: Socket) {
    const card = cards.find((item) => item.node.id === socket.node);
    const ports = socket.side === "input" ? card?.inputs : card?.outputs;
    const index = ports?.findIndex((port) => port.id === socket.port) ?? -1;
    if (!card || index < 0) return null;
    return {
      x: card.point.x + (socket.side === "output" ? CARD_WIDTH : 0),
      y: card.point.y + HEADER_HEIGHT + index * PORT_HEIGHT + PORT_HEIGHT / 2,
    };
  }
  function socketType(socket: Socket) {
    const card = cards.find((item) => item.node.id === socket.node);
    return (
      (socket.side === "input" ? card?.inputs : card?.outputs)?.find(
        (port) => port.id === socket.port,
      )?.type ?? ""
    );
  }
  function targetAt(clientX: number, clientY: number): Socket | null {
    const target = document
      .elementFromPoint(clientX, clientY)
      ?.closest<HTMLElement>("[data-port-side]");
    if (!target || !viewport.current?.contains(target)) return null;
    return {
      node: target.dataset.portNode!,
      port: target.dataset.portId!,
      side: target.dataset.portSide as Socket["side"],
    };
  }
  function endpoints(start: Pending, target: Socket) {
    if (start.socket.side === target.side) return null;
    return start.socket.side === "output"
      ? { from: endpoint(start.socket), to: endpoint(target) }
      : { from: endpoint(target), to: endpoint(start.socket) };
  }
  function finishConnection(target: Socket) {
    const start = pendingRef.current;
    if (!start) return;
    const pair = endpoints(start, target);
    if (!pair) return;
    clearConnection();
    if (!wiringDisabled) onConnect(pair.from, pair.to, start.originalIndex);
  }
  function beginOrFinish(socket: Socket) {
    if (wiringDisabled) return;
    const previous = pendingRef.current;
    if (previous && endpoints(previous, socket)) {
      finishConnection(socket);
      return;
    }
    const originalIndex =
      socket.side === "input"
        ? graph.connections.findIndex(
            (edge) =>
              edge.to.node === socket.node && edge.to.port === socket.port,
          )
        : -1;
    const next: Pending =
      originalIndex >= 0
        ? {
            socket: {
              ...graph.connections[originalIndex].from,
              side: "output",
            },
            originalIndex,
          }
        : { socket };
    pendingRef.current = next;
    setPending(next);
    setPointer(socketPosition(socket));
    setHover(null);
    setEdgeIndex(null);
  }
  function move(event: PointerEvent) {
    const box = viewport.current;
    if (!box) return;
    const active = gesture.current;
    if (active && active.pointerId !== event.pointerId) return;
    if (active?.kind === "pan") {
      event.preventDefault();
      onView({
        ...view,
        pan: panAfterDrag(active.origin, active.start, {
          x: event.clientX,
          y: event.clientY,
        }),
      });
      return;
    }
    if (disabled) return;
    if (active?.kind === "node") {
      onView({
        ...view,
        positions: {
          ...positions,
          [active.key]: {
            x: Math.max(
              12,
              active.origin.x + (event.clientX - active.start.x) / view.zoom,
            ),
            y: Math.max(
              12,
              active.origin.y + (event.clientY - active.start.y) / view.zoom,
            ),
          },
        },
      });
    } else if (pendingRef.current && !wiringDisabled) {
      const rect = box.getBoundingClientRect();
      setPointer(
        screenToCanvas(
          {
            x: event.clientX - rect.left - box.clientLeft,
            y: event.clientY - rect.top - box.clientTop,
          },
          pan,
          view.zoom,
        ),
      );
      const target = targetAt(event.clientX, event.clientY);
      const pair = target && endpoints(pendingRef.current, target);
      if (!target || !pair) {
        setHover(null);
        return;
      }
      let valid = true;
      try {
        rewireGraphPorts(
          graph,
          nodes,
          pair.from,
          pair.to,
          pendingRef.current.originalIndex,
        );
      } catch {
        valid = false;
      }
      setHover({ key: socketKey(target), valid });
    }
  }
  function zoomTo(next: number) {
    const box = viewport.current;
    const anchor = {
      x: (box?.clientWidth ?? 600) / 2,
      y: (box?.clientHeight ?? 460) / 2,
    };
    clearConnection();
    onView({
      ...view,
      zoom: next,
      pan: zoomAround(pan, view.zoom, next, anchor),
    });
  }
  function selectEdge(index: number) {
    clearConnection();
    setEdgeIndex(index);
    viewport.current?.focus({ preventScroll: true });
  }
  function removeSelectedEdge() {
    if (edgeIndex === null || wiringDisabled) return;
    onDisconnect(edgeIndex);
    setEdgeIndex(null);
  }
  const pendingStart = pending && socketPosition(pending.socket);
  const previewPath =
    pendingStart && pointer
      ? pending?.socket.side === "output"
        ? wirePath(pendingStart, pointer)
        : wirePath(pointer, pendingStart)
      : "";
  function portButton(socket: Socket, type: string, required?: boolean) {
    const active = pending && socketKey(pending.socket) === socketKey(socket);
    const target =
      hover?.key === socketKey(socket)
        ? hover.valid
          ? " target-valid"
          : " target-invalid"
        : "";
    const connected = graph.connections.some((edge) =>
      socket.side === "input"
        ? edge.to.node === socket.node && edge.to.port === socket.port
        : edge.from.node === socket.node && edge.from.port === socket.port,
    );
    return (
      <button
        key={socket.port}
        className={`canvas-port ${socket.side}-port ${active ? "connecting" : ""}${target} ${connected ? "connected" : ""}`}
        style={{ "--socket-color": portColor(type) } as CSSProperties}
        disabled={wiringDisabled}
        data-port-node={socket.node}
        data-port-id={socket.port}
        data-port-side={socket.side}
        aria-label={`${socket.side === "input" ? "输入" : "输出"}端口 ${socket.node}.${socket.port} (${type})`}
        title={wiringReason || `${type}${required ? " · 必需" : ""}`}
        onPointerDown={(event) => {
          if (event.button !== 0 || wiringDisabled) return;
          event.preventDefault();
          viewport.current?.focus({ preventScroll: true });
          beginOrFinish(socket);
          if (pendingRef.current) {
            gesture.current = {
              kind: "wire",
              pointerId: event.pointerId,
              start: { x: event.clientX, y: event.clientY },
            };
            event.currentTarget.setPointerCapture(event.pointerId);
          }
        }}
        onClick={(event) => {
          if (event.detail === 0) beginOrFinish(socket);
        }}
      >
        {socket.side === "input" && <span className="port-dot" />}
        <span>
          {socket.port}
          {required ? " *" : ""}
        </span>
        {socket.side === "output" && <span className="port-dot" />}
      </button>
    );
  }
  return (
    <section
      className="graph-canvas"
      aria-label="节点画布"
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          clearConnection();
          setEdgeIndex(null);
        }
        if (
          (event.key === "Delete" || event.key === "Backspace") &&
          viewport.current?.contains(event.target as Node)
        ) {
          event.preventDefault();
          removeSelectedEdge();
        }
      }}
    >
      <div className="canvas-toolbar">
        <span className="muted">
          {graph.nodes.length} 节点 · {graph.connections.length} 连接
        </span>
        <div className="canvas-tools">
          {pending && <button onClick={clearConnection}>取消连线</button>}
          {edgeIndex !== null && (
            <button
              className="danger"
              disabled={wiringDisabled}
              title={wiringReason || undefined}
              onClick={removeSelectedEdge}
            >
              删除连线
            </button>
          )}
          <button
            disabled={disabled}
            onClick={() => {
              clearConnection();
              onView({
                ...view,
                positions: autoLayout(graph, nodes),
                pan: { x: 0, y: 0 },
                zoom: 1,
              });
            }}
          >
            整理布局
          </button>
          <button
            aria-label="缩小画布"
            disabled={view.zoom <= 0.5}
            onClick={() => zoomTo(Math.max(0.5, +(view.zoom - 0.1).toFixed(1)))}
          >
            −
          </button>
          <button
            aria-label="重置画布视图"
            onClick={() => {
              clearConnection();
              onView({ ...view, pan: { x: 0, y: 0 }, zoom: 1 });
            }}
          >
            {Math.round(view.zoom * 100)}%
          </button>
          <button
            aria-label="放大画布"
            disabled={view.zoom >= 1.5}
            onClick={() => zoomTo(Math.min(1.5, +(view.zoom + 0.1).toFixed(1)))}
          >
            +
          </button>
        </div>
      </div>
      <div
        className={`canvas-viewport ${panning ? "panning" : ""}`}
        ref={viewport}
        tabIndex={0}
        aria-label="画布，中键拖动平移"
        style={{
          backgroundPosition: `${pan.x}px ${pan.y}px`,
          backgroundSize: `${20 * view.zoom}px ${20 * view.zoom}px`,
        }}
        onAuxClick={(event) => {
          if (event.button === 1) event.preventDefault();
        }}
        onContextMenu={(event) => {
          if (pendingRef.current) {
            event.preventDefault();
            clearConnection();
          }
        }}
        onPointerDownCapture={(event) => {
          if (event.button !== 1) return;
          event.preventDefault();
          event.stopPropagation();
          clearConnection();
          event.currentTarget.focus({ preventScroll: true });
          gesture.current = {
            kind: "pan",
            pointerId: event.pointerId,
            start: { x: event.clientX, y: event.clientY },
            origin: pan,
          };
          setPanning(true);
          event.currentTarget.setPointerCapture(event.pointerId);
        }}
        onPointerDown={(event) => {
          if (
            event.button === 0 &&
            !(event.target as Element).closest(".canvas-node, .edge-hit")
          ) {
            clearConnection();
            setEdgeIndex(null);
            event.currentTarget.focus({ preventScroll: true });
          }
        }}
        onPointerMove={move}
        onPointerCancel={clearConnection}
        onLostPointerCapture={() => {
          gesture.current = null;
          setPanning(false);
        }}
        onPointerUp={(event) => {
          const active = gesture.current;
          if (!active || active.pointerId !== event.pointerId) return;
          if (
            active.kind === "wire" &&
            Math.hypot(
              event.clientX - active.start.x,
              event.clientY - active.start.y,
            ) > 4
          ) {
            const target = targetAt(event.clientX, event.clientY);
            if (
              target &&
              pendingRef.current &&
              endpoints(pendingRef.current, target)
            )
              finishConnection(target);
            else clearConnection();
          }
          gesture.current = null;
          setPanning(false);
          if (event.currentTarget.hasPointerCapture(event.pointerId))
            event.currentTarget.releasePointerCapture(event.pointerId);
        }}
      >
        <div
          className="canvas-surface"
          style={{
            width,
            height,
            transform: `translate(${pan.x}px, ${pan.y}px) scale(${view.zoom})`,
          }}
        >
          <svg
            className="canvas-edges"
            width={width}
            height={height}
            aria-label="节点连线"
          >
            {graph.connections.map((edge, index) => {
              const fromSocket: Socket = { ...edge.from, side: "output" },
                toSocket: Socket = { ...edge.to, side: "input" };
              const from = socketPosition(fromSocket),
                to = socketPosition(toSocket);
              if (!from || !to || pending?.originalIndex === index) return null;
              const path = wirePath(from, to);
              return (
                <g
                  key={index}
                  className={
                    edgeIndex === index ? "canvas-edge selected" : "canvas-edge"
                  }
                  style={
                    {
                      "--wire-color": portColor(socketType(fromSocket)),
                    } as CSSProperties
                  }
                >
                  <path className="edge-shadow" d={path} />
                  <path className="edge-line" d={path} />
                  <path
                    className="edge-hit"
                    d={path}
                    role="button"
                    tabIndex={0}
                    aria-label={`选择连线 ${edge.from.node}.${edge.from.port} → ${edge.to.node}.${edge.to.port}`}
                    onClick={() => selectEdge(index)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter" || event.key === " ") {
                        event.preventDefault();
                        selectEdge(index);
                      }
                    }}
                  />
                </g>
              );
            })}
            {previewPath && (
              <path
                className={`edge-preview ${hover ? (hover.valid ? "valid" : "invalid") : ""}`}
                d={previewPath}
              />
            )}
          </svg>
          {cards.map((card) => (
            <article
              key={card.node.id}
              className={`canvas-node ${view.selectedId === card.node.id ? "selected" : ""} ${!card.descriptor ? "unknown" : ""}`}
              style={{
                left: card.point.x,
                top: card.point.y,
                width: CARD_WIDTH,
                height: card.height,
              }}
            >
              <button
                className="canvas-node-header"
                disabled={disabled}
                aria-label={`选择节点 ${card.node.id}`}
                title="拖动调整位置"
                onClick={() => onView({ ...view, selectedId: card.node.id })}
                onPointerDown={(event) => {
                  if (event.button !== 0 || disabled) return;
                  event.preventDefault();
                  clearConnection();
                  setEdgeIndex(null);
                  gesture.current = {
                    kind: "node",
                    pointerId: event.pointerId,
                    key: positionKey(card.node),
                    start: { x: event.clientX, y: event.clientY },
                    origin: card.point,
                  };
                  onView({ ...view, selectedId: card.node.id });
                  event.currentTarget.setPointerCapture(event.pointerId);
                }}
              >
                <strong>
                  {card.descriptor?.displayName ?? card.node.type}
                </strong>
                <code>{card.node.id}</code>
              </button>
              <div className="canvas-ports">
                <div>
                  {card.inputs.map((port) =>
                    portButton(
                      { node: card.node.id, port: port.id, side: "input" },
                      port.type,
                      port.required,
                    ),
                  )}
                </div>
                <div>
                  {card.outputs.map((port) =>
                    portButton(
                      { node: card.node.id, port: port.id, side: "output" },
                      port.type,
                    ),
                  )}
                </div>
              </div>
            </article>
          ))}
          {!cards.length && (
            <div className="canvas-empty">从节点目录添加节点</div>
          )}
        </div>
      </div>
      <p className="canvas-help" role="status">
        {wiringReason ||
          (pending
            ? "选择目标端口 · Esc / 右键取消"
            : "中键平移 · 拖动端口连线 · 选中连线后 Delete 删除")}
      </p>
    </section>
  );
}
