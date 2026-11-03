/**
 * Drawings on a chart: trend lines, horizontal lines, Fibonacci retracements,
 * rectangles and notes, anchored to data. An anchor holds an x value of the x
 * column, exactly as the frame's points carry it, and a value on the price's
 * axis, so a drawing stays with the data however the chart is zoomed. The engine
 * says where each anchored x value sits on the axis, as only it can on trading
 * time; an anchor placed since the last frame is found among the frame's points,
 * which it was snapped to.
 *
 * Everything here is worked out without the DOM, so that tests can check it.
 */

import { canonicalJson } from "../chart-frame";
import type { ChartFrameData } from "../chart-frame";
import type { LayoutAnchor, LayoutDrawing } from "../layout";
import type { ChartModel, Rect } from "./model";
import type { Scale } from "./scale";

/** A kind of drawing, as a tool that makes one. */
export type DrawingTool = LayoutDrawing["kind"];

/** A point in CSS pixels. */
export interface Point {
  readonly x: number;
  readonly y: number;
}

/** The levels of a Fibonacci retracement, as shares of the move from its end back to its start, and their labels. */
export const FIBONACCI: readonly (readonly [number, string])[] = [
  [0, "0%"],
  [0.236, "23.6%"],
  [0.382, "38.2%"],
  [0.5, "50%"],
  [0.618, "61.8%"],
  [0.786, "78.6%"],
  [1, "100%"]
];

/** A drawing as drawn, by its place among the layout's drawings. */
export type Shape =
  | { readonly kind: "trend" | "rectangle"; readonly index: number; readonly a: Point; readonly b: Point }
  | { readonly kind: "horizontal"; readonly index: number; readonly y: number; readonly left: number; readonly right: number }
  | { readonly kind: "fibonacci"; readonly index: number; readonly a: Point; readonly b: Point; readonly levels: readonly { readonly y: number; readonly label: string }[] }
  | { readonly kind: "note"; readonly index: number; readonly at: Point; readonly text: string; readonly box: Rect };

/** An anchor snapped to the data, and where it is drawn. */
export interface Snap {
  readonly anchor: LayoutAnchor;
  readonly point: Point;
}

/** How far from a drawing, in pixels, the pointer still picks it. */
export const SLACK = 6;

/** The price's scale: the left axis of the plot. */
function priceScale(model: ChartModel): Scale | null {
  return model.yAxes.find((a) => a.side === "left")?.scale ?? null;
}

/** Where an anchored x value sits on the axis: from the engine, or among the frame's points for one just placed. */
export function anchorPosition(frame: ChartFrameData, x: LayoutAnchor["x"]): number | null {
  const json = JSON.stringify(x);
  const placed = frame.anchors.get(json);
  if (placed !== undefined) {
    return Number.isFinite(placed) ? placed : null;
  }
  const content = frame.content;
  // The frame writes numbers as the engine does, `2.0` where JavaScript writes `2`: compare them as values.
  const same = (text: string | null): boolean => text !== null && (text === json || canonicalJson(text) === json);
  if (content.kind === "candles") {
    const i = content.candles.xJson.findIndex(same);
    return i < 0 ? null : (content.candles.from[i] ?? null);
  }
  if (content.kind === "series") {
    for (const s of content.series) {
      const i = s.xJson.findIndex(same);
      if (i >= 0) {
        return s.x[i] ?? null;
      }
    }
  }
  return null;
}

/** An anchor in pixels, or null if the axis cannot place it or the scale cannot, as for a price at or below zero on a log scale. */
function pointOf(frame: ChartFrameData, x: Scale, y: Scale, anchor: LayoutAnchor): Point | null {
  const position = anchorPosition(frame, anchor.x);
  if (position === null || (y.log && anchor.y <= 0)) {
    return null;
  }
  return { x: x.map(position), y: y.map(anchor.y) };
}

/** The drawings as drawn on a chart's price plot. A drawing whose anchors cannot be placed is left out. */
export function shapesOf(drawings: readonly LayoutDrawing[], frame: ChartFrameData, model: ChartModel, measure: (text: string) => number): Shape[] {
  const [x, y] = [model.x, priceScale(model)];
  if (x === null || y === null) {
    return [];
  }
  return drawings.flatMap((d, index): Shape[] => {
    const shape = shapeOf(d, index, frame, x, y, model.plot, measure);
    return shape === null ? [] : [shape];
  });
}

function shapeOf(d: LayoutDrawing, index: number, frame: ChartFrameData, x: Scale, y: Scale, plot: Rect, measure: (text: string) => number): Shape | null {
  if (d.kind === "horizontal") {
    return y.log && d.y <= 0 ? null : { kind: "horizontal", index, y: y.map(d.y), left: plot.x, right: plot.x + plot.w };
  }
  if (d.kind === "note") {
    const at = pointOf(frame, x, y, d.at);
    return at === null ? null : { kind: "note", index, at, text: d.text, box: noteBox(at, d.text, plot, measure) };
  }
  const [a, b] = [pointOf(frame, x, y, d.from), pointOf(frame, x, y, d.to)];
  if (a === null || b === null) {
    return null;
  }
  if (d.kind === "fibonacci") {
    // Level 0 sits at the move's end and 100% back at its start.
    const levels = FIBONACCI.map(([share, label]) => ({ y: y.map(d.to.y + (d.from.y - d.to.y) * share), label }));
    return { kind: "fibonacci", index, a, b, levels };
  }
  return { kind: d.kind, index, a, b };
}

/** A note's box: to the right of its point, or to the left near the plot's right edge. */
function noteBox(at: Point, text: string, plot: Rect, measure: (text: string) => number): Rect {
  const w = Math.ceil(measure(text)) + 12;
  const h = 20;
  const left = at.x + 8 + w > plot.x + plot.w ? at.x - 8 - w : at.x + 8;
  return { x: left, y: at.y - h / 2, w, h };
}

/** The distance from `p` to the segment from `a` to `b`. */
function toSegment(p: Point, a: Point, b: Point): number {
  const [dx, dy] = [b.x - a.x, b.y - a.y];
  const length = dx * dx + dy * dy;
  const t = length === 0 ? 0 : Math.max(0, Math.min(1, ((p.x - a.x) * dx + (p.y - a.y) * dy) / length));
  return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

function inside(p: Point, r: Rect): boolean {
  return p.x >= r.x && p.x <= r.x + r.w && p.y >= r.y && p.y <= r.y + r.h;
}

/** How far `p` is from a drawing: its lines, or a rectangle's or note's inside. */
function distance(shape: Shape, p: Point): number {
  switch (shape.kind) {
    case "trend":
      return toSegment(p, shape.a, shape.b);
    case "horizontal":
      return p.x >= shape.left && p.x <= shape.right ? Math.abs(p.y - shape.y) : Number.POSITIVE_INFINITY;
    case "fibonacci":
      return Math.min(...shape.levels.map((l) => toSegment(p, { x: shape.a.x, y: l.y }, { x: shape.b.x, y: l.y })));
    case "rectangle": {
      const box = { x: Math.min(shape.a.x, shape.b.x), y: Math.min(shape.a.y, shape.b.y), w: Math.abs(shape.b.x - shape.a.x), h: Math.abs(shape.b.y - shape.a.y) };
      return inside(p, box) ? 0 : Math.min(...edges(box).map(([a, b]) => toSegment(p, a, b)));
    }
    case "note":
      return inside(p, shape.box) ? 0 : Math.hypot(p.x - shape.at.x, p.y - shape.at.y);
  }
}

function edges(r: Rect): readonly (readonly [Point, Point])[] {
  const [tl, tr, br, bl] = [
    { x: r.x, y: r.y },
    { x: r.x + r.w, y: r.y },
    { x: r.x + r.w, y: r.y + r.h },
    { x: r.x, y: r.y + r.h }
  ];
  return [
    [tl, tr],
    [tr, br],
    [br, bl],
    [bl, tl]
  ];
}

/** The drawing nearest `p` within a few pixels, by its place among the layout's drawings, or null. */
export function drawingAt(shapes: readonly Shape[], p: Point): number | null {
  let best: { readonly index: number; readonly distance: number } | null = null;
  for (const shape of shapes) {
    const d = distance(shape, p);
    if (d <= SLACK && (best === null || d < best.distance)) {
      best = { index: shape.index, distance: d };
    }
  }
  return best?.index ?? null;
}

/** A drawing's handles: the points its anchors are drawn at. A horizontal line's is where the pointer meets it. */
export function handles(shape: Shape): readonly Point[] {
  switch (shape.kind) {
    case "trend":
    case "rectangle":
    case "fibonacci":
      return [shape.a, shape.b];
    case "horizontal":
      return [];
    case "note":
      return [shape.at];
  }
}

/** The handle of a drawing under `p`, by its anchor's place, or null. */
export function handleAt(shape: Shape, p: Point): number | null {
  const i = handles(shape).findIndex((h) => Math.hypot(h.x - p.x, h.y - p.y) <= SLACK);
  return i < 0 ? null : i;
}

/** `value` rounded to what one pixel of the scale can show: no more digits than a pixel tells apart. */
function rounded(value: number, perPixel: number): number {
  if (!(perPixel > 0) || !Number.isFinite(value)) {
    return value;
  }
  const decimals = Math.max(0, Math.min(12, -Math.floor(Math.log10(perPixel))));
  return Number(value.toFixed(decimals));
}

/** A JSON scalar of an x value, or null for anything else. */
function scalarOf(json: string): LayoutAnchor["x"] | null {
  const value: unknown = JSON.parse(json);
  return typeof value === "string" || typeof value === "number" || typeof value === "boolean" ? value : null;
}

/** The frame's point or candle nearest axis position `at`, among series not hidden: its position and exact x value. */
function nearestData(frame: ChartFrameData, at: number, hidden: readonly string[]): { readonly position: number; readonly json: string } | null {
  const content = frame.content;
  const candidates: { readonly position: number; readonly json: string }[] = [];
  if (content.kind === "candles") {
    const c = content.candles;
    c.from.forEach((position, i) => {
      const json = c.xJson[i];
      if (json !== null && json !== undefined) {
        candidates.push({ position, json });
      }
    });
  } else if (content.kind === "series") {
    for (const s of content.series.filter((series) => !hidden.includes(series.label))) {
      s.x.forEach((position, i) => {
        const json = s.xJson[i];
        if (json !== null && json !== undefined && s.yText[i] !== null) {
          candidates.push({ position, json });
        }
      });
    }
  }
  let best: (typeof candidates)[number] | null = null;
  for (const c of candidates) {
    if (best === null || Math.abs(c.position - at) < Math.abs(best.position - at)) {
      best = c;
    }
  }
  return best;
}

/**
 * An anchor at the pointer: at the x of the frame's nearest point or candle, exactly as the frame carries it, and at
 * the pointer's price, rounded to what a pixel tells apart. Null where there is no data to anchor to.
 */
export function snap(frame: ChartFrameData, model: ChartModel, p: Point, hidden: readonly string[] = []): Snap | null {
  const [x, y] = [model.x, priceScale(model)];
  if (x === null || y === null) {
    return null;
  }
  const near = nearestData(frame, x.invert(p.x), hidden);
  const value = near === null ? null : scalarOf(near.json);
  if (near === null || value === null) {
    return null;
  }
  const price = rounded(y.invert(p.y), Math.abs(y.invert(p.y) - y.invert(p.y + 1)));
  return { anchor: { x: value, y: price }, point: { x: x.map(near.position), y: y.map(price) } };
}

/** A drawing made with a tool from one anchor to another, or at one, with a note's text. */
export function drawingFrom(tool: DrawingTool, from: Snap, to: Snap, text = ""): LayoutDrawing {
  switch (tool) {
    case "horizontal":
      return { kind: "horizontal", y: from.anchor.y };
    case "note":
      return { kind: "note", at: from.anchor, text };
    case "trend":
    case "fibonacci":
    case "rectangle":
      return { kind: tool, from: from.anchor, to: to.anchor };
  }
}

/** A drawing with one anchor moved: by its place among the drawing's anchors, as `handles` gives them. */
export function moved(d: LayoutDrawing, anchor: number, to: Snap): LayoutDrawing {
  switch (d.kind) {
    case "horizontal":
      return { ...d, y: to.anchor.y };
    case "note":
      return { ...d, at: to.anchor };
    case "trend":
    case "fibonacci":
    case "rectangle":
      return anchor === 0 ? { ...d, from: to.anchor } : { ...d, to: to.anchor };
  }
}
