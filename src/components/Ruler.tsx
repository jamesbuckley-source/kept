/**
 * The onscreen ruler on Linux.
 *
 * On Windows the ruler is a GDI popup painted from its own thread. Linux has no
 * such surface, so the port is split: the backend opens a transparent,
 * full-screen webview (it also owns the desktop snapshot, the window list,
 * click-through while locked and Esc system-wide), and this file paints the
 * overlay on a canvas. The measurements, tools, snapping rules, readouts and
 * shortcuts mirror `src-tauri/src/ruler.rs` so both platforms mean the same
 * thing by a distance, a box or an angle.
 */

import { useEffect, useRef } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import {
  errorMessage,
  rulerOverlayClose,
  rulerContext,
  rulerCopy,
  rulerLock,
  rulerPatch,
  rulerRefresh,
} from "../lib/ipc";
import type { RulerContext, RulerRect } from "../lib/types";

type Point = { x: number; y: number };
type Size = { width: number; height: number };
type Tool = "select" | "distance" | "box" | "angle";
type Units = "physical" | "dip" | "both";
type CopyFormat = "px" | "pair" | "css" | "json";

type Measurement =
  | { kind: "distance"; a: Point; b: Point }
  | { kind: "box"; left: number; top: number; right: number; bottom: number }
  | { kind: "angle"; vertex: Point; a: Point; b: Point };

type Guide = { vertical: boolean; value: number };

const BUTTONS = [
  "select",
  "distance",
  "box",
  "angle",
  "guides",
  "snap",
  "units",
  "copy",
  "lock",
  "close",
] as const;
type Button = (typeof BUTTONS)[number];

const TOOL_BUTTONS: Record<string, Tool> = {
  select: "select",
  distance: "distance",
  box: "box",
  angle: "angle",
};

/** Copy the pointer had when the readout last confirmed a copy. */
const COPY_CONFIRM_MS = 1_200;

// ------------------------------------------------------------------ formatting

function fmt(value: number): string {
  const rounded = Math.round(value * 10) / 10;
  return Math.abs(rounded - Math.round(rounded)) < 0.05
    ? String(Math.round(rounded))
    : rounded.toFixed(1);
}

function grouped(value: number): string {
  return Math.round(value).toLocaleString("en-US");
}

const dip = (px: number, dpi: number) => (dpi ? (px * 96) / dpi : px);
const unitsLabel = (units: Units) =>
  units === "physical" ? "px" : units === "dip" ? "dip" : "both";

function nextUnits(units: Units): Units {
  return units === "physical" ? "dip" : units === "dip" ? "both" : "physical";
}

function unitText(px: number, units: Units, dpi: number): string {
  switch (units) {
    case "dip":
      return `${fmt(dip(px, dpi))} dip`;
    case "both":
      return `${fmt(px)} px · ${fmt(dip(px, dpi))} dip`;
    default:
      return `${fmt(px)} px`;
  }
}

function pairText(width: number, height: number, units: Units, dpi: number): string {
  if (units === "both") {
    return `${width} × ${height} px · ${fmt(dip(width, dpi))} × ${fmt(dip(height, dpi))} dip`;
  }
  const [w, h, unit] = numeric(width, height, units, dpi);
  return `${w} × ${h} ${unit}`;
}

function numeric(
  width: number,
  height: number,
  units: Units,
  dpi: number,
): [string, string, string] {
  return units === "dip"
    ? [fmt(dip(width, dpi)), fmt(dip(height, dpi)), "dip"]
    : [String(width), String(height), "px"];
}

function cssLength(value: number, units: Units, dpi: number): string {
  return units === "dip" ? `${fmt(dip(value, dpi))}dip` : `${value}px`;
}

// ------------------------------------------------------------------- geometry

const distance = (a: Point, b: Point) => Math.hypot(b.x - a.x, b.y - a.y);

function angleDegrees(vertex: Point, a: Point, b: Point): number {
  const u = { x: a.x - vertex.x, y: a.y - vertex.y };
  const v = { x: b.x - vertex.x, y: b.y - vertex.y };
  const lu = Math.hypot(u.x, u.y);
  const lv = Math.hypot(v.x, v.y);
  if (lu === 0 || lv === 0) return 0;
  const cos = Math.min(1, Math.max(-1, (u.x * v.x + u.y * v.y) / (lu * lv)));
  return (Math.acos(cos) * 180) / Math.PI;
}

function nearest(values: Iterable<number>, target: number, reach: number): number | null {
  let best: number | null = null;
  for (const value of values) {
    if (Math.abs(value - target) > reach) continue;
    if (best === null || Math.abs(value - target) < Math.abs(best - target)) best = value;
  }
  return best;
}

const gridValue = (value: number, step: number) => Math.round(value / step) * step;

function tickStep(scale: number): number {
  for (const step of [10, 25, 50, 100, 200, 500, 1000]) {
    if (step * scale >= 48) return step;
  }
  return 1000;
}

function near(point: Point, target: Point, reach: number): boolean {
  return Math.abs(point.x - target.x) <= reach && Math.abs(point.y - target.y) <= reach;
}

function toSegment(point: Point, a: Point, b: Point): number {
  const dx = b.x - a.x;
  const dy = b.y - a.y;
  const length = dx * dx + dy * dy;
  const t = length === 0 ? 0 : Math.min(1, Math.max(0, ((point.x - a.x) * dx + (point.y - a.y) * dy) / length));
  return Math.hypot(point.x - (a.x + t * dx), point.y - (a.y + t * dy));
}

/** The eight box handles, clockwise from the top-left corner. */
function boxHandles(left: number, top: number, right: number, bottom: number): Point[] {
  const cx = (left + right) / 2;
  const cy = (top + bottom) / 2;
  return [
    { x: left, y: top },
    { x: cx, y: top },
    { x: right, y: top },
    { x: right, y: cy },
    { x: right, y: bottom },
    { x: cx, y: bottom },
    { x: left, y: bottom },
    { x: left, y: cy },
  ];
}

/** Shift on a distance pulls it onto an axis, or onto the diagonal. */
function constrained(anchor: Point, point: Point): Point {
  const dx = point.x - anchor.x;
  const dy = point.y - anchor.y;
  if (Math.abs(dx) >= Math.abs(dy) * 2) return { x: point.x, y: anchor.y };
  if (Math.abs(dy) >= Math.abs(dx) * 2) return { x: anchor.x, y: point.y };
  const reach = Math.max(Math.abs(dx), Math.abs(dy));
  return {
    x: anchor.x + reach * (dx < 0 ? -1 : 1),
    y: anchor.y + reach * (dy < 0 ? -1 : 1),
  };
}

/** Shift on a box makes the drag from `anchor` square. */
function squarePoint(anchor: Point, point: Point): Point {
  const dx = point.x - anchor.x;
  const dy = point.y - anchor.y;
  const reach = Math.max(Math.abs(dx), Math.abs(dy));
  return {
    x: anchor.x + reach * (dx < 0 ? -1 : 1),
    y: anchor.y + reach * (dy < 0 ? -1 : 1),
  };
}

/** Shift on an angle rounds it to 15°, keeping the ray's length. */
function snapAngle(vertex: Point, a: Point, b: Point, step = 15): Point {
  const length = distance(vertex, b);
  if (length === 0) return b;
  const base = Math.atan2(a.y - vertex.y, a.x - vertex.x);
  const current = Math.atan2(b.y - vertex.y, b.x - vertex.x);
  const angle = base + Math.round(((current - base) * 180) / Math.PI / step) * step * (Math.PI / 180);
  return {
    x: vertex.x + Math.round(length * Math.cos(angle)),
    y: vertex.y + Math.round(length * Math.sin(angle)),
  };
}

function measurementSize(measurement: Measurement): Size {
  switch (measurement.kind) {
    case "distance":
      return {
        width: Math.abs(measurement.b.x - measurement.a.x),
        height: Math.abs(measurement.b.y - measurement.a.y),
      };
    case "box":
      return {
        width: Math.abs(measurement.right - measurement.left),
        height: Math.abs(measurement.bottom - measurement.top),
      };
    case "angle":
      return {
        width: Math.abs(Math.max(measurement.a.x, measurement.b.x) - measurement.vertex.x),
        height: Math.abs(Math.max(measurement.a.y, measurement.b.y) - measurement.vertex.y),
      };
  }
}

/** The number the toolbar leads with, matching the Windows ruler. */
function primary(measurement: Measurement, units: Units, dpi: number): string {
  switch (measurement.kind) {
    case "distance":
      return unitText(measurement.b.x - measurement.a.x, units, dpi);
    case "box": {
      const { width, height } = measurementSize(measurement);
      return pairText(width, height, units, dpi);
    }
    case "angle":
      return `${fmt(angleDegrees(measurement.vertex, measurement.a, measurement.b))}°`;
  }
}

/** The supporting line under the primary value. */
function secondary(measurement: Measurement): string {
  const { width, height } = measurementSize(measurement);
  switch (measurement.kind) {
    case "distance":
      return `Δ ${width} × ${height} · ${fmt(distance(measurement.a, measurement.b))} px`;
    case "box":
      return `area ${grouped(width * height)} px² · perimeter ${grouped(width * 2 + height * 2)} px`;
    case "angle":
      return `rays ${fmt(distance(measurement.vertex, measurement.a))} px · ${fmt(
        distance(measurement.vertex, measurement.b),
      )} px`;
  }
}

function formatted(
  measurement: Measurement,
  units: Units,
  format: CopyFormat,
  dpi: number,
): string {
  const { width, height } = measurementSize(measurement);
  const degrees = () =>
    measurement.kind === "angle"
      ? angleDegrees(measurement.vertex, measurement.a, measurement.b)
      : 0;
  switch (format) {
    case "pair":
      return measurement.kind === "angle" ? `${fmt(degrees())}°` : pairText(width, height, units, dpi);
    case "css":
      return measurement.kind === "angle"
        ? `transform: rotate(${fmt(degrees())}deg);`
        : `width: ${cssLength(width, units, dpi)}; height: ${cssLength(height, units, dpi)};`;
    case "json": {
      if (measurement.kind === "angle") return `{"degrees":${fmt(degrees())}}`;
      const [w, h, unit] = numeric(width, height, units, dpi);
      return `{"width":${w},"height":${h},"unit":"${unit}"}`;
    }
    default:
      return primary(measurement, units, dpi);
  }
}

// ------------------------------------------------------------------- the state

type Drag =
  | { kind: "none" }
  | { kind: "new" }
  | { kind: "move"; index: number; last: Point }
  | { kind: "handle"; index: number; handle: number }
  | { kind: "guide"; index: number }
  | { kind: "toolbar"; offset: Point };

type Palette = {
  panel: string;
  panel2: string;
  panel3: string;
  edge: string;
  ink: string;
  inkDim: string;
  inkFaint: string;
  accent: string;
  accentHi: string;
  danger: string;
};

function palette(): Palette {
  const css = getComputedStyle(document.documentElement);
  const read = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
  return {
    panel: read("--color-panel", "#12161f"),
    panel2: read("--color-panel-2", "#181d29"),
    panel3: read("--color-panel-3", "#1f2533"),
    edge: read("--color-edge", "#252c3b"),
    ink: read("--color-ink", "#e7eaf0"),
    inkDim: read("--color-ink-dim", "#99a3b5"),
    inkFaint: read("--color-ink-faint", "#6a7488"),
    accent: read("--color-accent", "#6d8cff"),
    accentHi: read("--color-accent-hi", "#8aa2ff"),
    danger: read("--color-danger", "#f87171"),
  };
}

function withAlpha(color: string, alpha: number): string {
  const hex = color.trim();
  if (hex.startsWith("#")) {
    const digits = hex.length === 4 ? hex.slice(1).split("").map((c) => c + c).join("") : hex.slice(1);
    const value = Number.parseInt(digits, 16);
    if (Number.isFinite(value) && digits.length === 6) {
      return `rgba(${(value >> 16) & 255}, ${(value >> 8) & 255}, ${value & 255}, ${alpha})`;
    }
  }
  return color;
}

class RulerOverlay {
  private readonly canvas: HTMLCanvasElement;
  private readonly ctx: CanvasRenderingContext2D;
  private readonly colors = palette();
  private scale = window.devicePixelRatio || 1;
  private width = 0;
  private height = 0;

  private readonly bounds: RulerRect;
  private readonly windowRects: RulerRect[];
  private tool: Tool = "distance";
  private units: Units;
  private copyFormat: CopyFormat;
  private snap: boolean;
  private readonly snapGrid: number;
  private readonly snapThreshold: number;
  private readonly edgeRulers: boolean;
  private guidesEnabled: boolean;

  private measurements: Measurement[] = [];
  private active: number | null = null;
  private guides: Guide[] = [];
  private selectedGuide: number | null = null;
  private pending: { kind: "distance" | "box"; a: Point; b: Point } | null = null;
  private anglePoints: Point[] = [];
  private pointer: Point = { x: 0, y: 0 };
  private snapHint: string | null = null;
  private hover: Button | null = null;
  private drag: Drag = { kind: "none" };
  private toolbar = { left: 0, top: 0, right: 0, bottom: 0 };
  private copiedAt: number | null = null;
  private locked = false;
  private alt = false;

  private snapshot: Uint8ClampedArray | null = null;
  private snapshotWidth = 0;
  private snapshotHeight = 0;
  /** The snapshot as a picture, used as the backdrop when the window is opaque. */
  private snapshotImage: HTMLImageElement | null = null;
  private readonly opaque: boolean;

  private dirty = true;
  private frame = 0;
  private readonly disposers: Array<() => void> = [];

  constructor(canvas: HTMLCanvasElement, context: RulerContext) {
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("the ruler needs a 2D canvas");
    this.canvas = canvas;
    this.ctx = ctx;
    this.bounds = context.bounds;
    this.windowRects = context.windows.map((rect) => this.toLocalRect(rect));
    this.units = context.units;
    this.copyFormat = context.copyFormat;
    this.snap = context.snap;
    this.snapGrid = context.snapGrid;
    this.snapThreshold = context.snapThreshold;
    this.edgeRulers = context.edgeRulers;
    this.guidesEnabled = context.guides;
    this.opaque = context.opaque;
    this.loadSnapshot(context.snapshot);
    this.measure();
    const [width, height] = this.toolbarSize();
    const stored = context.toolbarPos;
    const left = stored ? stored.x - this.bounds.left : (this.width - width) / 2;
    const top = stored ? stored.y - this.bounds.top : 40 * this.scale;
    this.toolbar = this.clampToolbar(left, top, width, height);
    this.pointer = { x: this.width / 2, y: this.height / 2 };
  }

  // ------------------------------------------------------------------ lifecycle

  mount(): void {
    this.measure();
    this.listen();
    const loop = () => {
      if (this.dirty) {
        this.dirty = false;
        this.draw();
      }
      this.frame = window.requestAnimationFrame(loop);
    };
    this.frame = window.requestAnimationFrame(loop);
    this.requestDraw();
  }

  destroy(): void {
    window.cancelAnimationFrame(this.frame);
    for (const dispose of this.disposers.splice(0)) dispose();
  }

  setLocked(locked: boolean): void {
    if (this.locked === locked) return;
    this.locked = locked;
    if (locked) this.hover = null;
    this.requestDraw();
  }

  private listen(): void {
    const on = <K extends keyof WindowEventMap>(
      target: Window,
      type: K,
      handler: (event: WindowEventMap[K]) => void,
    ) => {
      target.addEventListener(type, handler as EventListener);
      this.disposers.push(() => target.removeEventListener(type, handler as EventListener));
    };

    on(window, "resize", () => {
      this.measure();
      this.requestDraw();
    });
    on(window, "pointermove", (event) => this.pointerMoved(event));
    on(window, "pointerdown", (event) => this.pointerPressed(event));
    on(window, "pointerup", () => this.pointerReleased());
    on(window, "pointercancel", () => this.pointerReleased());
    on(window, "contextmenu", (event) => {
      event.preventDefault();
      if (!this.locked) void this.close();
    });
    on(window, "keydown", (event) => this.keyPressed(event));
    on(window, "keyup", (event) => {
      this.alt = event.altKey;
      this.requestDraw();
    });
  }

  // ------------------------------------------------------------------ geometry

  private measure(): void {
    this.scale = window.devicePixelRatio || 1;
    this.width = Math.max(1, Math.round(window.innerWidth * this.scale));
    this.height = Math.max(1, Math.round(window.innerHeight * this.scale));
    this.canvas.width = this.width;
    this.canvas.height = this.height;
  }

  private requestDraw(): void {
    this.dirty = true;
  }

  /** A screen point (physical, absolute) as a point on the canvas. */
  private point(event: { clientX: number; clientY: number }): Point {
    return {
      x: event.clientX * this.scale + this.bounds.left,
      y: event.clientY * this.scale + this.bounds.top,
    };
  }

  private toLocal(point: Point): Point {
    return { x: point.x - this.bounds.left, y: point.y - this.bounds.top };
  }

  private toLocalRect(rect: RulerRect): RulerRect {
    return {
      left: rect.left - this.bounds.left,
      top: rect.top - this.bounds.top,
      right: rect.right - this.bounds.left,
      bottom: rect.bottom - this.bounds.top,
    };
  }

  private strip(): number {
    return Math.round(18 * this.scale);
  }

  private inEdgeStrip(point: Point): boolean {
    const strip = this.strip();
    const local = this.toLocal(point);
    return (
      local.x < strip ||
      local.y < strip ||
      this.width - local.x < strip ||
      this.height - local.y < strip
    );
  }

  private toolbarSize(): [number, number] {
    const pad = Math.round(6 * this.scale);
    const grip = Math.round(16 * this.scale);
    const button = Math.round(32 * this.scale);
    const readout = Math.round(196 * this.scale);
    return [pad * 2 + grip + button * BUTTONS.length + readout, Math.round(34 * this.scale)];
  }

  private clampToolbar(left: number, top: number, width: number, height: number) {
    const visible = 40 * this.scale;
    const low = -width + visible;
    const high = this.width - visible;
    const x = low <= high ? Math.min(Math.max(left, low), high) : low;
    const y = Math.min(Math.max(top, 0), Math.max(0, this.height - visible));
    return { left: x, top: y, right: x + width, bottom: y + height };
  }

  private buttonAt(point: Point): Button | null {
    const local = this.toLocal(point);
    const { left, top, right, bottom } = this.toolbar;
    if (local.x < left || local.x > right || local.y < top || local.y > bottom) return null;
    const pad = Math.round(6 * this.scale);
    const grip = Math.round(16 * this.scale);
    const width = Math.round(32 * this.scale);
    const start = left + pad + grip;
    if (local.x < start) return null;
    const index = Math.floor((local.x - start) / width);
    return index >= 0 && index < BUTTONS.length ? BUTTONS[index] : null;
  }

  private buttonRect(index: number) {
    const pad = Math.round(6 * this.scale);
    const grip = Math.round(16 * this.scale);
    const width = Math.round(32 * this.scale);
    const left = this.toolbar.left + pad + grip + width * index;
    return { left, top: this.toolbar.top, right: left + width, bottom: this.toolbar.bottom };
  }

  // ------------------------------------------------------------------- snapshot

  private loadSnapshot(url: string | null): void {
    if (!url) {
      this.snapshot = null;
      return;
    }
    const image = new Image();
    image.onload = () => {
      const canvas = document.createElement("canvas");
      canvas.width = image.naturalWidth;
      canvas.height = image.naturalHeight;
      const ctx = canvas.getContext("2d", { willReadFrequently: true });
      if (!ctx) return;
      ctx.drawImage(image, 0, 0);
      this.snapshot = ctx.getImageData(0, 0, canvas.width, canvas.height).data;
      this.snapshotWidth = canvas.width;
      this.snapshotHeight = canvas.height;
      this.snapshotImage = image;
      this.requestDraw();
    };
    image.src = url;
  }

  private pixel(x: number, y: number): [number, number, number] {
    const pixels = this.snapshot;
    if (!pixels || x < 0 || y < 0 || x >= this.snapshotWidth || y >= this.snapshotHeight) {
      return [0, 0, 0];
    }
    const at = (y * this.snapshotWidth + x) * 4;
    return [pixels[at], pixels[at + 1], pixels[at + 2]];
  }

  /** The strongest colour edge within reach of `target`, like the GDI ruler. */
  private pixelEdge(target: number, axis: "x" | "y", threshold: number): number | null {
    if (!this.snapshot) return null;
    const local = this.toLocal(this.pointer);
    const start = Math.round(target);
    const limit = axis === "x" ? this.snapshotWidth : this.snapshotHeight;
    if (start < 0 || start >= limit) return null;
    const from = Math.max(0, start - threshold);
    const to = Math.min(limit - 2, start + threshold);
    if (to <= from) return null;
    const fixed =
      axis === "x"
        ? Math.min(Math.max(local.y, 0), this.snapshotHeight - 1)
        : Math.min(Math.max(local.x, 0), this.snapshotWidth - 1);
    let best = 0;
    let at: number | null = null;
    for (let index = from; index <= to; index += 1) {
      const first = axis === "x" ? this.pixel(index, fixed) : this.pixel(fixed, index);
      const second =
        axis === "x" ? this.pixel(index + 1, fixed) : this.pixel(fixed, index + 1);
      const contrast =
        Math.abs(first[0] - second[0]) +
        Math.abs(first[1] - second[1]) +
        Math.abs(first[2] - second[2]);
      if (contrast > best) {
        best = contrast;
        at = index + 1;
      }
    }
    if (best < 48 || at === null) return null;
    return axis === "x" ? this.bounds.left + at : this.bounds.top + at;
  }

  // -------------------------------------------------------------------- snapping

  private anchors(): Point[] {
    const out: Point[] = [];
    for (const measurement of this.measurements) {
      switch (measurement.kind) {
        case "distance":
          out.push(measurement.a, measurement.b);
          break;
        case "box":
          out.push(
            { x: measurement.left, y: measurement.top },
            { x: measurement.right, y: measurement.top },
            { x: measurement.left, y: measurement.bottom },
            { x: measurement.right, y: measurement.bottom },
          );
          break;
        case "angle":
          out.push(measurement.vertex, measurement.a, measurement.b);
          break;
      }
    }
    return out;
  }

  private snapAxis(target: number, axis: "x" | "y"): [number | null, string | null] {
    const threshold = this.snapThreshold;
    const guide = nearest(
      this.guides
        .filter((entry) => entry.vertical === (axis === "x"))
        .map((entry) => entry.value),
      target,
      threshold,
    );
    if (guide !== null) return [guide, "guide"];

    const window = nearest(
      this.windowRects.flatMap((rect) =>
        axis === "x" ? [rect.left, rect.right] : [rect.top, rect.bottom],
      ),
      target,
      threshold,
    );
    if (window !== null) return [window, "window"];

    const edge = this.pixelEdge(target, axis, threshold);
    if (edge !== null) return [edge, "edge"];

    if (this.snapGrid > 0) {
      const value = gridValue(target, this.snapGrid);
      if (Math.abs(value - target) <= threshold) return [value, "grid"];
    }
    return [null, null];
  }

  private snapPoint(point: Point): [Point, string | null] {
    if (!this.snap || this.alt) return [point, null];
    const threshold = this.snapThreshold;
    const anchor = this.anchors()
      .filter(
        (candidate) =>
          Math.abs(candidate.x - point.x) <= threshold &&
          Math.abs(candidate.y - point.y) <= threshold,
      )
      .sort(
        (a, b) =>
          Math.max(Math.abs(a.x - point.x), Math.abs(a.y - point.y)) -
          Math.max(Math.abs(b.x - point.x), Math.abs(b.y - point.y)),
      )[0];
    if (anchor) return [anchor, "point"];

    let out = { ...point };
    let kind: string | null = null;
    const [x, xKind] = this.snapAxis(point.x, "x");
    if (x !== null) {
      out.x = x;
      kind = xKind;
    }
    const [y, yKind] = this.snapAxis(point.y, "y");
    if (y !== null) {
      out.y = y;
      kind = kind && yKind ? (kind < yKind ? kind : yKind) : (yKind ?? kind);
    }
    out.x = Math.min(Math.max(out.x, this.bounds.left), this.bounds.right - 1);
    out.y = Math.min(Math.max(out.y, this.bounds.top), this.bounds.bottom - 1);
    return [out, kind];
  }

  // ---------------------------------------------------------------- hit testing

  private measurementHit(
    point: Point,
  ): { index: number; handle: number | null } | null {
    const reach = Math.round(8 * this.scale);
    for (let index = this.measurements.length - 1; index >= 0; index -= 1) {
      const measurement = this.measurements[index];
      switch (measurement.kind) {
        case "distance":
          if (near(point, measurement.a, reach)) return { index, handle: 0 };
          if (near(point, measurement.b, reach)) return { index, handle: 1 };
          if (toSegment(point, measurement.a, measurement.b) <= 6 * this.scale) {
            return { index, handle: null };
          }
          break;
        case "box": {
          const handles = boxHandles(
            measurement.left,
            measurement.top,
            measurement.right,
            measurement.bottom,
          );
          for (let handle = 0; handle < handles.length; handle += 1) {
            if (near(point, handles[handle], reach)) return { index, handle };
          }
          if (
            point.x >= measurement.left &&
            point.x <= measurement.right &&
            point.y >= measurement.top &&
            point.y <= measurement.bottom
          ) {
            return { index, handle: null };
          }
          break;
        }
        case "angle":
          for (let handle = 0; handle < 3; handle += 1) {
            const at = [measurement.vertex, measurement.a, measurement.b][handle];
            if (near(point, at, reach)) return { index, handle };
          }
          if (
            toSegment(point, measurement.vertex, measurement.a) <= 6 * this.scale ||
            toSegment(point, measurement.vertex, measurement.b) <= 6 * this.scale
          ) {
            return { index, handle: null };
          }
          break;
      }
    }
    return null;
  }

  private guideHit(point: Point): number | null {
    const reach = Math.round(4 * this.scale);
    const local = this.toLocal(point);
    const index = this.guides.findIndex((guide) =>
      guide.vertical
        ? Math.abs(local.x - (guide.value - this.bounds.left)) <= reach
        : Math.abs(local.y - (guide.value - this.bounds.top)) <= reach,
    );
    return index === -1 ? null : index;
  }

  private cursorFor(point: Point): string {
    if (this.buttonAt(point) || this.insideToolbar(point)) return "default";
    if (this.measurementHit(point)) return "move";
    if (this.guideHit(point) !== null) return "ew-resize";
    return "crosshair";
  }

  private insideToolbar(point: Point): boolean {
    const local = this.toLocal(point);
    return (
      local.x >= this.toolbar.left &&
      local.x <= this.toolbar.right &&
      local.y >= this.toolbar.top &&
      local.y <= this.toolbar.bottom
    );
  }

  // ------------------------------------------------------------------- gestures

  private pointerMoved(event: PointerEvent): void {
    if (this.locked) return;
    this.alt = event.altKey;
    const point = this.point(event);
    this.pointer = point;

    switch (this.drag.kind) {
      case "new": {
        if (!this.pending) break;
        const [snapped, kind] = this.snapPoint(point);
        this.snapHint = kind;
        const anchor = this.pending.a;
        const b =
          this.pending.kind === "distance"
            ? event.shiftKey
              ? constrained(anchor, snapped)
              : snapped
            : event.shiftKey
              ? squarePoint(anchor, snapped)
              : snapped;
        this.pending = { ...this.pending, b };
        break;
      }
      case "handle": {
        const [snapped, kind] = this.snapPoint(point);
        this.snapHint = kind;
        const current = this.measurements[this.drag.index];
        if (current) {
          this.measurements[this.drag.index] = this.resized(
            current,
            this.drag.handle,
            snapped,
            event.shiftKey,
          );
          this.active = this.drag.index;
        }
        break;
      }
      case "move": {
        const current = this.measurements[this.drag.index];
        if (current) {
          const delta = { x: point.x - this.drag.last.x, y: point.y - this.drag.last.y };
          this.measurements[this.drag.index] = this.moved(current, delta);
          this.active = this.drag.index;
          this.drag = { kind: "move", index: this.drag.index, last: point };
        }
        break;
      }
      case "guide": {
        const guide = this.guides[this.drag.index];
        if (guide) {
          this.guides[this.drag.index] = {
            vertical: guide.vertical,
            value: guide.vertical ? point.x : point.y,
          };
        }
        break;
      }
      case "toolbar": {
        const local = this.toLocal(point);
        const width = this.toolbar.right - this.toolbar.left;
        const height = this.toolbar.bottom - this.toolbar.top;
        this.toolbar = this.clampToolbar(
          local.x - this.drag.offset.x,
          local.y - this.drag.offset.y,
          width,
          height,
        );
        break;
      }
      default:
        this.hover = this.buttonAt(point);
    }

    this.canvas.style.cursor = this.drag.kind === "none" ? this.cursorFor(point) : "crosshair";
    this.requestDraw();
  }

  private pointerPressed(event: PointerEvent): void {
    if (this.locked || event.button !== 0) return;
    event.preventDefault();
    const point = this.point(event);
    this.pointer = point;
    this.alt = event.altKey;

    const button = this.buttonAt(point);
    if (button) {
      this.handleButton(button);
      return;
    }

    const local = this.toLocal(point);
    if (this.guidesEnabled && this.edgeRulers && this.inEdgeStrip(point)) {
      const horizontal =
        local.y < this.strip() || this.height - local.y < this.strip();
      this.guides.push({
        vertical: !horizontal,
        value: horizontal ? point.y : point.x,
      });
      this.selectedGuide = this.guides.length - 1;
      this.drag = { kind: "guide", index: this.selectedGuide };
      this.requestDraw();
      return;
    }

    if (this.insideToolbar(point)) {
      this.drag = {
        kind: "toolbar",
        offset: { x: local.x - this.toolbar.left, y: local.y - this.toolbar.top },
      };
      return;
    }

    const hit = this.measurementHit(point);
    if (hit) {
      this.active = hit.index;
      this.selectedGuide = null;
      this.drag =
        hit.handle === null
          ? { kind: "move", index: hit.index, last: point }
          : { kind: "handle", index: hit.index, handle: hit.handle };
      this.requestDraw();
      return;
    }

    const guide = this.guideHit(point);
    if (guide !== null) {
      this.selectedGuide = guide;
      this.active = null;
      this.drag = { kind: "guide", index: guide };
      this.requestDraw();
      return;
    }

    this.active = null;
    this.selectedGuide = null;
    this.snapHint = null;

    if (this.tool === "select") {
      this.requestDraw();
      return;
    }
    if (this.tool === "angle") {
      const [snapped, kind] = this.snapPoint(point);
      this.snapHint = kind;
      this.anglePoints.push(snapped);
      if (this.anglePoints.length === 3) {
        const [vertex, a] = this.anglePoints;
        const b = event.shiftKey
          ? snapAngle(vertex, a, this.anglePoints[2])
          : this.anglePoints[2];
        this.measurements.push({ kind: "angle", vertex, a, b });
        this.active = this.measurements.length - 1;
        this.anglePoints = [];
        this.snapHint = null;
      }
      this.requestDraw();
      return;
    }

    const [snapped, kind] = this.snapPoint(point);
    this.snapHint = kind;
    this.pending = { kind: this.tool, a: snapped, b: snapped };
    this.drag = { kind: "new" };
    this.requestDraw();
  }

  private pointerReleased(): void {
    if (this.locked) return;
    const drag = this.drag;
    this.drag = { kind: "none" };
    this.snapHint = null;

    if (drag.kind === "new") this.commit();
    if (drag.kind === "toolbar") {
      void rulerPatch({
        rulerToolbarPos: {
          x: Math.round(this.toolbar.left + this.bounds.left),
          y: Math.round(this.toolbar.top + this.bounds.top),
        },
      });
    }
    if (drag.kind === "guide") {
      const guide = this.guides[drag.index];
      if (guide) {
        const strip = this.strip();
        const local = this.toLocal(this.pointer);
        const inside = guide.vertical
          ? local.x < strip || this.width - local.x < strip
          : local.y < strip || this.height - local.y < strip;
        if (inside) {
          this.guides.splice(drag.index, 1);
          this.selectedGuide = null;
        }
      }
    }
    this.requestDraw();
  }

  private commit(): void {
    const pending = this.pending;
    this.pending = null;
    if (!pending) return;
    if (pending.kind === "distance") {
      const { a, b } = pending;
      if (Math.abs(a.x - b.x) + Math.abs(a.y - b.y) > 2) {
        this.measurements.push({ kind: "distance", a, b });
        this.active = this.measurements.length - 1;
      }
      return;
    }
    const left = Math.min(pending.a.x, pending.b.x);
    const right = Math.max(pending.a.x, pending.b.x);
    const top = Math.min(pending.a.y, pending.b.y);
    const bottom = Math.max(pending.a.y, pending.b.y);
    if (right - left > 1 && bottom - top > 1) {
      this.measurements.push({ kind: "box", left, top, right, bottom });
      this.active = this.measurements.length - 1;
    }
  }

  private resized(
    measurement: Measurement,
    handle: number,
    point: Point,
    square: boolean,
  ): Measurement {
    if (measurement.kind === "distance") {
      return handle === 0
        ? { kind: "distance", a: point, b: measurement.b }
        : { kind: "distance", a: measurement.a, b: point };
    }
    if (measurement.kind === "angle") {
      if (handle === 0) return { kind: "angle", vertex: point, a: measurement.a, b: measurement.b };
      if (handle === 1) return { kind: "angle", vertex: measurement.vertex, a: point, b: measurement.b };
      return { kind: "angle", vertex: measurement.vertex, a: measurement.a, b: point };
    }
    let { left, top, right, bottom } = measurement;
    switch (handle) {
      case 0:
        left = point.x;
        top = point.y;
        break;
      case 1:
        top = point.y;
        break;
      case 2:
        right = point.x;
        top = point.y;
        break;
      case 3:
        right = point.x;
        break;
      case 4:
        right = point.x;
        bottom = point.y;
        break;
      case 5:
        bottom = point.y;
        break;
      case 6:
        left = point.x;
        bottom = point.y;
        break;
      default:
        left = point.x;
    }
    if (square) {
      const side = Math.max(Math.abs(right - left), Math.abs(bottom - top));
      if (handle === 0 || handle === 6 || handle === 7) left = right - side;
      else right = left + side;
      if (handle === 0 || handle === 1 || handle === 2) top = bottom - side;
      else bottom = top + side;
    }
    return {
      kind: "box",
      left: Math.min(left, right),
      top: Math.min(top, bottom),
      right: Math.max(left, right),
      bottom: Math.max(top, bottom),
    };
  }

  private moved(measurement: Measurement, delta: Point): Measurement {
    const clamp = (point: Point): Point => ({
      x: Math.min(Math.max(point.x + delta.x, this.bounds.left), this.bounds.right),
      y: Math.min(Math.max(point.y + delta.y, this.bounds.top), this.bounds.bottom),
    });
    if (measurement.kind === "distance") {
      return { kind: "distance", a: clamp(measurement.a), b: clamp(measurement.b) };
    }
    if (measurement.kind === "angle") {
      return {
        kind: "angle",
        vertex: clamp(measurement.vertex),
        a: clamp(measurement.a),
        b: clamp(measurement.b),
      };
    }
    const width = measurement.right - measurement.left;
    const height = measurement.bottom - measurement.top;
    const left = Math.min(
      Math.max(measurement.left + delta.x, this.bounds.left),
      this.bounds.right - width,
    );
    const top = Math.min(
      Math.max(measurement.top + delta.y, this.bounds.top),
      this.bounds.bottom - height,
    );
    return { kind: "box", left, top, right: left + width, bottom: top + height };
  }

  // -------------------------------------------------------------------- buttons

  private handleButton(button: Button): void {
    const tool = TOOL_BUTTONS[button];
    if (tool) {
      this.tool = tool;
      if (tool === "angle") this.anglePoints = [];
      this.requestDraw();
      return;
    }
    switch (button) {
      case "guides":
        this.guidesEnabled = !this.guidesEnabled;
        void rulerPatch({ rulerGuides: this.guidesEnabled });
        break;
      case "snap":
        this.snap = !this.snap;
        void rulerPatch({ rulerSnap: this.snap });
        break;
      case "units":
        this.units = nextUnits(this.units);
        void rulerPatch({ rulerUnits: this.units });
        break;
      case "copy":
        void this.copy();
        break;
      case "lock": {
        const next = !this.locked;
        this.locked = next;
        this.hover = null;
        void rulerLock(next).catch((error) => {
          console.error("the ruler could not lock", errorMessage(error));
          this.locked = !next;
          this.requestDraw();
        });
        break;
      }
      case "close":
        void this.close();
        break;
    }
    this.requestDraw();
  }

  private async copy(): Promise<void> {
    const measurement = this.measurement();
    if (!measurement) return;
    const text = formatted(measurement, this.units, this.copyFormat, Math.round(96 * this.scale));
    try {
      await rulerCopy(text);
      this.copiedAt = performance.now();
      this.requestDraw();
      window.setTimeout(() => this.requestDraw(), COPY_CONFIRM_MS + 80);
    } catch (error) {
      console.error("the ruler could not copy the measurement", errorMessage(error));
    }
  }

  private measurement(): Measurement | null {
    if (this.pending) {
      return this.pending.kind === "distance"
        ? { kind: "distance", a: this.pending.a, b: this.pending.b }
        : {
            kind: "box",
            left: Math.min(this.pending.a.x, this.pending.b.x),
            top: Math.min(this.pending.a.y, this.pending.b.y),
            right: Math.max(this.pending.a.x, this.pending.b.x),
            bottom: Math.max(this.pending.a.y, this.pending.b.y),
          };
    }
    return this.active === null ? null : (this.measurements[this.active] ?? null);
  }

  private async close(): Promise<void> {
    await rulerOverlayClose().catch(() => undefined);
  }

  // ------------------------------------------------------------------ keyboard

  private keyPressed(event: KeyboardEvent): void {
    this.alt = event.altKey;
    const step = event.shiftKey ? 10 : 1;
    const key = event.key;

    if (key === "Escape") {
      event.preventDefault();
      void this.close();
      return;
    }
    if (key === "F5") {
      event.preventDefault();
      void rulerRefresh()
        .then((context) => {
          this.loadSnapshot(context.snapshot);
          this.requestDraw();
        })
        .catch((error) => console.error("the ruler could not refresh", errorMessage(error)));
      return;
    }
    if (key === "Backspace" || key === "Delete") {
      event.preventDefault();
      if (this.selectedGuide !== null) {
        this.guides.splice(this.selectedGuide, 1);
        this.selectedGuide = null;
      } else if (this.active !== null) {
        this.measurements.splice(this.active, 1);
        this.active = null;
      }
      this.requestDraw();
      return;
    }
    if (key === "Tab") {
      event.preventDefault();
      const count = this.measurements.length;
      if (count > 0) {
        this.active =
          this.active === null
            ? 0
            : (this.active + (event.shiftKey ? count - 1 : 1)) % count;
        this.selectedGuide = null;
        this.requestDraw();
      }
      return;
    }
    if (key.startsWith("Arrow")) {
      event.preventDefault();
      const delta =
        key === "ArrowLeft"
          ? { x: -step, y: 0 }
          : key === "ArrowRight"
            ? { x: step, y: 0 }
            : key === "ArrowUp"
              ? { x: 0, y: -step }
              : { x: 0, y: step };
      if (this.selectedGuide !== null) {
        const guide = this.guides[this.selectedGuide];
        if (guide) {
          this.guides[this.selectedGuide] = {
            vertical: guide.vertical,
            value: guide.value + (guide.vertical ? delta.x : delta.y),
          };
        }
      } else if (this.active !== null) {
        const measurement = this.measurements[this.active];
        if (measurement) this.measurements[this.active] = this.moved(measurement, delta);
      }
      this.requestDraw();
      return;
    }
    const letter = key.length === 1 ? key.toLowerCase() : key;
    const tools: Record<string, Tool> = { v: "select", d: "distance", b: "box", a: "angle" };
    if (tools[letter]) {
      event.preventDefault();
      this.tool = tools[letter];
      if (this.tool === "angle") this.anglePoints = [];
      this.requestDraw();
    } else if (letter === "g") {
      event.preventDefault();
      this.guidesEnabled = !this.guidesEnabled;
      this.requestDraw();
    } else if (letter === "s") {
      event.preventDefault();
      this.snap = !this.snap;
      this.requestDraw();
    } else if (letter === "l") {
      event.preventDefault();
      const next = !this.locked;
      this.locked = next;
      void rulerLock(next).catch((error) => {
        console.error("the ruler could not lock", errorMessage(error));
        this.locked = !next;
        this.requestDraw();
      });
      this.requestDraw();
    } else if (letter === "c") {
      event.preventDefault();
      void this.copy();
    }
  }

  // -------------------------------------------------------------------- drawing

  private draw(): void {
    const { ctx } = this;
    ctx.clearRect(0, 0, this.width, this.height);
    if (this.opaque) this.drawBackdrop();
    if (this.edgeRulers) this.drawEdgeRulers();
    this.drawGuides();
    this.measurements.forEach((measurement, index) => {
      this.drawMeasurement(measurement, index === this.active);
    });
    this.drawPending();
    this.drawCrosshair();
    this.drawSnapHint();
    this.drawToolbar();
    this.drawLocked();
  }

  /** The desktop itself, for a session with no compositor to blend through. */
  private drawBackdrop(): void {
    const { ctx, colors } = this;
    if (this.snapshotImage) {
      ctx.drawImage(this.snapshotImage, 0, 0, this.width, this.height);
    } else {
      ctx.fillStyle = colors.panel;
      ctx.fillRect(0, 0, this.width, this.height);
    }
  }

  private drawEdgeRulers(): void {
    const { ctx, colors } = this;
    const strip = this.strip();
    const step = tickStep(this.scale);
    const mid = Math.max(Math.floor(step / 2), 5);
    const label = `${Math.round(10 * this.scale)}px ui-sans-serif, system-ui, sans-serif`;

    ctx.fillStyle = withAlpha(colors.panel, 0.94);
    ctx.fillRect(0, 0, this.width, strip);
    ctx.fillRect(0, this.height - strip, this.width, strip);
    ctx.fillRect(0, 0, strip, this.height);
    ctx.fillRect(this.width - strip, 0, strip, this.height);

    ctx.strokeStyle = withAlpha(colors.edge, 0.95);
    ctx.lineWidth = Math.max(1, Math.round(this.scale));
    ctx.beginPath();
    ctx.moveTo(0, strip - 0.5);
    ctx.lineTo(this.width, strip - 0.5);
    ctx.moveTo(0, this.height - strip + 0.5);
    ctx.lineTo(this.width, this.height - strip + 0.5);
    ctx.moveTo(strip - 0.5, 0);
    ctx.lineTo(strip - 0.5, this.height);
    ctx.moveTo(this.width - strip + 0.5, 0);
    ctx.lineTo(this.width - strip + 0.5, this.height);
    ctx.stroke();

    ctx.font = label;
    ctx.fillStyle = colors.inkDim;
    ctx.textBaseline = "top";

    const unit = Math.max(1, Math.round(this.scale));
    for (let x = 0; x < this.width; x += 5 * this.scale) {
      const value = this.bounds.left + Math.round(x);
      const major = value % step === 0;
      const medium = value % mid === 0;
      const length = Math.round(strip / (major ? 2 : medium ? 3 : 5));
      ctx.fillStyle = colors.inkFaint;
      ctx.fillRect(Math.round(x), strip - length, unit, length);
      ctx.fillRect(Math.round(x), this.height - strip, unit, length);
      if (major) {
        ctx.fillStyle = colors.inkDim;
        ctx.fillText(String(value), Math.round(x + 2 * this.scale), 1);
        ctx.fillText(
          String(value),
          Math.round(x + 2 * this.scale),
          this.height - strip + 2 * this.scale,
        );
      }
    }

    for (let y = 0; y < this.height; y += 5 * this.scale) {
      const value = this.bounds.top + Math.round(y);
      const major = value % step === 0;
      const medium = value % mid === 0;
      const length = Math.round(strip / (major ? 2 : medium ? 3 : 5));
      ctx.fillStyle = colors.inkFaint;
      ctx.fillRect(strip - length, Math.round(y), length, unit);
      ctx.fillRect(this.width - strip, Math.round(y), length, unit);
      if (major) {
        ctx.save();
        ctx.fillStyle = colors.inkDim;
        ctx.translate(strip - 3 * this.scale, Math.round(y - 2 * this.scale));
        ctx.rotate(-Math.PI / 2);
        ctx.fillText(String(value), 0, 0);
        ctx.restore();
        ctx.save();
        ctx.fillStyle = colors.inkDim;
        ctx.translate(this.width - 3 * this.scale, Math.round(y - 2 * this.scale));
        ctx.rotate(-Math.PI / 2);
        ctx.fillText(String(value), 0, 0);
        ctx.restore();
      }
    }

    // The pointer's own marks in the strips, like the GDI cross-highlight.
    const pointer = this.toLocal(this.pointer);
    ctx.fillStyle = colors.accent;
    ctx.fillRect(Math.round(pointer.x), 0, unit, strip);
    ctx.fillRect(Math.round(pointer.x), this.height - strip, unit, strip);
    ctx.fillRect(0, Math.round(pointer.y), strip, unit);
    ctx.fillRect(this.width - strip, Math.round(pointer.y), strip, unit);
  }

  private drawGuides(): void {
    const { ctx, colors } = this;
    this.guides.forEach((guide, index) => {
      const selected = index === this.selectedGuide;
      const at = guide.vertical ? guide.value - this.bounds.left : guide.value - this.bounds.top;
      ctx.strokeStyle = selected ? colors.accentHi : withAlpha(colors.accent, 0.75);
      ctx.lineWidth = Math.max(1, Math.round(this.scale));
      ctx.beginPath();
      if (guide.vertical) {
        ctx.moveTo(Math.round(at) + 0.5, 0);
        ctx.lineTo(Math.round(at) + 0.5, this.height);
      } else {
        ctx.moveTo(0, Math.round(at) + 0.5);
        ctx.lineTo(this.width, Math.round(at) + 0.5);
      }
      ctx.stroke();
    });
  }

  private drawMeasurement(measurement: Measurement, active: boolean): void {
    const { ctx, colors } = this;
    const color = active ? colors.accentHi : colors.accent;
    const width = Math.max(1, Math.round(this.scale));
    ctx.strokeStyle = color;
    ctx.lineWidth = width;

    if (measurement.kind === "distance") {
      const a = this.toLocal(measurement.a);
      const b = this.toLocal(measurement.b);
      ctx.beginPath();
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(b.x, b.y);
      ctx.stroke();
      this.handle(a, color);
      this.handle(b, color);
      const text = primary(measurement, this.units, Math.round(96 * this.scale));
      this.chip(
        text,
        active ? secondary(measurement) : null,
        (a.x + b.x) / 2,
        (a.y + b.y) / 2 - Math.round(14 * this.scale),
        "centre",
      );
      return;
    }

    if (measurement.kind === "box") {
      const left = measurement.left - this.bounds.left;
      const top = measurement.top - this.bounds.top;
      const right = measurement.right - this.bounds.left;
      const bottom = measurement.bottom - this.bounds.top;
      ctx.strokeRect(left, top, right - left, bottom - top);
      for (const at of boxHandles(left, top, right, bottom)) this.handle(at, color);
      const text = primary(measurement, this.units, Math.round(96 * this.scale));
      this.chip(
        text,
        active ? secondary(measurement) : null,
        left,
        top - Math.round(14 * this.scale),
        "left",
      );
      return;
    }

    const vertex = this.toLocal(measurement.vertex);
    const a = this.toLocal(measurement.a);
    const b = this.toLocal(measurement.b);
    ctx.beginPath();
    ctx.moveTo(vertex.x, vertex.y);
    ctx.lineTo(a.x, a.y);
    ctx.moveTo(vertex.x, vertex.y);
    ctx.lineTo(b.x, b.y);
    ctx.stroke();
    const radius = Math.round(26 * this.scale);
    const start = Math.atan2(a.y - vertex.y, a.x - vertex.x);
    const end = Math.atan2(b.y - vertex.y, b.x - vertex.x);
    ctx.beginPath();
    ctx.arc(vertex.x, vertex.y, radius, Math.min(start, end), Math.max(start, end));
    ctx.stroke();
    this.handle(vertex, color);
    this.handle(a, color);
    this.handle(b, color);
    this.chip(
      primary(measurement, this.units, Math.round(96 * this.scale)),
      active ? secondary(measurement) : null,
      vertex.x + Math.round(radius * Math.cos((start + end) / 2)),
      vertex.y + Math.round(radius * Math.sin((start + end) / 2)),
      "centre",
    );
  }

  private drawPending(): void {
    const pending = this.pending;
    if (!pending) return;
    const { ctx, colors } = this;
    const width = Math.max(1, Math.round(this.scale));
    ctx.strokeStyle = colors.accentHi;
    ctx.lineWidth = width;
    ctx.setLineDash([Math.round(6 * this.scale), Math.round(4 * this.scale)]);
    const a = this.toLocal(pending.a);
    const b = this.toLocal(pending.b);
    if (pending.kind === "distance") {
      ctx.beginPath();
      ctx.moveTo(a.x, a.y);
      ctx.lineTo(b.x, b.y);
      ctx.stroke();
    } else {
      ctx.strokeRect(
        Math.min(a.x, b.x),
        Math.min(a.y, b.y),
        Math.abs(b.x - a.x),
        Math.abs(b.y - a.y),
      );
    }
    ctx.setLineDash([]);
    const measurement: Measurement =
      pending.kind === "distance"
        ? { kind: "distance", a: pending.a, b: pending.b }
        : {
            kind: "box",
            left: Math.min(pending.a.x, pending.b.x),
            top: Math.min(pending.a.y, pending.b.y),
            right: Math.max(pending.a.x, pending.b.x),
            bottom: Math.max(pending.a.y, pending.b.y),
          };
    this.chip(
      primary(measurement, this.units, Math.round(96 * this.scale)),
      secondary(measurement),
      (a.x + b.x) / 2,
      (a.y + b.y) / 2 - Math.round(16 * this.scale),
      "centre",
    );
  }

  private drawCrosshair(): void {
    if (this.drag.kind !== "none" || this.locked) return;
    if (this.insideToolbar(this.pointer)) return;
    const { ctx, colors } = this;
    const pointer = this.toLocal(this.pointer);
    ctx.save();
    ctx.strokeStyle = withAlpha(colors.accent, 0.35);
    ctx.lineWidth = 1;
    ctx.setLineDash([Math.round(3 * this.scale), Math.round(4 * this.scale)]);
    ctx.beginPath();
    ctx.moveTo(Math.round(pointer.x) + 0.5, 0);
    ctx.lineTo(Math.round(pointer.x) + 0.5, this.height);
    ctx.moveTo(0, Math.round(pointer.y) + 0.5);
    ctx.lineTo(this.width, Math.round(pointer.y) + 0.5);
    ctx.stroke();
    ctx.restore();
  }

  private drawSnapHint(): void {
    if (!this.snapHint) return;
    const { ctx, colors } = this;
    const pointer = this.toLocal(this.pointer);
    ctx.save();
    ctx.strokeStyle = colors.accentHi;
    ctx.lineWidth = Math.max(1, Math.round(this.scale));
    const arm = Math.round(6 * this.scale);
    ctx.beginPath();
    ctx.moveTo(pointer.x - arm, pointer.y);
    ctx.lineTo(pointer.x + arm, pointer.y);
    ctx.moveTo(pointer.x, pointer.y - arm);
    ctx.lineTo(pointer.x, pointer.y + arm);
    ctx.stroke();
    ctx.restore();
    this.chip(
      this.snapHint,
      null,
      pointer.x + Math.round(14 * this.scale),
      pointer.y + Math.round(10 * this.scale),
      "left",
    );
  }

  private drawToolbar(): void {
    const { ctx, colors } = this;
    const { left, top, right, bottom } = this.toolbar;
    const radius = Math.round(7 * this.scale);
    const width = Math.max(1, Math.round(this.scale));

    ctx.save();
    ctx.shadowColor = "rgba(0, 0, 0, 0.45)";
    ctx.shadowBlur = 16 * this.scale;
    ctx.shadowOffsetY = 4 * this.scale;
    ctx.fillStyle = withAlpha(colors.panel, 0.97);
    this.roundRect(left, top, right - left, bottom - top, radius);
    ctx.fill();
    ctx.restore();
    ctx.strokeStyle = this.locked ? colors.accent : colors.edge;
    ctx.lineWidth = width;
    this.roundRect(left, top, right - left, bottom - top, radius);
    ctx.stroke();

    const pad = Math.round(6 * this.scale);
    const grip = Math.round(16 * this.scale);
    const gripX = left + pad + Math.round(grip / 2);
    ctx.fillStyle = this.locked ? withAlpha(colors.accentHi, 0.8) : colors.inkFaint;
    for (let row = 0; row < 3; row += 1) {
      for (let column = 0; column < 2; column += 1) {
        ctx.beginPath();
        ctx.arc(
          gripX - 2 * this.scale + column * 4 * this.scale,
          top + bottom / 2 - 4 * this.scale + row * 4 * this.scale,
          Math.max(1, this.scale),
          0,
          Math.PI * 2,
        );
        ctx.fill();
      }
    }

    BUTTONS.forEach((button, index) => {
      const rect = this.buttonRect(index);
      const hovered = this.hover === button;
      const selected =
        (button === "select" && this.tool === "select") ||
        (button === "distance" && this.tool === "distance") ||
        (button === "box" && this.tool === "box") ||
        (button === "angle" && this.tool === "angle") ||
        (button === "guides" && this.guidesEnabled) ||
        (button === "snap" && this.snap) ||
        (button === "lock" && this.locked);
      if (hovered || selected) {
        ctx.fillStyle = selected ? withAlpha(colors.accent, 0.22) : withAlpha(colors.panel3, 0.95);
        this.roundRect(
          rect.left + 2 * this.scale,
          rect.top + 3 * this.scale,
          rect.right - rect.left - 4 * this.scale,
          rect.bottom - rect.top - 6 * this.scale,
          5 * this.scale,
        );
        ctx.fill();
      }
      this.drawIcon(
        button,
        rect,
        selected ? colors.accentHi : hovered ? colors.ink : colors.inkDim,
      );
    });

    this.drawReadout();

    if (this.hover) {
      const index = BUTTONS.indexOf(this.hover);
      const rect = this.buttonRect(index);
      const text = this.fullLabel(this.hover);
      ctx.font = `${Math.round(11 * this.scale)}px ui-sans-serif, system-ui, sans-serif`;
      const metrics = ctx.measureText(text);
      const w = metrics.width + 16 * this.scale;
      const h = Math.round(22 * this.scale);
      let x = (rect.left + rect.right) / 2 - w / 2;
      x = Math.min(Math.max(x, 4 * this.scale), this.width - w - 4 * this.scale);
      const y = top > h + 6 * this.scale ? top - h - 6 * this.scale : bottom + 6 * this.scale;
      ctx.fillStyle = withAlpha(colors.panel2, 0.98);
      this.roundRect(x, y, w, h, 5 * this.scale);
      ctx.fill();
      ctx.strokeStyle = colors.edge;
      ctx.lineWidth = width;
      ctx.stroke();
      ctx.fillStyle = colors.ink;
      ctx.textBaseline = "middle";
      ctx.fillText(text, x + 8 * this.scale, y + h / 2);
    }
  }

  private drawReadout(): void {
    const { ctx, colors } = this;
    const pad = Math.round(6 * this.scale);
    const readout = Math.round(196 * this.scale);
    const left = this.toolbar.right - pad - readout;
    const measurement = this.measurement();
    const copied =
      this.copiedAt !== null && performance.now() - this.copiedAt < COPY_CONFIRM_MS;

    ctx.textBaseline = "middle";
    if (copied) {
      ctx.fillStyle = colors.accentHi;
      ctx.font = `600 ${Math.round(12 * this.scale)}px ui-sans-serif, system-ui, sans-serif`;
      ctx.fillText("Copied to clipboard", left, (this.toolbar.top + this.toolbar.bottom) / 2);
      return;
    }
    if (!measurement) {
      ctx.fillStyle = colors.inkFaint;
      ctx.font = `${Math.round(11 * this.scale)}px ui-sans-serif, system-ui, sans-serif`;
      ctx.fillText(
        this.tool === "select" ? "Select a measurement" : "Drag on screen to measure",
        left,
        (this.toolbar.top + this.toolbar.bottom) / 2,
      );
      return;
    }
    const text = primary(measurement, this.units, Math.round(96 * this.scale));
    const sub = secondary(measurement);
    ctx.fillStyle = colors.ink;
    ctx.font = `600 ${Math.round(12 * this.scale)}px ui-sans-serif, system-ui, sans-serif`;
    ctx.fillText(text, left, this.toolbar.top + (this.toolbar.bottom - this.toolbar.top) / 2 - 6 * this.scale);
    ctx.fillStyle = colors.inkFaint;
    ctx.font = `${Math.round(9 * this.scale)}px ui-sans-serif, system-ui, sans-serif`;
    ctx.fillText(sub, left, this.toolbar.top + (this.toolbar.bottom - this.toolbar.top) / 2 + 7 * this.scale);
  }

  private drawLocked(): void {
    if (!this.locked) return;
    const { ctx, colors } = this;
    const text = "Ruler locked · use Unlock below to keep measuring";
    ctx.font = `${Math.round(11 * this.scale)}px ui-sans-serif, system-ui, sans-serif`;
    const metrics = ctx.measureText(text);
    const w = metrics.width + 20 * this.scale;
    const h = Math.round(26 * this.scale);
    const x = (this.width - w) / 2;
    const y = Math.min(this.toolbar.bottom + 10 * this.scale, this.height - this.strip() - h - 8 * this.scale);
    ctx.fillStyle = withAlpha(colors.panel2, 0.97);
    this.roundRect(x, y, w, h, h / 2);
    ctx.fill();
    ctx.strokeStyle = withAlpha(colors.accent, 0.8);
    ctx.lineWidth = Math.max(1, Math.round(this.scale));
    ctx.stroke();
    ctx.fillStyle = colors.ink;
    ctx.textBaseline = "middle";
    ctx.fillText(text, x + 10 * this.scale, y + h / 2);
  }

  private fullLabel(button: Button): string {
    switch (button) {
      case "select":
        return "Select / move (V)";
      case "distance":
        return "Distance (D)";
      case "box":
        return "Box / area (B)";
      case "angle":
        return "Angle (A)";
      case "guides":
        return this.guidesEnabled ? "Guides on (G)" : "Guides off (G)";
      case "snap":
        return this.snap ? "Snap to edges on (S)" : "Snap to edges off (S)";
      case "units":
        return `Units: ${unitsLabel(this.units)} (click to cycle)`;
      case "copy":
        return "Copy measurement (C)";
      case "lock":
        return "Lock: click-through; use Unlock to return (L)";
      case "close":
        return "Close (Esc)";
    }
  }

  // --------------------------------------------------------------------- shapes

  private handle(at: Point, color: string): void {
    const { ctx, colors } = this;
    const side = Math.round(7 * this.scale);
    const half = side / 2;
    ctx.fillStyle = colors.panel;
    ctx.fillRect(at.x - half, at.y - half, side, side);
    ctx.strokeStyle = color;
    ctx.lineWidth = Math.max(1, Math.round(this.scale));
    ctx.strokeRect(at.x - half, at.y - half, side, side);
  }

  private chip(
    text: string,
    sub: string | null,
    x: number,
    y: number,
    anchor: "left" | "centre",
  ): void {
    const { ctx, colors } = this;
    const scale = this.scale;
    const font = `600 ${Math.round(11 * scale)}px ui-sans-serif, system-ui, sans-serif`;
    const subFont = `${Math.round(9 * scale)}px ui-sans-serif, system-ui, sans-serif`;
    ctx.font = font;
    const textWidth = ctx.measureText(text).width;
    ctx.font = subFont;
    const subWidth = sub ? ctx.measureText(sub).width : 0;
    const padX = Math.round(6 * scale);
    const padY = Math.round(4 * scale);
    const width = Math.max(textWidth, subWidth) + padX * 2;
    const height = Math.round(16 * scale) + (sub ? Math.round(12 * scale) : 0) + padY * 2;
    let left = anchor === "left" ? x : x - width / 2;
    left = Math.min(Math.max(left, 4 * scale), this.width - width - 4 * scale);
    let top = y;
    if (top < this.strip() + 4 * scale) top = this.strip() + 4 * scale;
    if (top + height > this.height - this.strip() - 4 * scale) {
      top = this.height - this.strip() - 4 * scale - height;
    }
    ctx.fillStyle = withAlpha(colors.panel2, 0.96);
    this.roundRect(left, top, width, height, Math.round(5 * scale));
    ctx.fill();
    ctx.strokeStyle = withAlpha(colors.edge, 0.95);
    ctx.lineWidth = Math.max(1, Math.round(scale));
    ctx.stroke();
    ctx.textBaseline = "top";
    ctx.fillStyle = colors.ink;
    ctx.font = font;
    ctx.fillText(text, left + padX, top + padY);
    if (sub) {
      ctx.fillStyle = colors.inkFaint;
      ctx.font = subFont;
      ctx.fillText(sub, left + padX, top + padY + Math.round(16 * scale));
    }
  }

  private roundRect(x: number, y: number, width: number, height: number, radius: number): void {
    const { ctx } = this;
    const r = Math.min(radius, width / 2, height / 2);
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.lineTo(x + width - r, y);
    ctx.quadraticCurveTo(x + width, y, x + width, y + r);
    ctx.lineTo(x + width, y + height - r);
    ctx.quadraticCurveTo(x + width, y + height, x + width - r, y + height);
    ctx.lineTo(x + r, y + height);
    ctx.quadraticCurveTo(x, y + height, x, y + height - r);
    ctx.lineTo(x, y + r);
    ctx.quadraticCurveTo(x, y, x + r, y);
    ctx.closePath();
  }

  private drawIcon(button: Button, rect: { left: number; top: number; right: number; bottom: number }, color: string): void {
    const { ctx } = this;
    const scale = this.scale;
    const side = Math.round(16 * scale);
    const ox = rect.left + (rect.right - rect.left - side) / 2;
    const oy = rect.top + (rect.bottom - rect.top - side) / 2;
    const p = (x: number, y: number): Point => ({ x: ox + x * scale, y: oy + y * scale });
    const width = scale >= 1.5 ? 2 : 1;
    ctx.strokeStyle = color;
    ctx.fillStyle = color;
    ctx.lineWidth = width;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";

    switch (button) {
      case "select": {
        const points = [[4, 2], [11, 9], [7, 9], [10, 13], [8, 14], [6, 10], [4, 12]];
        ctx.beginPath();
        points.forEach(([x, y], index) => {
          const at = p(x, y);
          if (index === 0) ctx.moveTo(at.x, at.y);
          else ctx.lineTo(at.x, at.y);
        });
        ctx.closePath();
        ctx.stroke();
        break;
      }
      case "distance": {
        const a = p(2, 8);
        const b = p(14, 8);
        ctx.beginPath();
        ctx.moveTo(a.x, a.y);
        ctx.lineTo(b.x, b.y);
        ctx.moveTo(p(2, 5).x, p(2, 5).y);
        ctx.lineTo(p(2, 11).x, p(2, 11).y);
        ctx.moveTo(p(14, 5).x, p(14, 5).y);
        ctx.lineTo(p(14, 11).x, p(14, 11).y);
        ctx.stroke();
        break;
      }
      case "box": {
        const left = p(3, 4);
        const right = p(13, 12);
        ctx.strokeRect(left.x, left.y, right.x - left.x, right.y - left.y);
        for (const [x, y] of [[2, 3], [12, 3], [2, 11], [12, 11]]) {
          const at = p(x, y);
          ctx.fillRect(at.x, at.y, Math.max(2, 2 * scale), Math.max(2, 2 * scale));
        }
        break;
      }
      case "angle": {
        ctx.beginPath();
        ctx.moveTo(p(2, 2).x, p(2, 2).y);
        ctx.lineTo(p(2, 14).x, p(2, 14).y);
        ctx.lineTo(p(14, 14).x, p(14, 14).y);
        ctx.stroke();
        ctx.beginPath();
        ctx.arc(p(2, 14).x, p(2, 14).y, 7 * scale, -Math.PI / 2, 0);
        ctx.stroke();
        break;
      }
      case "guides": {
        ctx.beginPath();
        for (const y of [3, 8, 13]) {
          ctx.moveTo(p(4, y).x, p(4, y).y);
          ctx.lineTo(p(12, y).x, p(12, y).y);
        }
        ctx.stroke();
        break;
      }
      case "snap": {
        ctx.beginPath();
        ctx.moveTo(p(3, 11).x, p(3, 11).y);
        ctx.lineTo(p(3, 5).x, p(3, 5).y);
        ctx.lineTo(p(8, 5).x, p(8, 5).y);
        ctx.moveTo(p(8, 13).x, p(8, 13).y);
        ctx.lineTo(p(13, 13).x, p(13, 13).y);
        ctx.lineTo(p(13, 8).x, p(13, 8).y);
        ctx.stroke();
        ctx.beginPath();
        ctx.arc(p(8, 8).x, p(8, 8).y, 1.6 * scale, 0, Math.PI * 2);
        ctx.fill();
        break;
      }
      case "units": {
        ctx.font = `600 ${Math.round(8 * scale)}px ui-sans-serif, system-ui, sans-serif`;
        ctx.textBaseline = "middle";
        const text = unitsLabel(this.units);
        const metrics = ctx.measureText(text);
        ctx.fillText(text, ox + (side - metrics.width) / 2, oy + side / 2);
        break;
      }
      case "copy": {
        ctx.strokeRect(p(5, 2).x, p(5, 2).y, 8 * scale, 8 * scale);
        ctx.strokeRect(p(2, 6).x, p(2, 6).y, 8 * scale, 8 * scale);
        break;
      }
      case "lock": {
        const y = this.locked ? 6 : 5;
        ctx.strokeRect(p(3, y).x, p(3, y).y, 10 * scale, 8 * scale);
        ctx.beginPath();
        if (this.locked) {
          ctx.arc(p(8, y).x, p(8, y).y, 3 * scale, Math.PI, 0);
        } else {
          ctx.arc(p(8, y).x + 2 * scale, p(8, y).y, 3 * scale, Math.PI, Math.PI * 1.8);
        }
        ctx.stroke();
        break;
      }
      case "close": {
        ctx.beginPath();
        ctx.moveTo(p(4, 4).x, p(4, 4).y);
        ctx.lineTo(p(12, 12).x, p(12, 12).y);
        ctx.moveTo(p(12, 4).x, p(12, 4).y);
        ctx.lineTo(p(4, 12).x, p(4, 12).y);
        ctx.stroke();
        break;
      }
    }
  }
}

/** The ruler window: one canvas, and the overlay that owns it. */
export function Ruler() {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const overlayRef = useRef<RulerOverlay | null>(null);

  useEffect(() => {
    let cancelled = false;
    const { body, documentElement } = document;
    const previous = [body.style.background, documentElement.style.background];
    body.style.background = "transparent";
    documentElement.style.background = "transparent";

    const unlisten = listen<boolean>("ruler-lock", (event) => {
      overlayRef.current?.setLocked(event.payload);
      // Unlocking has to take the keyboard back, or the first key after it goes
      // to whatever had focus instead of the ruler.
      if (!event.payload) {
        void getCurrentWindow()
          .setFocus()
          .catch(() => undefined);
      }
    });

    rulerContext()
      .then((context) => {
        if (cancelled || !canvasRef.current) return;
        const overlay = new RulerOverlay(canvasRef.current, context);
        overlayRef.current = overlay;
        overlay.mount();
      })
      .catch((error) => {
        console.error("the ruler could not start", errorMessage(error));
        void rulerOverlayClose();
      });

    return () => {
      cancelled = true;
      void unlisten.then((off) => off());
      overlayRef.current?.destroy();
      overlayRef.current = null;
      body.style.background = previous[0];
      documentElement.style.background = previous[1];
    };
  }, []);

  return <canvas ref={canvasRef} className="fixed inset-0 block select-none" />;
}
