/**
 * Chart interaction, worked out without the DOM so that tests can check it: what
 * the crosshair reads at a position, which mark the pointer is on, how zooming
 * and panning move the x range, and the filters a brushed range selects.
 */

import type { ChartFrameData } from "../chart-frame";
import type { LayoutFilter, LayoutScalar } from "../layout";
import type { ChartModel, Hit, Mark } from "./model";

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

/** How far from the crosshair, in pixels, another series' point may be and still be read. */
const NEAR = 6;

/**
 * What the crosshair reads at axis position `x` on a line or area chart: it snaps to the nearest point of any shown
 * series, then lists every shown series' point nearest it, so the pointer never has to land on a line. The engine
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
    const i = nearestIndex(s.x, x);
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
    const i = nearestIndex(s.x, at);
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

/**
 * The filters that select the data a brushed x range covers: from the first point or bucket inside it to the last,
 * with each bound the exact value the engine sent, so a linked view's rows are the chart's. Null if nothing is
 * inside.
 */
export function brushFilters(frame: ChartFrameData, column: string, hidden: readonly string[], range: Range): readonly LayoutFilter[] | null {
  const content = frame.content;
  const [from, to] = range[0] <= range[1] ? range : [range[1], range[0]];
  // The first and last point or bucket inside the range, with the values that bound it.
  let first = Number.POSITIVE_INFINITY;
  let last = Number.NEGATIVE_INFINITY;
  let start: string | null = null;
  let end: string | null = null;
  const consider = (x: number, low: string | null, high: string | null): void => {
    if (x < from || x > to) {
      return;
    }
    if (x < first) {
      first = x;
      start = low;
    }
    if (x > last) {
      last = x;
      end = high;
    }
  };
  if (content.kind === "series") {
    for (const s of content.series) {
      if (!hidden.includes(s.label)) {
        s.x.forEach((x, i) => {
          consider(x, s.xJson[i] ?? null, s.xJson[i] ?? null);
        });
      }
    }
  } else if (content.kind === "candles") {
    const c = content.candles;
    c.from.forEach((x, i) => {
      consider(x, c.xJson[i] ?? null, c.endJson[i] ?? null);
    });
  }
  const [low, high] = [scalar(start), scalar(end)];
  if (low === undefined || high === undefined) {
    return null;
  }
  // A bucket stops short of the next one's start; a point includes itself.
  const op = content.kind === "candles" ? "lt" : "le";
  return [
    { column, op: "ge", value: low, values: [] },
    { column, op, value: high, values: [] }
  ];
}
