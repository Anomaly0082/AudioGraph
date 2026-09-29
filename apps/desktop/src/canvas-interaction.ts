import type { Point } from "./graph-canvas-layout";

// Camera coordinates are screen pixels; graph coordinates are independent of zoom.
export function panAfterDrag(
  origin: Point,
  start: Point,
  current: Point,
): Point {
  return {
    x: origin.x + current.x - start.x,
    y: origin.y + current.y - start.y,
  };
}
export function screenToCanvas(point: Point, pan: Point, zoom: number): Point {
  return { x: (point.x - pan.x) / zoom, y: (point.y - pan.y) / zoom };
}
export function zoomAround(
  pan: Point,
  zoom: number,
  nextZoom: number,
  anchor: Point,
): Point {
  const fixed = screenToCanvas(anchor, pan, zoom);
  return { x: anchor.x - fixed.x * nextZoom, y: anchor.y - fixed.y * nextZoom };
}
export function wirePath(from: Point, to: Point): string {
  const bend = Math.max(
    65,
    Math.min(
      260,
      Math.abs(to.x - from.x) * 0.5 + Math.abs(to.y - from.y) * 0.12,
    ),
  );
  return `M ${from.x} ${from.y} C ${from.x + bend} ${from.y}, ${to.x - bend} ${to.y}, ${to.x} ${to.y}`;
}
export function portColor(type: string): string {
  switch (type.toLowerCase()) {
    case "audio":
    case "audiostream":
    case "audioblock":
      return "#e4c467";
    case "text":
      return "#8acdb1";
    case "filepath":
    case "file_path":
      return "#bba1dc";
    case "number":
      return "#a9bbd2";
    case "boolean":
      return "#dd98b7";
    default:
      return "#9fa9b7";
  }
}
