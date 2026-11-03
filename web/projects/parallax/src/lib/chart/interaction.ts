/**
 * Chart interaction, worked out without the DOM so that tests can check it: what
 * the crosshair reads at a position, which mark the pointer is on, how zooming
 * and panning move the x range, and the filters a brushed range selects.
 */

import type { CandleData, ChartFrameData, IndicatorLineData, SeriesData } from "../chart-frame";
import type { LayoutFilter, LayoutScalar } from "../layout";
import type { IndicatorStyle } from "./indicators";
import type { ChartModel, Hit, Mark } from "./model";
import type { Scale } from "./scale";

/** A row of a tooltip: the value leads, the label follows, the series' colour keys it. */
export interface TipRow {
  readonly value: string;
  readonly label: string;
  readonly color: string | null;
}

/** What the crosshair or the pointer reads. */
export interface Reading {
  readonly title: string;
  readonly rows: readonly TipRow[];
  /** The x the crosshair snapped to, in pixels, or null for a mark without one. */
  readonly crosshair: number | null;
}

/** The index of the value in sorted `xs` nearest to `x`, or -1 if there is none. */
export function nearestIndex(xs: Float64Array, x: number): number {
  if (xs.length === 0) {
    return -1;
  }
  let [low, high] = [0, xs.length - 1];
  while (low < high) {
    const mid = (low + high) >> 1;
    if ((xs[mid] ?? 0) < x) {
      low = mid + 1;
    } else {
      high = mid;
    }
  }
  const before = low - 1;
  return before >= 0 && Math.abs((xs[before] ?? 0) - x) <= Math.abs((xs[low] ?? 0) - x) ? before : low;
}

/** A line with text per point: a series or an indicator's line. */
interface Points {
  readonly x: Float64Array;
  readonly yText: readonly (string | null)[];
}

/** Whether point `i` of `s` is drawn, rather than a break between segments. */
function drawn(s: Points, i: number): boolean {
  return (s.yText[i] ?? null) !== null;
}

/** The index of the drawn point of `s` nearest `x`, skipping breaks, or -1 if there is none. */
export function nearestDrawn(s: Points, x: number): number {
  const i = nearestIndex(s.x, x);
  if (i < 0) {
    return -1;
  }
  // The nearest drawn point is the first one met going left from the nearest point, or going right.
  let left = i;
  while (left >= 0 && !drawn(s, left)) {
    left -= 1;
  }
  let right = i;
  while (right < s.x.length && !drawn(s, right)) {
    right += 1;
  }
  if (right >= s.x.length) {
    return left;
  }
  return left >= 0 && Math.abs((s.x[left] ?? 0) - x) <= Math.abs((s.x[right] ?? 0) - x) ? left : right;
}

/** How far from the crosshair, in pixels, another series' point may be and still be read. */
const NEAR = 6;

/**
 * What the crosshair reads at axis position `x` on a line or area chart: it snaps to the nearest point of any shown
 * series, then lists every shown series' point nearest it, so the pointer never has to land on a line. Breaks between
 * segments are never snapped to or read. The engine
 * keeps different points of each series when it reduces them to the pixels, so a series' nearest point may be of
 * another x than the crosshair's; its row then names that x too, and a series with no point within a few pixels, or
 * a gap there, says nothing.
 */
export function readSeries(frame: ChartFrameData, model: ChartModel, hidden: readonly string[], x: number): Reading | null {
  const content = frame.content;
  const scale = model.x;
  if (content.kind !== "series" || scale === null) {
    return null;
  }
  const shown = content.series.map((s, i) => ({ s, color: model.legend[i]?.color ?? null })).filter(({ s }) => !hidden.includes(s.label) && s.x.length > 0);
  let snapped: number | null = null;
  let title = "";
  for (const { s } of shown) {
    const i = nearestDrawn(s, x);
    const at = s.x[i];
    if (at !== undefined && (snapped === null || Math.abs(at - x) < Math.abs(snapped - x))) {
      snapped = at;
      title = s.xText[i] ?? "";
    }
  }
  if (snapped === null) {
    return null;
  }
  const at = snapped;
  const lone = model.marks.find((m) => m.kind === "line")?.color ?? null;
  const rows = shown.flatMap(({ s, color }): TipRow[] => {
    const i = nearestDrawn(s, at);
    const [near, value] = [s.x[i], s.yText[i]];
    if (near === undefined || value === null || value === undefined || Math.abs(scale.map(near) - scale.map(at)) > NEAR) {
      return [];
    }
    return [{ value, label: near === at ? s.label : `${s.label}, ${s.xText[i] ?? ""}`, color: color ?? lone }];
  });
  return { title, rows, crosshair: scale.map(at) };
}

function rows(hit: Hit, color: string | null): readonly TipRow[] {
  return hit.rows.map((r) => ({ value: r.value, label: r.label, color }));
}

/** How far the pointer is from a mark, or null if it is outside the mark's hit area, a few pixels larger than the mark. */
function distanceTo(mark: Mark, px: number, py: number): number | null {
  const slack = 4;
  switch (mark.kind) {
    case "candle": {
      const distance = Math.abs(mark.x - px);
      return distance <= mark.width / 2 + slack ? distance : null;
    }
    case "rect":
    case "cell": {
      const r = mark.rect;
      const inside = px >= r.x - slack && px <= r.x + r.w + slack && py >= r.y - slack && py <= r.y + r.h + slack;
      return inside ? Math.hypot(px - (r.x + r.w / 2), py - (r.y + r.h / 2)) : null;
    }
    case "line":
    case "band":
    case "columns":
      return null;
  }
}

/** What a mark's tooltip reads. */
function readMark(mark: Mark): Reading | null {
  switch (mark.kind) {
    case "candle":
      return { title: mark.hit.title, rows: rows(mark.hit, null), crosshair: mark.x };
    case "rect":
      return { title: mark.hit.title, rows: rows(mark.hit, mark.color), crosshair: null };
    case "cell":
      return mark.hit === null ? null : { title: mark.hit.title, rows: rows(mark.hit, mark.color), crosshair: null };
    case "line":
    case "band":
    case "columns":
      return null;
  }
}

/** The mark nearest the pointer whose hit area holds it, and what it reads. */
export function markAt(model: ChartModel, px: number, py: number): Reading | null {
  let best: { readonly mark: Mark; readonly distance: number } | null = null;
  for (const mark of model.marks) {
    // A density's cells read nothing of their own.
    if (mark.kind === "cell" && mark.hit === null) {
      continue;
    }
    const distance = distanceTo(mark, px, py);
    if (distance !== null && (best === null || distance < best.distance)) {
      best = { mark, distance };
    }
  }
  return best === null ? null : readMark(best.mark);
}

/**
 * Each indicator line's value at axis position `x`: its drawn point nearest `x`, within a few pixels, under its own x
 * when that differs from the crosshair's, as for series. Hidden overlays read nothing.
 */
export function indicatorRows(frame: ChartFrameData, model: ChartModel, hidden: readonly string[], x: number): TipRow[] {
  const scale = model.x;
  if (scale === null) {
    return [];
  }
  // On a candle, each indicator reads the last row inside the candle's bucket, the one its close comes from.
  const bucket = bucketAt(frame, x);
  return frame.indicators.flatMap((line, i): TipRow[] => {
    const style = model.indicators[i];
    if (style === undefined || hidden.includes(style.group) || hidden.includes(style.follows)) {
      return [];
    }
    return bucket === null ? readNear(line, style, scale, x) : readBucket(line, style, bucket);
  });
}

/** An indicator line's value in a candle's bucket, at its last row, under that row's date. */
function readBucket(line: IndicatorLineData, style: IndicatorStyle, bucket: Range): TipRow[] {
  const at = lastDrawnIn(line, bucket);
  const [near, value] = [line.x[at], line.yText[at]];
  return near === undefined || value === null || value === undefined ? [] : [{ value, label: `${style.label}, ${line.xText[at] ?? ""}`, color: style.color }];
}

/** An indicator line's value at its drawn point nearest `x`, within a few pixels, under its own x if that differs. */
function readNear(line: IndicatorLineData, style: IndicatorStyle, scale: Scale, x: number): TipRow[] {
  const at = nearestDrawn(line, x);
  const [near, value] = [line.x[at], line.yText[at]];
  if (near === undefined || value === null || value === undefined || Math.abs(scale.map(near) - scale.map(x)) > NEAR) {
    return [];
  }
  const label = Math.abs(scale.map(near) - scale.map(x)) < 0.5 ? style.label : `${style.label}, ${line.xText[at] ?? ""}`;
  return [{ value, label, color: style.color }];
}

/** The bucket holding axis position `x`, as its start and the next one's, or null outside every bucket or without candles. */
function bucketAt(frame: ChartFrameData, x: number): Range | null {
  if (frame.content.kind !== "candles") {
    return null;
  }
  const c = frame.content.candles;
  // The last bucket starting at or before `x`, if `x` is before its end.
  let [low, high] = [0, c.from.length];
  while (low < high) {
    const mid = (low + high) >> 1;
    if ((c.from[mid] ?? 0) <= x) {
      low = mid + 1;
    } else {
      high = mid;
    }
  }
  const [from, to] = [c.from[low - 1], c.to[low - 1]];
  return from === undefined || to === undefined || x >= to ? null : [from, to];
}

/** The index of the last drawn point of `line` inside `[from, to)`, or -1 if none is. */
function lastDrawnIn(line: Points, [from, to]: Range): number {
  // The first point at or past `to`, by bisection; the one before it is the last inside.
  let [low, high] = [0, line.x.length];
  while (low < high) {
    const mid = (low + high) >> 1;
    if ((line.x[mid] ?? 0) < to) {
      low = mid + 1;
    } else {
      high = mid;
    }
  }
  let i = low - 1;
  while (i >= 0 && !drawn(line, i)) {
    i -= 1;
  }
  return i >= 0 && (line.x[i] ?? Number.NaN) >= from ? i : -1;
}

/** An x range in axis positions. */
export type Range = readonly [number, number];

/** The smallest share of the full range a zoom may reach. */
const MIN_SHARE = 1e-6;

/** Keeps `range` within `full`, at least its smallest share wide, moving it rather than shrinking it at the edges. */
function clamp(range: Range, full: Range | null): Range {
  if (full === null) {
    return range;
  }
  const span = Math.min(full[1] - full[0], Math.max(range[1] - range[0], (full[1] - full[0]) * MIN_SHARE));
  const from = Math.min(Math.max(range[0], full[0]), full[1] - span);
  return [from, from + span];
}

/** Zooms `range` by `factor` around `anchor`: below 1 zooms in, above 1 out. Null when it reaches the full range. */
export function zoomRange(range: Range, anchor: number, factor: number, full: Range | null): Range | null {
  const next = clamp([anchor - (anchor - range[0]) * factor, anchor + (range[1] - anchor) * factor], full);
  return full !== null && next[0] <= full[0] && next[1] >= full[1] ? null : next;
}

/** Moves `range` by `delta` axis units, staying within the full range. */
export function panRange(range: Range, delta: number, full: Range | null): Range {
  return clamp([range[0] + delta, range[1] + delta], full);
}

function scalar(json: string | null | undefined): LayoutScalar | undefined {
  if (json === null || json === undefined) {
    return undefined;
  }
  const value: unknown = JSON.parse(json);
  return typeof value === "string" || typeof value === "number" || typeof value === "boolean" ? value : undefined;
}

/** A point or bucket a brush covers: where it starts and ends on the axis, and those bounds' exact values. */
interface Span {
  readonly from: number;
  readonly to: number;
  readonly fromJson: string | null;
  readonly toJson: string | null;
}

/** The drawn points of the shown series inside `[from, to]`. */
function pointsIn(series: readonly SeriesData[], hidden: readonly string[], from: number, to: number): Span[] {
  const spans: Span[] = [];
  for (const s of series.filter((s) => !hidden.includes(s.label))) {
    s.x.forEach((x, i) => {
      if (x >= from && x <= to && drawn(s, i)) {
        const json = s.xJson[i] ?? null;
        spans.push({ from: x, to: x, fromJson: json, toJson: json });
      }
    });
  }
  return spans;
}

/** The buckets that overlap `[from, to]`, each whole: from its start up to the next bucket's start. */
function bucketsIn(c: CandleData, from: number, to: number): Span[] {
  const spans: Span[] = [];
  c.from.forEach((start, i) => {
    const end = c.to[i] ?? start;
    if (start <= to && end > from) {
      spans.push({ from: start, to: end, fromJson: c.xJson[i] ?? null, toJson: c.endJson[i] ?? null });
    }
  });
  return spans;
}

/** A brushed range's selection: the filters that select it, and the x range it covers, which the band snaps to. */
export interface Brushed {
  readonly filters: readonly LayoutFilter[];
  readonly range: Range;
}

/**
 * What a brushed x range selects: on a line or area chart, from the first drawn point inside it to the last; on candles,
 * every bucket it touches, whole. Each bound is the exact value the engine sent, so a linked view's rows are those the
 * chart drew there, and the band snaps to the selection so that it shows exactly what was selected. Null if nothing is
 * selected.
 */
export function brushFilters(frame: ChartFrameData, column: string, hidden: readonly string[], range: Range): Brushed | null {
  const content = frame.content;
  const [from, to] = range[0] <= range[1] ? range : [range[1], range[0]];
  let spans: Span[] = [];
  if (content.kind === "series") {
    spans = pointsIn(content.series, hidden, from, to);
  } else if (content.kind === "candles") {
    spans = bucketsIn(content.candles, from, to);
  }
  if (spans.length === 0) {
    return null;
  }
  const first = spans.reduce((a, b) => (b.from < a.from ? b : a));
  const last = spans.reduce((a, b) => (b.to > a.to ? b : a));
  const [low, high] = [scalar(first.fromJson), scalar(last.toJson)];
  if (low === undefined || high === undefined) {
    return null;
  }
  // A bucket stops short of the next one's start; a point includes itself.
  const op = content.kind === "candles" ? "lt" : "le";
  return {
    filters: [
      { column, op: "ge", value: low, values: [] },
      { column, op, value: high, values: [] }
    ],
    range: [first.from, last.to]
  };
}
