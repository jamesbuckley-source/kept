/**
 * The onscreen ruler as macOS draws it.
 *
 * Windows gets a Win32 popup painted with GDI. There is no such window on macOS,
 * so the same overlay is a transparent, always-on-top webview covering one display
 * and a canvas that draws the guides, distances, boxes and angles. The interaction
 * rules follow `ruler.rs` — drag to measure, Shift to constrain, click three points
 * for an angle, drag guides out of the edge rulers — and every string on screen is
 * asked of the backend, so units, DIP conversion and copy formats cannot drift
 * between the two ports.
 *
 * Coordinates are physical screen pixels throughout: the canvas backing store is
 * the display's pixel size, so a 200-pixel drag reads 200 px whatever the display's
 * scale factor is.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";

import { useSettings } from "../hooks/useSettings";
import {
  errorMessage,
  rulerClose,
  rulerCopyMeasurement,
  rulerLengthText,
  rulerMeasurementText,
  rulerSetLocked,
  rulerSetup,
  type RulerMeasurement,
  type RulerReadout,
  type RulerSetup,
} from "../lib/ipc";
import { applyTheme } from "../lib/theme";

type Point = { x: number; y: number };
type Shape = RulerMeasurement;
type Guide = { vertical: boolean; at: number };
type Tool = "select" | "distance" | "box" | "angle";

/** How strong a snap was; a point beats an axis line, a guide beats the grid. */
type SnapKind = "point" | "guide" | "grid";

type Drag =
  | { kind: "none" }
  | { kind: "new"; anchor: Point }
  | { kind: "move"; index: number; last: Point }
  | { kind: "handle"; index: number; handle: number }
  | { kind: "guide"; index: number }
  | { kind: "toolbar"; offset: Point };

type Colors = {
  panel: string;
  panel3: string;
  edge: string;
  ink: string;
  inkDim: string;
  inkFaint: string;
  accent: string;
  accentHi: string;
  ok: string;
  danger: string;
};

const SNAP_RANK: Record<SnapKind, number> = { point: 0, guide: 1, grid: 2 };

function clamp(value: number, low: number, high: number): number {
  return Math.min(Math.max(value, low), Math.max(low, high));
}

function point(a: Point): [number, number] {
  return [Math.round(a.x), Math.round(a.y)];
}

function anchor(shape: Shape): Point[] {
  switch (shape.kind) {
    case "distance":
      return [
        { x: shape.a[0], y: shape.a[1] },
        { x: shape.b[0], y: shape.b[1] },
      ];
    case "box":
      return [
        { x: shape.left, y: shape.top },
        { x: shape.right, y: shape.top },
        { x: shape.left, y: shape.bottom },
        { x: shape.right, y: shape.bottom },
      ];
    case "angle":
      return [
        { x: shape.vertex[0], y: shape.vertex[1] },
        { x: shape.a[0], y: shape.a[1] },
        { x: shape.b[0], y: shape.b[1] },
      ];
  }
}

function boxFromPoints(a: Point, b: Point): Shape {
  return {
    kind: "box",
    left: Math.round(Math.min(a.x, b.x)),
    top: Math.round(Math.min(a.y, b.y)),
    right: Math.round(Math.max(a.x, b.x)),
    bottom: Math.round(Math.max(a.y, b.y)),
  };
}

function angleFrom(vertex: Point, a: Point, b: Point): Shape {
  return { kind: "angle", vertex: point(vertex), a: point(a), b: point(b) };
}

/** Distance from a point to a segment, in physical pixels. */
function toSegment(at: Point, a: Point, b: Point): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const length = dx * dx + dy * dy;
  if (length === 0) return Math.hypot(at.x - a.x, at.y - a.y);
  const t = clamp(((at.x - a.x) * dx + (at.y - a.y) * dy) / length, 0, 1);
  return Math.hypot(at.x - (a.x + dx * t), at.y - (a.y + dy * t));
}

/** The eight corners and edge midpoints of a box, in `resized`'s handle order. */
function boxHandles(left: number, top: number, right: number, bottom: number): Point[] {
  const midX = left + (right - left) / 2;
  const midY = top + (bottom - top) / 2;
  return [
    { x: left, y: top },
    { x: midX, y: top },
    { x: right, y: top },
    { x: right, y: midY },
    { x: right, y: bottom },
    { x: midX, y: bottom },
    { x: left, y: bottom },
    { x: left, y: midY },
  ];
}

/** Projects a drag onto the nearest of the eight compass directions. */
function constrained(from: Point, to: Point): Point {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  if (Math.abs(dx) >= Math.abs(dy) * 2) return { x: to.x, y: from.y };
  if (Math.abs(dy) >= Math.abs(dx) * 2) return { x: from.x, y: to.y };
  const reach = Math.max(Math.abs(dx), Math.abs(dy));
  return {
    x: from.x + reach * (dx < 0 ? -1 : 1),
    y: from.y + reach * (dy < 0 ? -1 : 1),
  };
}

/** The corner that makes a drag from `from` a square. */
function squarePoint(from: Point, to: Point): Point {
  const dx = to.x - from.x;
  const dy = to.y - from.y;
  const reach = Math.max(Math.abs(dx), Math.abs(dy));
  return {
    x: from.x + reach * (dx < 0 ? -1 : 1),
    y: from.y + reach * (dy < 0 ? -1 : 1),
  };
}

/** Rounds the second ray to `step` degrees, keeping its length. */
function snapAngle(vertex: Point, a: Point, b: Point, step: number): Point {
  const length = Math.hypot(b.x - vertex.x, b.y - vertex.y);
  if (length === 0) return b;
  const base = Math.atan2(a.y - vertex.y, a.x - vertex.x);
  const current = Math.atan2(b.y - vertex.y, b.x - vertex.x);
  const delta = ((current - base) * 180) / Math.PI;
  const angle = base + ((Math.round(delta / step) * step) * Math.PI) / 180;
  return {
    x: vertex.x + Math.round(length * Math.cos(angle)),
    y: vertex.y + Math.round(length * Math.sin(angle)),
  };
}

/** Moves a shape by a delta, keeping every point on the display. */
function moved(shape: Shape, delta: Point, bounds: { width: number; height: number }): Shape {
  const move = (at: Point): Point => ({
    x: clamp(at.x + delta.x, 0, bounds.width),
    y: clamp(at.y + delta.y, 0, bounds.height),
  });
  switch (shape.kind) {
    case "distance":
      return { ...shape, a: point(move({ x: shape.a[0], y: shape.a[1] })), b: point(move({ x: shape.b[0], y: shape.b[1] })) };
    case "box": {
      const width = shape.right - shape.left;
      const height = shape.bottom - shape.top;
      const left = clamp(shape.left + delta.x, 0, bounds.width - width);
      const top = clamp(shape.top + delta.y, 0, bounds.height - height);
      return { ...shape, left, top, right: left + width, bottom: top + height };
    }
    case "angle":
      return {
        ...shape,
        vertex: point(move({ x: shape.vertex[0], y: shape.vertex[1] })),
        a: point(move({ x: shape.a[0], y: shape.a[1] })),
        b: point(move({ x: shape.b[0], y: shape.b[1] })),
      };
  }
}

/** A handle drag, mirroring `resized` in ruler.rs. */
function resized(shape: Shape, handle: number, at: Point, square: boolean): Shape {
  switch (shape.kind) {
    case "distance":
      return handle === 0 ? { ...shape, a: point(at) } : { ...shape, b: point(at) };
    case "box": {
      let { left, top, right, bottom } = shape;
      switch (handle) {
        case 0:
          left = at.x;
          top = at.y;
          break;
        case 1:
          top = at.y;
          break;
        case 2:
          right = at.x;
          top = at.y;
          break;
        case 3:
          right = at.x;
          break;
        case 4:
          right = at.x;
          bottom = at.y;
          break;
        case 5:
          bottom = at.y;
          break;
        case 6:
          left = at.x;
          bottom = at.y;
          break;
        default:
          left = at.x;
      }
      if (square) {
        const side = Math.max(Math.abs(right - left), Math.abs(bottom - top));
        if (handle === 0 || handle === 6 || handle === 7) left = right - side;
        else right = left + side;
        if (handle === 0 || handle === 1 || handle === 2) top = bottom - side;
        else bottom = top + side;
      }
      return {
        ...shape,
        left: Math.min(left, right),
        top: Math.min(top, bottom),
        right: Math.max(left, right),
        bottom: Math.max(top, bottom),
      };
    }
    case "angle": {
      if (handle === 0) return { ...shape, vertex: point(at) };
      if (handle === 1) return { ...shape, a: point(at) };
      return { ...shape, b: point(at) };
    }
  }
}

/** Whether `at` is close enough to a shape to grab it; `handle` when it is a knob. */
function hitShape(shape: Shape, at: Point, reach: number): number | null | undefined {
  const near = (target: Point) => Math.abs(at.x - target.x) <= reach && Math.abs(at.y - target.y) <= reach;
  switch (shape.kind) {
    case "distance": {
      const a = { x: shape.a[0], y: shape.a[1] };
      const b = { x: shape.b[0], y: shape.b[1] };
      if (near(a)) return 0;
      if (near(b)) return 1;
      return toSegment(at, a, b) <= reach ? null : undefined;
    }
    case "box": {
      const handles = boxHandles(shape.left, shape.top, shape.right, shape.bottom);
      const found = handles.findIndex(near);
      if (found >= 0) return found;
      const inside =
        at.x >= shape.left && at.x <= shape.right && at.y >= shape.top && at.y <= shape.bottom;
      return inside ? null : undefined;
    }
    case "angle": {
      const points = [
        { x: shape.vertex[0], y: shape.vertex[1] },
        { x: shape.a[0], y: shape.a[1] },
        { x: shape.b[0], y: shape.b[1] },
      ];
      const found = points.findIndex(near);
      if (found >= 0) return found;
      const vertex = points[0];
      const onRay =
        toSegment(at, vertex, points[1]) <= reach || toSegment(at, vertex, points[2]) <= reach;
      return onRay ? null : undefined;
    }
  }
}

/** Reads the palette the rest of the app paints with. */
function readColors(): Colors {
  const style = getComputedStyle(document.documentElement);
  const read = (name: string, fallback: string) =>
    style.getPropertyValue(name).trim() || fallback;
  return {
    panel: read("--color-panel", "#12161f"),
    panel3: read("--color-panel-3", "#1f2533"),
    edge: read("--color-edge", "#252c3b"),
    ink: read("--color-ink", "#e7eaf0"),
    inkDim: read("--color-ink-dim", "#99a3b5"),
    inkFaint: read("--color-ink-faint", "#6a7488"),
    accent: read("--color-accent", "#6d8cff"),
    accentHi: read("--color-accent-hi", "#8aa2ff"),
    ok: read("--color-ok", "#4ade80"),
    danger: read("--color-danger", "#ff6b6b"),
  };
}

function withAlpha(color: string, alpha: number): string {
  const hex = color.trim();
  if (hex.startsWith("#") && (hex.length === 7 || hex.length === 4)) {
    const digits =
      hex.length === 4
        ? hex
            .slice(1)
            .split("")
            .map((digit) => digit + digit)
            .join("")
        : hex.slice(1);
    const value = Number.parseInt(digits, 16);
    if (!Number.isNaN(value)) {
      const r = (value >> 16) & 0xff;
      const g = (value >> 8) & 0xff;
      const b = value & 0xff;
      return `rgba(${r}, ${g}, ${b}, ${alpha})`;
    }
    return hex;
  }
  const rgb = hex.match(/rgba?\(([^)]+)\)/);
  if (rgb) {
    const [r, g, b] = rgb[1].split(",").map((part) => part.trim());
    return `rgba(${r}, ${g}, ${b}, ${alpha})`;
  }
  return hex;
}

function roundedRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  height: number,
  radius: number,
) {
  const r = Math.min(radius, width / 2, height / 2);
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + width, y, x + width, y + height, r);
  ctx.arcTo(x + width, y + height, x, y + height, r);
  ctx.arcTo(x, y + height, x, y, r);
  ctx.arcTo(x, y, x + width, y, r);
  ctx.closePath();
}

/** A two-line readout badge, drawn inside the display wherever it fits. */
function drawBadge(
  ctx: CanvasRenderingContext2D,
  at: Point,
  primary: string,
  secondary: string | null,
  colors: Colors,
  scale: number,
) {
  const font = (size: number, weight: number) =>
    `${weight} ${Math.round(size * scale)}px -apple-system, "SF Pro Text", system-ui, sans-serif`;
  const pad = Math.round(6 * scale);
  const lineGap = Math.round(2 * scale);
  ctx.font = font(12, 600);
  const primaryWidth = ctx.measureText(primary).width;
  ctx.font = font(10, 400);
  const secondaryWidth = secondary ? ctx.measureText(secondary).width : 0;
  const width = Math.max(primaryWidth, secondaryWidth) + pad * 2;
  const primaryHeight = Math.round(15 * scale);
  const secondaryHeight = secondary ? Math.round(13 * scale) + lineGap : 0;
  const height = primaryHeight + secondaryHeight + pad * 2;

  const x = clamp(at.x - width / 2, Math.round(2 * scale), ctx.canvas.width - width - 2);
  const y = clamp(at.y - height, Math.round(2 * scale), ctx.canvas.height - height - 2);
  ctx.fillStyle = withAlpha(colors.panel, 0.94);
  ctx.strokeStyle = withAlpha(colors.edge, 0.95);
  ctx.lineWidth = Math.max(1, scale);
  roundedRect(ctx, x, y, width, height, Math.round(5 * scale));
  ctx.fill();
  ctx.stroke();

  ctx.textBaseline = "top";
  ctx.textAlign = "left";
  ctx.fillStyle = colors.ink;
  ctx.font = font(12, 600);
  ctx.fillText(primary, x + pad, y + pad);
  if (secondary) {
    ctx.fillStyle = colors.inkDim;
    ctx.font = font(10, 400);
    ctx.fillText(secondary, x + pad, y + pad + primaryHeight + lineGap);
  }
  return { x, y, width, height };
}

function drawHandles(
  ctx: CanvasRenderingContext2D,
  points: Point[],
  colors: Colors,
  scale: number,
  active: boolean,
) {
  const side = Math.round((active ? 7 : 5) * scale);
  const half = side / 2;
  ctx.lineWidth = Math.max(1, scale);
  for (const at of points) {
    ctx.fillStyle = active ? colors.panel : withAlpha(colors.panel3, 0.9);
    ctx.strokeStyle = active ? colors.accent : colors.inkDim;
    ctx.fillRect(at.x - half, at.y - half, side, side);
    ctx.strokeRect(at.x - half, at.y - half, side, side);
  }
}

function drawShape(
  ctx: CanvasRenderingContext2D,
  shape: Shape,
  colors: Colors,
  scale: number,
  active: boolean,
) {
  const line = Math.max(1, (active ? 2 : 1.3) * scale);
  const color = active ? colors.accent : withAlpha(colors.inkDim, 0.85);
  ctx.strokeStyle = color;
  ctx.lineWidth = line;
  ctx.lineCap = "butt";

  switch (shape.kind) {
    case "distance": {
      const a = { x: shape.a[0], y: shape.a[1] };
      const b = { x: shape.b[0], y: shape.b[1] };
      ctx.beginPath();
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(b.x, b.y);
      ctx.stroke();
      // End caps, so a short drag still reads as a measurement.
      const cap = Math.round(6 * scale);
      const dx = b.x - a.x;
      const dy = b.y - a.y;
      const length = Math.hypot(dx, dy) || 1;
      const nx = (-dy / length) * cap;
      const ny = (dx / length) * cap;
      ctx.beginPath();
      ctx.moveTo(a.x - nx, a.y - ny);
      ctx.lineTo(a.x + nx, a.y + ny);
      ctx.moveTo(b.x - nx, b.y - ny);
      ctx.lineTo(b.x + nx, b.y + ny);
      ctx.stroke();
      drawHandles(ctx, [a, b], colors, scale, active);
      break;
    }
    case "box": {
      ctx.strokeRect(
        shape.left,
        shape.top,
        shape.right - shape.left,
        shape.bottom - shape.top,
      );
      drawHandles(
        ctx,
        boxHandles(shape.left, shape.top, shape.right, shape.bottom),
        colors,
        scale,
        active,
      );
      break;
    }
    case "angle": {
      const vertex = { x: shape.vertex[0], y: shape.vertex[1] };
      const a = { x: shape.a[0], y: shape.a[1] };
      const b = { x: shape.b[0], y: shape.b[1] };
      ctx.beginPath();
      ctx.moveTo(vertex.x, vertex.y);
      ctx.lineTo(a.x, a.y);
      ctx.moveTo(vertex.x, vertex.y);
      ctx.lineTo(b.x, b.y);
      ctx.stroke();
      // Arc between the rays, drawn at a radius that stays visible on short rays.
      const radius = Math.min(
        Math.round(36 * scale),
        Math.hypot(a.x - vertex.x, a.y - vertex.y) * 0.5,
        Math.hypot(b.x - vertex.x, b.y - vertex.y) * 0.5,
      );
      if (radius > 2) {
        const start = Math.atan2(a.y - vertex.y, a.x - vertex.x);
        const end = Math.atan2(b.y - vertex.y, b.x - vertex.x);
        ctx.beginPath();
        ctx.arc(vertex.x, vertex.y, radius, start, end, false);
        ctx.stroke();
      }
      drawHandles(ctx, [vertex, a, b], colors, scale, active);
      break;
    }
  }
}

/** The edge strips, with numbered ticks that say where a guide will land. */
function drawEdgeRulers(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  strip: number,
  tickStep: number,
  colors: Colors,
  scale: number,
) {
  const font = `400 ${Math.round(9 * scale)}px -apple-system, system-ui, sans-serif`;
  const majorTick = Math.round(8 * scale);
  const minorTick = Math.round(4 * scale);
  const minorStep = Math.max(1, Math.round(tickStep / 10));

  ctx.fillStyle = withAlpha(colors.panel, 0.78);
  ctx.fillRect(0, 0, width, strip);
  ctx.fillRect(0, 0, strip, height);
  ctx.fillRect(0, height - strip, width, strip);
  ctx.fillRect(width - strip, 0, strip, height);

  ctx.strokeStyle = withAlpha(colors.edge, 0.9);
  ctx.lineWidth = Math.max(1, scale);
  ctx.beginPath();
  ctx.moveTo(0, strip);
  ctx.lineTo(width, strip);
  ctx.moveTo(strip, 0);
  ctx.lineTo(strip, height);
  ctx.moveTo(0, height - strip);
  ctx.lineTo(width, height - strip);
  ctx.moveTo(width - strip, 0);
  ctx.lineTo(width - strip, height);
  ctx.stroke();

  ctx.strokeStyle = withAlpha(colors.inkFaint, 0.85);
  ctx.fillStyle = colors.inkDim;
  ctx.font = font;
  ctx.textBaseline = "top";
  ctx.textAlign = "left";

  const ticks = (limit: number) => {
    const values: { at: number; major: boolean }[] = [];
    for (let at = 0; at < limit; at += minorStep) {
      values.push({ at, major: at % tickStep === 0 });
    }
    return values;
  };

  for (const { at, major } of ticks(width)) {
    const length = major ? majorTick : minorTick;
    ctx.beginPath();
    ctx.moveTo(at, 0);
    ctx.lineTo(at, length);
    ctx.moveTo(at, height - length);
    ctx.lineTo(at, height);
    ctx.stroke();
    if (major) {
      ctx.fillText(String(at), at + Math.round(2 * scale), strip - Math.round(11 * scale));
      ctx.fillText(String(at), at + Math.round(2 * scale), height - strip + Math.round(1 * scale));
    }
  }
  for (const { at, major } of ticks(height)) {
    const length = major ? majorTick : minorTick;
    ctx.beginPath();
    ctx.moveTo(0, at);
    ctx.lineTo(length, at);
    ctx.moveTo(width - length, at);
    ctx.lineTo(width, at);
    ctx.stroke();
    if (major) {
      ctx.fillText(String(at), Math.round(1 * scale), at + Math.round(2 * scale));
      ctx.fillText(String(at), width - strip + Math.round(1 * scale), at + Math.round(2 * scale));
    }
  }
}

export function RulerOverlay() {
  const { settings, patch } = useSettings();
  const [setup, setSetup] = useState<RulerSetup | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [tool, setTool] = useState<Tool>("distance");
  const [shapes, setShapes] = useState<Shape[]>([]);
  const [active, setActive] = useState<number | null>(null);
  const [guides, setGuides] = useState<Guide[]>([]);
  const [selectedGuide, setSelectedGuide] = useState<number | null>(null);
  const [pending, setPending] = useState<Shape | null>(null);
  const [anglePoints, setAnglePoints] = useState<Point[]>([]);
  const [pointer, setPointer] = useState<Point>({ x: 0, y: 0 });
  const [snapMark, setSnapMark] = useState<{ at: Point; kind: SnapKind } | null>(null);
  const [locked, setLocked] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const [readout, setReadout] = useState<RulerReadout | null>(null);
  const [guideLabel, setGuideLabel] = useState<string | null>(null);
  const [toolbarAt, setToolbarAt] = useState<Point | null>(null);

  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const toolbarRef = useRef<HTMLDivElement | null>(null);
  const dragRef = useRef<Drag>({ kind: "none" });
  const restoredToolbar = useRef(false);

  const units = settings?.rulerUnits ?? "physical";
  const copyFormat = settings?.rulerCopyFormat ?? "px";
  const snap = settings?.rulerSnap ?? true;
  const snapGrid = settings?.rulerSnapGrid ?? 0;
  const snapThreshold = settings?.rulerSnapThreshold ?? 8;
  const edgeRulers = settings?.rulerEdgeRulers ?? true;
  const guidesEnabled = settings?.rulerGuides ?? true;

  const scale = setup?.scale ?? 1;
  const bounds = useMemo(
    () => ({ width: setup?.width ?? 0, height: setup?.height ?? 0 }),
    [setup],
  );
  const colors = useMemo(() => readColors(), [settings?.theme]);

  // ---- lifecycle ---------------------------------------------------------

  useEffect(() => {
    let cancelled = false;
    rulerSetup()
      .then((loaded) => {
        if (!cancelled) setSetup(loaded);
      })
      .catch((e) => setError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    if (settings?.theme) applyTheme(settings.theme);
  }, [settings?.theme]);

  // The app paints its canvas colour behind every window; this one has to show
  // the desktop instead.
  useEffect(() => {
    const { body, documentElement } = document;
    const previous = [body.style.background, documentElement.style.background];
    body.style.background = "transparent";
    documentElement.style.background = "transparent";
    return () => {
      body.style.background = previous[0];
      documentElement.style.background = previous[1];
    };
  }, []);

  // Esc while locked belongs to the backend (it is a global shortcut by then),
  // and unlocking is reported back here so the padlock redraws.
  useEffect(() => {
    let cancelled = false;
    let stop: (() => void) | undefined;
    listen("ruler:unlocked", () => {
      if (!cancelled) setLocked(false);
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
      stop?.();
    };
  }, []);

  // The stored toolbar position is in screen pixels; the overlay works in
  // display-local pixels, so the display's origin is taken off once.
  useEffect(() => {
    if (restoredToolbar.current || !setup || !settings) return;
    restoredToolbar.current = true;
    const stored = settings.rulerToolbarPos;
    if (!stored) return;
    setToolbarAt({
      x: clamp((stored.x - setup.origin[0]) / setup.scale, 0, window.innerWidth - 120),
      y: clamp((stored.y - setup.origin[1]) / setup.scale, 0, window.innerHeight - 40),
    });
  }, [setup, settings]);

  const close = useCallback(() => {
    void rulerClose().catch(() => undefined);
  }, []);

  // ---- snapping ----------------------------------------------------------

  const nearestOnAxis = useCallback(
    (value: number, axis: "x" | "y"): { value: number; kind: SnapKind } | null => {
      if (!setup) return null;
      let bestValue = 0;
      let bestKind: SnapKind | null = null;
      let bestGap = Number.POSITIVE_INFINITY;
      const consider = (candidate: number, kind: SnapKind) => {
        const gap = Math.abs(candidate - value);
        if (gap > snapThreshold) return;
        const stronger = bestKind === null || SNAP_RANK[kind] < SNAP_RANK[bestKind];
        if (gap < bestGap || (gap === bestGap && stronger)) {
          bestValue = candidate;
          bestKind = kind;
          bestGap = gap;
        }
      };
      for (const guide of guides) {
        if (axis === "x" && guide.vertical) consider(guide.at, "guide");
        if (axis === "y" && !guide.vertical) consider(guide.at, "guide");
      }
      if (snapGrid > 0) {
        consider(Math.round(value / snapGrid) * snapGrid, "grid");
      }
      return bestKind ? { value: bestValue, kind: bestKind } : null;
    },
    [setup, guides, snapGrid, snapThreshold],
  );

  const snapPoint = useCallback(
    (at: Point, bypass: boolean): { at: Point; kind: SnapKind | null } => {
      if (!snap || bypass || !setup) {
        return { at, kind: null };
      }
      // A corner of an existing measurement pins both coordinates at once, so it
      // wins over an axis line just as it does in the Windows ruler.
      let best: Point | null = null;
      let bestGap = Number.POSITIVE_INFINITY;
      for (const shape of shapes) {
        for (const candidate of anchor(shape)) {
          const gap = Math.max(Math.abs(candidate.x - at.x), Math.abs(candidate.y - at.y));
          if (gap <= snapThreshold && gap < bestGap) {
            best = candidate;
            bestGap = gap;
          }
        }
      }
      if (best) return { at: { ...best }, kind: "point" };

      const out = { ...at };
      let kind: SnapKind | null = null;
      const onX = nearestOnAxis(at.x, "x");
      if (onX) {
        out.x = onX.value;
        kind = onX.kind;
      }
      const onY = nearestOnAxis(at.y, "y");
      if (onY) {
        out.y = onY.value;
        kind = kind
          ? SNAP_RANK[onY.kind] < SNAP_RANK[kind]
            ? onY.kind
            : kind
          : onY.kind;
      }
      out.x = clamp(out.x, 0, setup.width - 1);
      out.y = clamp(out.y, 0, setup.height - 1);
      return { at: out, kind };
    },
    [snap, setup, shapes, snapThreshold, nearestOnAxis],
  );

  // ---- derived readout ---------------------------------------------------

  const shown = useMemo<Shape | null>(() => {
    if (pending) return pending;
    if (anglePoints.length === 2) return angleFrom(anglePoints[0], anglePoints[1], pointer);
    if (active !== null && shapes[active]) return shapes[active];
    return null;
  }, [pending, anglePoints, pointer, active, shapes]);

  const requestRef = useRef<{ shape: Shape; units: string; format: string; dpi: number } | null>(
    null,
  );
  const busyRef = useRef(false);

  // A drag changes the shape on every pointer move. One request is kept in
  // flight and the newest shape replaces whatever was queued, so the text keeps
  // up without a queue of stale round trips.
  useEffect(() => {
    if (!setup || !shown) {
      requestRef.current = null;
      setReadout(null);
      return;
    }
    requestRef.current = { shape: shown, units, format: copyFormat, dpi: setup.dpi };
    if (busyRef.current) return;
    busyRef.current = true;
    void (async () => {
      try {
        while (requestRef.current) {
          const request = requestRef.current;
          requestRef.current = null;
          const text = await rulerMeasurementText(
            request.shape,
            request.units,
            request.format,
            request.dpi,
          );
          if (!requestRef.current) setReadout(text);
        }
      } catch {
        // A missing label is not worth losing the overlay over.
      } finally {
        busyRef.current = false;
      }
    })();
  }, [shown, units, copyFormat, setup]);

  useEffect(() => {
    const guide = selectedGuide !== null ? guides[selectedGuide] : undefined;
    if (!setup || !guide) {
      setGuideLabel(null);
      return;
    }
    let cancelled = false;
    rulerLengthText(guide.at, units, setup.dpi)
      .then((text) => {
        if (!cancelled) setGuideLabel(text);
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [setup, selectedGuide, guides, units]);

  // ---- canvas size and drawing -------------------------------------------

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !setup) return;
    canvas.width = setup.width;
    canvas.height = setup.height;
  }, [setup]);

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx || !setup) return;

    ctx.clearRect(0, 0, canvas.width, canvas.height);
    const strip = Math.round(22 * scale);
    if (edgeRulers) {
      drawEdgeRulers(ctx, canvas.width, canvas.height, strip, setup.tickStep, colors, scale);
    }

    for (const [index, guide] of guides.entries()) {
      const selected = index === selectedGuide;
      ctx.strokeStyle = withAlpha(colors.accent, selected ? 1 : 0.55);
      ctx.lineWidth = Math.max(1, (selected ? 2 : 1) * scale);
      ctx.beginPath();
      if (guide.vertical) {
        ctx.moveTo(guide.at, 0);
        ctx.lineTo(guide.at, canvas.height);
      } else {
        ctx.moveTo(0, guide.at);
        ctx.lineTo(canvas.width, guide.at);
      }
      ctx.stroke();
    }

    shapes.forEach((shape, index) => {
      if (index === active) return;
      drawShape(ctx, shape, colors, scale, false);
    });
    if (active !== null && shapes[active]) {
      drawShape(ctx, shapes[active], colors, scale, true);
    }
    if (pending) {
      drawShape(ctx, pending, colors, scale, true);
    }
    if (anglePoints.length > 0) {
      const vertex = anglePoints[0];
      const a = anglePoints[1] ?? pointer;
      ctx.strokeStyle = colors.accent;
      ctx.lineWidth = Math.max(1, 1.4 * scale);
      ctx.beginPath();
      ctx.moveTo(vertex.x, vertex.y);
      ctx.lineTo(a.x, a.y);
      ctx.moveTo(vertex.x, vertex.y);
      ctx.lineTo(pointer.x, pointer.y);
      ctx.stroke();
      drawHandles(ctx, [vertex], colors, scale, true);
    }

    if (!locked) {
      ctx.strokeStyle = withAlpha(colors.ink, 0.16);
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.moveTo(0, pointer.y + 0.5);
      ctx.lineTo(canvas.width, pointer.y + 0.5);
      ctx.moveTo(pointer.x + 0.5, 0);
      ctx.lineTo(pointer.x + 0.5, canvas.height);
      ctx.stroke();
    }
    if (snapMark) {
      ctx.strokeStyle = colors.ok;
      ctx.lineWidth = Math.max(1, 1.4 * scale);
      ctx.beginPath();
      ctx.arc(snapMark.at.x, snapMark.at.y, Math.round(7 * scale), 0, Math.PI * 2);
      ctx.stroke();
    }

    // The live readout sits above the shape it describes.
    if (shown && readout) {
      const primary = readout.primary;
      if (shown.kind === "distance") {
        const a = { x: shown.a[0], y: shown.a[1] };
        const b = { x: shown.b[0], y: shown.b[1] };
        drawBadge(
          ctx,
          { x: (a.x + b.x) / 2, y: Math.min(a.y, b.y) - Math.round(10 * scale) },
          primary,
          readout.secondary,
          colors,
          scale,
        );
      } else if (shown.kind === "box") {
        drawBadge(
          ctx,
          { x: shown.left + (shown.right - shown.left) / 2, y: shown.top - Math.round(6 * scale) },
          primary,
          readout.secondary,
          colors,
          scale,
        );
      } else {
        drawBadge(
          ctx,
          {
            x: shown.vertex[0] + Math.round(48 * scale),
            y: shown.vertex[1] + Math.round(18 * scale),
          },
          primary,
          readout.secondary,
          colors,
          scale,
        );
      }
    }

    const guide = selectedGuide !== null ? guides[selectedGuide] : undefined;
    if (guide && guideLabel) {
      drawBadge(
        ctx,
        guide.vertical
          ? { x: guide.at + Math.round(26 * scale), y: Math.round(70 * scale) }
          : { x: Math.round(120 * scale), y: guide.at - Math.round(6 * scale) },
        guideLabel,
        null,
        colors,
        scale,
      );
    }
  }, [
    setup,
    scale,
    colors,
    edgeRulers,
    guides,
    selectedGuide,
    shapes,
    active,
    pending,
    anglePoints,
    pointer,
    locked,
    snapMark,
    shown,
    readout,
    guideLabel,
  ]);

  // ---- hit testing and pointer input -------------------------------------

  const reach = Math.round(7 * scale);
  const strip = Math.round(22 * scale);

  const toCanvas = useCallback((event: React.PointerEvent): Point => {
    const canvas = canvasRef.current;
    if (!canvas) return { x: 0, y: 0 };
    const rect = canvas.getBoundingClientRect();
    return {
      x: Math.round(((event.clientX - rect.left) * canvas.width) / Math.max(1, rect.width)),
      y: Math.round(((event.clientY - rect.top) * canvas.height) / Math.max(1, rect.height)),
    };
  }, []);

  const hitAt = useCallback(
    (at: Point) => {
      for (let index = shapes.length - 1; index >= 0; index -= 1) {
        const handle = hitShape(shapes[index], at, reach);
        if (handle !== undefined) return { index, handle };
      }
      return null;
    },
    [shapes, reach],
  );

  const guideAt = useCallback(
    (at: Point) => {
      for (let index = guides.length - 1; index >= 0; index -= 1) {
        const guide = guides[index];
        const gap = guide.vertical ? Math.abs(at.x - guide.at) : Math.abs(at.y - guide.at);
        if (gap <= Math.max(4, reach / 2)) return index;
      }
      return null;
    },
    [guides, reach],
  );

  const inEdge = useCallback(
    (at: Point) =>
      at.x < strip || at.y < strip || at.x > bounds.width - strip || at.y > bounds.height - strip,
    [strip, bounds],
  );

  const chooseTool = useCallback((next: Tool) => {
    setTool(next);
    setAnglePoints([]);
    setPending(null);
    dragRef.current = { kind: "none" };
  }, []);

  const cycleActive = useCallback(
    (delta: number) => {
      if (shapes.length === 0) return;
      setActive((current) => {
        const index = current === null ? 0 : (current + delta + shapes.length) % shapes.length;
        return index;
      });
      setSelectedGuide(null);
    },
    [shapes.length],
  );

  const removeSelection = useCallback(() => {
    if (selectedGuide !== null) {
      const index = selectedGuide;
      setGuides((current) => current.filter((_, at) => at !== index));
      setSelectedGuide(null);
      return;
    }
    if (active !== null) {
      const index = active;
      setShapes((current) => current.filter((_, at) => at !== index));
      setActive(null);
    }
  }, [selectedGuide, active]);

  const nudge = useCallback(
    (delta: Point) => {
      if (selectedGuide !== null) {
        setGuides((current) =>
          current.map((guide, index) =>
            index === selectedGuide
              ? {
                  vertical: guide.vertical,
                  at: clamp(
                    guide.at + (guide.vertical ? delta.x : delta.y),
                    0,
                    guide.vertical ? bounds.width : bounds.height,
                  ),
                }
              : guide,
          ),
        );
        return;
      }
      if (active !== null) {
        setShapes((current) =>
          current.map((shape, index) =>
            index === active ? moved(shape, delta, bounds) : shape,
          ),
        );
      }
    },
    [selectedGuide, active, bounds],
  );

  const copyActive = useCallback(() => {
    const shape = (active !== null ? shapes[active] : null) ?? pending ?? shown;
    if (!shape || !setup) return;
    rulerCopyMeasurement(shape, units, copyFormat, setup.dpi)
      .then((text) => {
        setCopied(text);
        window.setTimeout(() => setCopied(null), 1_400);
      })
      .catch((e) => setError(errorMessage(e)));
  }, [active, shapes, pending, shown, setup, units, copyFormat]);

  const toggleLock = useCallback(() => {
    const next = !locked;
    rulerSetLocked(next)
      .then(() => setLocked(next))
      .catch((e) => setError(errorMessage(e)));
  }, [locked]);

  const commitPending = useCallback(() => {
    if (!pending) return;
    if (pending.kind === "distance") {
      const span = Math.abs(pending.a[0] - pending.b[0]) + Math.abs(pending.a[1] - pending.b[1]);
      if (span <= 2) {
        setPending(null);
        return;
      }
    }
    if (pending.kind === "box" && (pending.right - pending.left <= 1 || pending.bottom - pending.top <= 1)) {
      setPending(null);
      return;
    }
    setShapes((current) => [...current, pending]);
    setActive(shapes.length);
    setPending(null);
  }, [pending, shapes.length]);

  const onPointerDown = (event: React.PointerEvent<HTMLCanvasElement>) => {
    if (locked) return;
    if (event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    const at = toCanvas(event);
    setPointer(at);

    if (guidesEnabled && edgeRulers && inEdge(at)) {
      const horizontal = at.y < strip || at.y > bounds.height - strip;
      const guide: Guide = horizontal ? { vertical: false, at: at.y } : { vertical: true, at: at.x };
      setGuides((current) => [...current, guide]);
      setSelectedGuide(guides.length);
      setActive(null);
      dragRef.current = { kind: "guide", index: guides.length };
      return;
    }

    const hit = hitAt(at);
    if (hit) {
      setActive(hit.index);
      setSelectedGuide(null);
      dragRef.current =
        hit.handle === null
          ? { kind: "move", index: hit.index, last: at }
          : { kind: "handle", index: hit.index, handle: hit.handle };
      return;
    }

    const guideIndex = guideAt(at);
    if (guideIndex !== null) {
      setSelectedGuide(guideIndex);
      setActive(null);
      dragRef.current = { kind: "guide", index: guideIndex };
      return;
    }

    setActive(null);
    setSelectedGuide(null);

    if (tool === "select") return;

    const snapped = snapPoint(at, event.altKey);
    setSnapMark(snapped.kind ? { at: snapped.at, kind: snapped.kind } : null);

    if (tool === "distance" || tool === "box") {
      dragRef.current = { kind: "new", anchor: snapped.at };
      setPending(
        tool === "distance"
          ? { kind: "distance", a: point(snapped.at), b: point(snapped.at) }
          : boxFromPoints(snapped.at, snapped.at),
      );
      return;
    }

    // Angle: three clicks — vertex, first ray, second ray.
    if (anglePoints.length >= 2) {
      const vertex = anglePoints[0];
      const first = anglePoints[1];
      const second = event.shiftKey ? snapAngle(vertex, first, snapped.at, 15) : snapped.at;
      setShapes((current) => [...current, angleFrom(vertex, first, second)]);
      setActive(shapes.length);
      setAnglePoints([]);
    } else {
      setAnglePoints((current) => [...current, snapped.at]);
    }
  };

  const onPointerMove = (event: React.PointerEvent<HTMLCanvasElement>) => {
    const at = toCanvas(event);
    setPointer(at);
    const drag = dragRef.current;

    if (drag.kind === "new") {
      const snapped = snapPoint(at, event.altKey);
      setSnapMark(snapped.kind ? { at: snapped.at, kind: snapped.kind } : null);
      if (tool === "distance") {
        const end = event.shiftKey ? constrained(drag.anchor, snapped.at) : snapped.at;
        setPending({ kind: "distance", a: point(drag.anchor), b: point(end) });
      } else {
        const end = event.shiftKey ? squarePoint(drag.anchor, snapped.at) : snapped.at;
        setPending(boxFromPoints(drag.anchor, end));
      }
    } else if (drag.kind === "move") {
      const delta = { x: at.x - drag.last.x, y: at.y - drag.last.y };
      if (delta.x !== 0 || delta.y !== 0) {
        setShapes((current) =>
          current.map((shape, index) => (index === drag.index ? moved(shape, delta, bounds) : shape)),
        );
      }
      dragRef.current = { kind: "move", index: drag.index, last: at };
    } else if (drag.kind === "handle") {
      const snapped = snapPoint(at, event.altKey);
      setSnapMark(snapped.kind ? { at: snapped.at, kind: snapped.kind } : null);
      setShapes((current) =>
        current.map((shape, index) =>
          index === drag.index ? resized(shape, drag.handle, snapped.at, event.shiftKey) : shape,
        ),
      );
    } else if (drag.kind === "guide") {
      setGuides((current) =>
        current.map((guide, index) => {
          if (index !== drag.index) return guide;
          return {
            vertical: guide.vertical,
            at: clamp(
              guide.vertical ? at.x : at.y,
              0,
              guide.vertical ? bounds.width : bounds.height,
            ),
          };
        }),
      );
    }

    // Cursor feedback: knobs resize, bodies move, empty space measures.
    const canvas = event.currentTarget;
    if (drag.kind !== "none") {
      canvas.style.cursor = drag.kind === "move" ? "move" : "crosshair";
      return;
    }
    const hit = hitAt(at);
    if (hit && hit.handle !== null) {
      const shape = shapes[hit.index];
      if (shape?.kind === "box") {
        canvas.style.cursor =
          hit.handle === 1 || hit.handle === 5
            ? "ns-resize"
            : hit.handle === 3 || hit.handle === 7
              ? "ew-resize"
              : "nwse-resize";
      } else {
        canvas.style.cursor = "move";
      }
    } else if (hit) {
      canvas.style.cursor = "move";
    } else if (guideAt(at) !== null) {
      canvas.style.cursor = "move";
    } else {
      canvas.style.cursor = "crosshair";
    }
  };

  const onPointerUp = () => {
    const drag = dragRef.current;
    dragRef.current = { kind: "none" };
    setSnapMark(null);
    if (drag.kind === "new") {
      commitPending();
    } else if (drag.kind === "guide") {
      // Dropped back into a strip means "put it away", as on Windows.
      const index = drag.index;
      const guide = guides[index];
      if (guide) {
        const inside = guide.vertical
          ? guide.at < strip || guide.at > bounds.width - strip
          : guide.at < strip || guide.at > bounds.height - strip;
        if (inside) {
          setGuides((current) => current.filter((_, at) => at !== index));
          setSelectedGuide(null);
        }
      }
    }
  };

  // ---- keyboard ----------------------------------------------------------

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        // Locked, the backend owns Esc and turns it into an unlock.
        if (!locked) close();
        return;
      }
      if (locked || event.metaKey || event.ctrlKey || event.altKey) return;
      const key = event.key.toLowerCase();
      if (key === "backspace" || key === "delete") {
        event.preventDefault();
        removeSelection();
        return;
      }
      if (key === "tab") {
        event.preventDefault();
        cycleActive(event.shiftKey ? -1 : 1);
        return;
      }
      if (key.startsWith("arrow")) {
        event.preventDefault();
        const step = event.shiftKey ? 10 : 1;
        const delta =
          key === "arrowleft"
            ? { x: -step, y: 0 }
            : key === "arrowright"
              ? { x: step, y: 0 }
              : key === "arrowup"
                ? { x: 0, y: -step }
                : { x: 0, y: step };
        nudge(delta);
        return;
      }
      if (key === "v") chooseTool("select");
      else if (key === "d") chooseTool("distance");
      else if (key === "b") chooseTool("box");
      else if (key === "a") chooseTool("angle");
      else if (key === "g") void patch({ rulerGuides: !guidesEnabled }).catch(() => undefined);
      else if (key === "s") void patch({ rulerSnap: !snap }).catch(() => undefined);
      else if (key === "l") toggleLock();
      else if (key === "c") copyActive();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [
    locked,
    close,
    removeSelection,
    cycleActive,
    nudge,
    chooseTool,
    patch,
    guidesEnabled,
    snap,
    toggleLock,
    copyActive,
  ]);

  // ---- toolbar -----------------------------------------------------------

  const startToolbarDrag = (event: React.PointerEvent<HTMLDivElement>) => {
    if (locked || event.button !== 0) return;
    event.preventDefault();
    const element = toolbarRef.current;
    if (!element) return;
    const rect = element.getBoundingClientRect();
    setToolbarAt({ x: rect.left, y: rect.top });
    dragRef.current = {
      kind: "toolbar",
      offset: { x: event.clientX - rect.left, y: event.clientY - rect.top },
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  };

  const moveToolbar = (event: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current;
    if (drag.kind !== "toolbar") return;
    const element = toolbarRef.current;
    if (!element) return;
    const rect = element.getBoundingClientRect();
    setToolbarAt({
      x: clamp(event.clientX - drag.offset.x, 0, window.innerWidth - rect.width),
      y: clamp(event.clientY - drag.offset.y, 0, window.innerHeight - rect.height),
    });
  };

  const endToolbarDrag = () => {
    if (dragRef.current.kind !== "toolbar") return;
    dragRef.current = { kind: "none" };
    const element = toolbarRef.current;
    if (!element || !setup) return;
    const rect = element.getBoundingClientRect();
    void patch({
      rulerToolbarPos: {
        x: Math.round(rect.left * setup.scale + setup.origin[0]),
        y: Math.round(rect.top * setup.scale + setup.origin[1]),
      },
    }).catch(() => undefined);
  };

  const toolButton = (value: Tool, label: string, hint: string) => (
    <button
      key={value}
      type="button"
      title={hint}
      onClick={() => chooseTool(value)}
      className={`rounded px-1.5 py-0.5 transition-colors ${
        tool === value
          ? "bg-accent/25 text-accent-hi"
          : "text-ink-dim hover:bg-panel-2 hover:text-ink"
      }`}
    >
      {label}
    </button>
  );

  const toggleButton = (
    label: string,
    on: boolean,
    hint: string,
    action: () => void,
  ) => (
    <button
      type="button"
      title={hint}
      onClick={action}
      className={`rounded px-1.5 py-0.5 transition-colors ${
        on ? "bg-panel-3 text-ink" : "text-ink-faint hover:bg-panel-2 hover:text-ink"
      }`}
    >
      {label}
    </button>
  );

  const nextUnits = units === "physical" ? "dip" : units === "dip" ? "both" : "physical";

  return (
    <div className="fixed inset-0 overflow-hidden bg-transparent">
      <canvas
        ref={canvasRef}
        className="absolute inset-0 h-full w-full"
        style={{ cursor: "crosshair" }}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onContextMenu={(event) => {
          event.preventDefault();
          if (!locked) close();
        }}
      />

      <div
        ref={toolbarRef}
        onPointerDown={startToolbarDrag}
        onPointerMove={moveToolbar}
        onPointerUp={endToolbarDrag}
        onPointerCancel={endToolbarDrag}
        className={`absolute flex select-none items-center gap-0.5 rounded-panel border border-edge bg-panel/95 px-1 py-1 text-[11px] shadow-2xl ${
          locked ? "opacity-70" : ""
        }`}
        style={
          toolbarAt
            ? { left: toolbarAt.x, top: toolbarAt.y }
            : { left: "50%", top: 44, transform: "translateX(-50%)" }
        }
      >
        <span
          className="cursor-grab px-1 text-ink-faint"
          title="Drag the toolbar"
          aria-hidden="true"
        >
          ⠿
        </span>
        {toolButton("select", "Select", "Select and move measurements (V)")}
        {toolButton("distance", "Distance", "Measure a distance (D)")}
        {toolButton("box", "Box", "Measure a box or area (B)")}
        {toolButton("angle", "Angle", "Measure an angle with three clicks (A)")}
        <span className="mx-0.5 h-4 w-px bg-edge" aria-hidden="true" />
        {toggleButton(
          "Guides",
          guidesEnabled,
          guidesEnabled ? "Guides on (G)" : "Guides off (G)",
          () => void patch({ rulerGuides: !guidesEnabled }).catch(() => undefined),
        )}
        {toggleButton(
          "Snap",
          snap,
          snap ? "Snap to points, guides and the grid (S)" : "Snapping off (S)",
          () => void patch({ rulerSnap: !snap }).catch(() => undefined),
        )}
        <button
          type="button"
          title={`Units: ${units} (click to cycle)`}
          onClick={() => void patch({ rulerUnits: nextUnits }).catch(() => undefined)}
          className="rounded px-1.5 py-0.5 text-ink-dim transition-colors hover:bg-panel-2 hover:text-ink"
        >
          {units === "physical" ? "px" : units === "dip" ? "dip" : "both"}
        </button>
        <span className="mx-0.5 h-4 w-px bg-edge" aria-hidden="true" />
        <button
          type="button"
          title="Copy the measurement to the clipboard and history (C)"
          onClick={copyActive}
          className="rounded px-1.5 py-0.5 text-ink-dim transition-colors hover:bg-panel-2 hover:text-ink"
        >
          {copied ? "Copied" : "Copy"}
        </button>
        <button
          type="button"
          title={locked ? "Locked: click-through, Esc unlocks (L)" : "Lock: click-through; Esc unlocks (L)"}
          onClick={toggleLock}
          className={`rounded px-1.5 py-0.5 transition-colors ${
            locked ? "bg-accent/25 text-accent-hi" : "text-ink-dim hover:bg-panel-2 hover:text-ink"
          }`}
        >
          {locked ? "Locked" : "Lock"}
        </button>
        <button
          type="button"
          title="Close the ruler (Esc)"
          onClick={close}
          className="rounded px-1.5 py-0.5 text-ink-dim transition-colors hover:bg-panel-2 hover:text-ink"
        >
          ✕
        </button>
      </div>

      {locked && (
        <div className="pointer-events-none absolute inset-x-0 top-16 flex justify-center">
          <span className="rounded-full bg-panel/95 px-3 py-1 text-[11px] text-ink-dim shadow-lg">
            Click-through — press Esc to unlock, Esc again to close
          </span>
        </div>
      )}

      {error && (
        <div className="pointer-events-none absolute inset-x-0 bottom-6 flex justify-center">
          <span className="max-w-[520px] rounded bg-panel/95 px-3 py-1 text-[11px] text-danger shadow-lg">
            {error}
          </span>
        </div>
      )}

      {!setup && !error && (
        <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
          <span className="rounded-full bg-panel/95 px-3 py-1 text-[11px] text-ink-dim shadow-lg">
            Measuring the display…
          </span>
        </div>
      )}
    </div>
  );
}
