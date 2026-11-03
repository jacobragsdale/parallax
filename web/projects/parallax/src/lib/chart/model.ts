/**
 * The draw model of a chart: what to paint and where, worked out from the data
 * without touching a canvas, so that tests can check it.
 *
 * Marks follow fixed specs: 2 px lines with an area wash at 10 % opacity, bars
 * and columns at most 24 px thick with 4 px rounded data ends and square
 * baselines, a 2 px surface gap between touching bars and cells, and hairline
 * gridlines. Series take the categorical colours in a fixed order by their place
 * in the data, never cycled, so hiding one never repaints the others; series past
 * the eighth are drawn in the muted colour.
 */

import type { ChartContent, ChartFrameData } from "../chart-frame";
import type { LayoutChart } from "../layout";
import type { ChartColors } from "../theme";
import { linearScale, logDomain, logScale, niceDomain, valueTicks } from "./scale";
import type { Scale, ValueTick } from "./scale";

/** A rectangle in CSS pixels. */
export interface Rect {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
}

/** What a tooltip shows for a mark: values first, labels second. */
export interface Hit {
  readonly title: string;
  readonly rows: readonly { readonly label: string; readonly value: string }[];
}

/** One painted shape. */
export type Mark =
  | { readonly kind: "line"; readonly color: string; readonly segments: readonly Float64Array[]; readonly baseline: number | null }
  | { readonly kind: "rect"; readonly color: string; readonly rect: Rect; readonly round: "top" | "bottom" | "left" | "right" | "none"; readonly hit: Hit }
  | { readonly kind: "cell"; readonly color: string; readonly rect: Rect; readonly hit: Hit | null }
  | {
      readonly kind: "candle";
      readonly color: string;
      readonly hollow: boolean;
      readonly bars: boolean;
      readonly x: number;
      readonly width: number;
      readonly open: number;
      readonly close: number;
      readonly high: number;
      readonly low: number;
      readonly hit: Hit;
    };

/** A legend entry: the same shape as the series' mark. */
export interface LegendEntry {
  readonly label: string;
  readonly color: string;
  readonly shape: "line" | "rect";
  readonly hidden: boolean;
}

/** A value axis. */
export interface ValueAxis {
  readonly side: "left" | "right";
  readonly scale: Scale;
  readonly ticks: readonly ValueTick[];
}

/** A labelled tick on the category or x axis, in pixels. */
export interface AxisTick {
  readonly px: number;
  readonly label: string;
}

/** Everything a chart paints. */
export interface ChartModel {
  readonly width: number;
  readonly height: number;
  readonly plot: Rect;
  readonly xTicks: readonly AxisTick[];
  /** Labels down the left side, for a heatmap's rows or a horizontal bar chart's categories. */
  readonly yLabels: readonly AxisTick[];
  readonly yAxes: readonly ValueAxis[];
  /** The x scale of a chart the engine reduced, in axis positions, for zooming and the crosshair. */
  readonly x: Scale | null;
  readonly marks: readonly Mark[];
  readonly legend: readonly LegendEntry[];
  /** Said instead of a plot when there is nothing to draw. */
  readonly empty: string | null;
}

/** Data of a bar chart or heatmap, taken from the layout's grouped view. */
export interface CategoryData {
  readonly categories: readonly string[];
  readonly series: readonly { readonly label: string; readonly values: readonly number[]; readonly texts: readonly string[] }[];
}

/** What the model is built from. */
export interface ModelInput {
  readonly chart: LayoutChart;
  readonly width: number;
  readonly height: number;
  readonly colors: ChartColors;
  /** The colour of series past the eighth. */
  readonly muted: string;
  /** Measures a label's width in the chart font. */
  readonly measure: (text: string) => number;
  readonly frame: ChartFrameData | null;
  readonly categories: CategoryData | null;
}

const COUNT = new Intl.NumberFormat("en-US");

const PAD = 8;
const X_AXIS = 22;
const MAX_BAR = 24;
const GAP = 2;
const TICKS_PER_PX = 1 / 56;

function seriesColor(input: ModelInput, index: number): string {
  return input.colors.series[index] ?? input.muted;
}

function hidden(input: ModelInput, label: string): boolean {
  return input.chart.hidden.includes(label);
}

/** A legend, when there is more than one series to tell apart. */
function legendOf(input: ModelInput, labels: readonly string[], shape: "line" | "rect"): readonly LegendEntry[] {
  if (labels.length < 2) {
    return [];
  }
  return labels.map((label, i) => ({ label, color: seriesColor(input, i), shape, hidden: hidden(input, label) }));
}

function extent(values: Iterable<number>, positive: boolean): readonly [number, number] | null {
  let [low, high] = [Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY];
  for (const v of values) {
    if (Number.isFinite(v) && (!positive || v > 0)) {
      low = Math.min(low, v);
      high = Math.max(high, v);
    }
  }
  return low <= high ? [low, high] : null;
}

/** A value scale over `values` for the plot's height, logarithmic if asked and possible. */
function valueScale(input: ModelInput, values: readonly number[], range: readonly [number, number], zero: boolean): Scale {
  const count = Math.max(2, Math.abs(range[0] - range[1]) * TICKS_PER_PX);
  if (input.chart.log_y) {
    const positive = extent(values, true);
    if (positive !== null) {
      return logScale(logDomain(positive[0], positive[1]), range);
    }
  }
  const all = extent(values, false) ?? [0, 1];
  return linearScale(niceDomain(all[0], all[1], count, zero), range);
}

function axis(side: "left" | "right", scale: Scale): ValueAxis {
  return { side, scale, ticks: valueTicks(scale, Math.max(2, Math.abs(scale.range[0] - scale.range[1]) * TICKS_PER_PX)) };
}

function gutter(input: ModelInput, ticks: readonly ValueTick[]): number {
  return Math.ceil(Math.max(0, ...ticks.map((t) => input.measure(t.label)))) + PAD + 2;
}

/** The plot area once the value axes' labels have room. */
function plotFor(input: ModelInput, left: readonly ValueTick[], right: readonly ValueTick[] | null): Rect {
  const x = gutter(input, left);
  const w = input.width - x - (right === null ? PAD : gutter(input, right));
  return { x, y: PAD, w: Math.max(1, w), h: Math.max(1, input.height - PAD - X_AXIS) };
}

/** Re-ranges a scale onto a new pixel range, keeping its domain. */
function onto(scale: Scale, range: readonly [number, number]): Scale {
  return scale.log ? logScale(scale.domain, range) : linearScale(scale.domain, range);
}

function emptyModel(input: ModelInput, message: string): ChartModel {
  const plot = { x: PAD, y: PAD, w: Math.max(1, input.width - 2 * PAD), h: Math.max(1, input.height - PAD - X_AXIS) };
  return { width: input.width, height: input.height, plot, xTicks: [], yLabels: [], yAxes: [], x: null, marks: [], legend: [], empty: message };
}

type Content<K extends ChartContent["kind"]> = Extract<ChartContent, { kind: K }>;

function lineModel(input: ModelInput, frame: ChartFrameData, content: Content<"series">): ChartModel {
  const visible = content.series.map((s, i) => ({ s, i })).filter(({ s }) => !hidden(input, s.label));
  const values = (right: boolean): number[] => visible.filter(({ s }) => s.right === right).flatMap(({ s }) => Array.from(s.y));
  const area = input.chart.kind === "area";
  const range: readonly [number, number] = [input.height - X_AXIS, PAD];
  const leftScale = valueScale(input, values(false), range, area);
  const hasRight = visible.some(({ s }) => s.right);
  const rightScale = hasRight ? valueScale(input, values(true), range, area) : null;
  const plot = plotFor(input, axis("left", leftScale).ticks, rightScale === null ? null : axis("right", rightScale).ticks);
  const yRange: readonly [number, number] = [plot.y + plot.h, plot.y];
  const left = onto(leftScale, yRange);
  const right = rightScale === null ? null : onto(rightScale, yRange);
  const x = linearScale([frame.xFrom, frame.xTo], [plot.x, plot.x + plot.w]);
  const marks: Mark[] = visible.flatMap(({ s, i }): Mark[] => {
    const y = s.right && right !== null ? right : left;
    const segments: Float64Array[] = [];
    let current: number[] = [];
    for (let p = 0; p < s.x.length; p += 1) {
      const value = s.y[p] ?? Number.NaN;
      if (Number.isNaN(value) || (y.log && value <= 0)) {
        if (current.length > 0) {
          segments.push(Float64Array.from(current));
        }
        current = [];
        continue;
      }
      current.push(x.map(s.x[p] ?? 0), y.map(value));
    }
    if (current.length > 0) {
      segments.push(Float64Array.from(current));
    }
    const baseline = area ? Math.min(plot.y + plot.h, Math.max(plot.y, y.map(y.log ? y.domain[0] : 0))) : null;
    return segments.length === 0 ? [] : [{ kind: "line", color: seriesColor(input, i), segments, baseline }];
  });
  return {
    width: input.width,
    height: input.height,
    plot,
    xTicks: frame.ticks.map((t) => ({ px: x.map(t.x), label: t.label })),
    yLabels: [],
    yAxes: right === null ? [axis("left", left)] : [axis("left", left), axis("right", right)],
    x,
    marks,
    legend: legendOf(
      input,
      content.series.map((s) => s.label),
      area ? "rect" : "line"
    ),
    empty: content.series.every((s) => s.x.length === 0) ? "No data in this range" : null
  };
}

function histogramModel(input: ModelInput, frame: ChartFrameData, content: Content<"histogram">): ChartModel {
  const visible = content.series.map((s, i) => ({ s, i })).filter(({ s }) => !hidden(input, s.label));
  const counts = visible.flatMap(({ s }) => s.counts);
  const range: readonly [number, number] = [input.height - X_AXIS, PAD];
  const scale = valueScale(input, counts, range, true);
  const plot = plotFor(input, axis("left", scale).ticks, null);
  const y = onto(scale, [plot.y + plot.h, plot.y]);
  const x = linearScale([frame.xFrom, frame.xTo], [plot.x, plot.x + plot.w]);
  const marks: Mark[] = [];
  const base = y.map(y.log ? y.domain[0] : 0);
  visible.forEach(({ s, i }, n) => {
    for (let b = 0; b < s.counts.length; b += 1) {
      const count = s.counts[b] ?? 0;
      const [from, to] = [x.map(s.from[b] ?? 0), x.map(s.to[b] ?? 0)];
      // Series share a bin side by side; bins keep a surface gap between them.
      const share = (to - from - GAP) / visible.length;
      if (count <= 0 || share <= 0) {
        continue;
      }
      const top = y.map(count);
      const hit: Hit = { title: `${s.fromText[b] ?? ""} to ${s.toText[b] ?? ""}`, rows: [{ label: s.label, value: COUNT.format(count) }] };
      marks.push({ kind: "rect", color: seriesColor(input, i), rect: { x: from + GAP / 2 + n * share, y: top, w: share, h: base - top }, round: "top", hit });
    }
  });
  return {
    width: input.width,
    height: input.height,
    plot,
    xTicks: frame.ticks.map((t) => ({ px: x.map(t.x), label: t.label })),
    yLabels: [],
    yAxes: [axis("left", y)],
    x,
    marks,
    legend: legendOf(
      input,
      content.series.map((s) => s.label),
      "rect"
    ),
    empty: counts.every((c) => c === 0) ? "No data in this range" : null
  };
}

/** The ramp step for a share of the most, `0 < share <= 1`. */
function rampColor(ramp: readonly string[], share: number): string {
  const step = Math.min(ramp.length - 1, Math.max(0, Math.ceil(share * ramp.length) - 1));
  return ramp[step] ?? ramp[ramp.length - 1] ?? "#000000";
}

function densityModel(input: ModelInput, frame: ChartFrameData, content: Content<"density">): ChartModel {
  const range: readonly [number, number] = [input.height - X_AXIS, PAD];
  const scale = linearScale([content.yFrom, content.yTo], range);
  const plot = plotFor(input, axis("left", scale).ticks, null);
  const y = linearScale([content.yFrom, content.yTo], [plot.y + plot.h, plot.y]);
  const x = linearScale([frame.xFrom, frame.xTo], [plot.x, plot.x + plot.w]);
  // A density has a cell per 4 px square, too many to spread as arguments.
  let most = 0;
  for (const count of content.counts) {
    most = Math.max(most, count);
  }
  const [cw, ch] = [plot.w / content.columns, plot.h / content.rows];
  const marks: Mark[] = [];
  for (let r = 0; r < content.rows; r += 1) {
    for (let c = 0; c < content.columns; c += 1) {
      const count = content.counts[r * content.columns + c] ?? 0;
      if (count > 0) {
        // Counts span orders of magnitude, so the ramp follows their logarithm.
        const share = Math.log1p(count) / Math.log1p(most);
        marks.push({ kind: "cell", color: rampColor(input.colors.ramp, share), rect: { x: plot.x + c * cw, y: plot.y + plot.h - (r + 1) * ch, w: cw, h: ch }, hit: null });
      }
    }
  }
  return {
    width: input.width,
    height: input.height,
    plot,
    xTicks: frame.ticks.map((t) => ({ px: x.map(t.x), label: t.label })),
    yLabels: [],
    yAxes: [axis("left", y)],
    x,
    marks,
    legend: [],
    empty: most === 0 ? "No data in this range" : null
  };
}

const PERIODS: Readonly<Record<string, string>> = { week: "Week", month: "Month", quarter: "Quarter", year: "Year" };

/** A bucket's title: a day's date, or the period it starts, such as "Month of 2024-06-01" or "5 minutes from 09:30". */
export function bucketTitle(interval: string, start: string): string {
  const period = PERIODS[interval];
  if (period !== undefined) {
    return `${period} of ${start}`;
  }
  const many = /^(\d+)([my])$/.exec(interval);
  if (many === null) {
    return start;
  }
  const [count, unit] = [many[1] ?? "", many[2] === "m" ? "minute" : "year"];
  return `${count} ${unit}${count === "1" ? "" : "s"} from ${start}`;
}

/** The candle or OHLC bar for bucket `i`, or null if a price is missing. */
function candleMark(input: ModelInput, c: Content<"candles">["candles"], i: number, x: Scale, y: Scale, bars: boolean): Mark | null {
  const [open, high, low, close] = [c.open[i] ?? Number.NaN, c.high[i] ?? Number.NaN, c.low[i] ?? Number.NaN, c.close[i] ?? Number.NaN];
  if (![open, high, low, close].every(Number.isFinite)) {
    return null;
  }
  const [from, to] = [x.map(c.from[i] ?? 0), x.map(c.to[i] ?? 0)];
  const up = close >= open;
  const hit: Hit = {
    title: bucketTitle(c.interval, c.xText[i] ?? ""),
    rows: [
      { label: "Open", value: c.openText[i] ?? "" },
      { label: "High", value: c.highText[i] ?? "" },
      { label: "Low", value: c.lowText[i] ?? "" },
      { label: "Close", value: c.closeText[i] ?? "" }
    ]
  };
  return {
    kind: "candle",
    color: up ? input.colors.up : input.colors.down,
    // Rising candles are hollow and falling ones filled, so direction never rests on colour alone.
    hollow: up,
    bars,
    x: (from + to) / 2,
    width: Math.max(1, Math.min(MAX_BAR, (to - from) * 0.7)),
    open: y.map(open),
    close: y.map(close),
    high: y.map(high),
    low: y.map(low),
    hit
  };
}

function candleModel(input: ModelInput, frame: ChartFrameData, content: Content<"candles">): ChartModel {
  const c = content.candles;
  const range: readonly [number, number] = [input.height - X_AXIS, PAD];
  const scale = valueScale(input, [...c.low, ...c.high], range, false);
  const plot = plotFor(input, axis("left", scale).ticks, null);
  const y = onto(scale, [plot.y + plot.h, plot.y]);
  const x = linearScale([frame.xFrom, frame.xTo], [plot.x, plot.x + plot.w]);
  const bars = input.chart.kind === "ohlc";
  const marks: Mark[] = [];
  for (let i = 0; i < c.from.length; i += 1) {
    const mark = candleMark(input, c, i, x, y, bars);
    if (mark !== null) {
      marks.push(mark);
    }
  }
  return {
    width: input.width,
    height: input.height,
    plot,
    xTicks: frame.ticks.map((t) => ({ px: x.map(t.x), label: t.label })),
    yLabels: [],
    yAxes: [axis("left", y)],
    x,
    marks,
    legend: [],
    empty: marks.length === 0 ? "No data in this range" : null
  };
}

/** A label's height where labels stack down the side. */
const LINE = 16;

/**
 * Category labels thinned so that they do not overlap: along an axis across, each needs its text's width; down the
 * side, each needs a line.
 */
function categoryTicks(input: ModelInput, categories: readonly string[], position: (i: number) => number, length: number, across = true): readonly AxisTick[] {
  const room = across ? Math.max(1, ...categories.map((c) => input.measure(c))) + PAD : LINE;
  const every = Math.max(1, Math.ceil((room * categories.length) / Math.max(1, length)));
  return categories.flatMap((label, i) => (i % every === 0 ? [{ px: position(i), label }] : []));
}

/** Where each bar of a category sits along its band, and how thick it is. */
function barSlots(band: number, visible: number, stacked: boolean): { readonly thickness: number; readonly offset: (n: number) => number } {
  if (stacked || visible <= 1) {
    const thickness = Math.max(1, Math.min(MAX_BAR, band * 0.6));
    return { thickness, offset: () => (band - thickness) / 2 };
  }
  const thickness = Math.max(1, Math.min(MAX_BAR, (band * 0.8 - GAP * (visible - 1)) / visible));
  const group = visible * thickness + (visible - 1) * GAP;
  return { thickness, offset: (n) => (band - group) / 2 + n * (thickness + GAP) };
}

/** How bars of one chart are laid out. */
interface BarGeometry {
  readonly value: Scale;
  readonly horizontal: boolean;
  readonly stacked: boolean;
  readonly thickness: number;
}

/** A bar from `start` to `start + v` along the value axis, at `across` on the category axis. */
function barRect(g: BarGeometry, start: number, v: number, across: number): { readonly kind: "rect"; readonly rect: Rect; readonly round: "top" | "bottom" | "left" | "right" } {
  const [a, b] = [g.value.map(start), g.value.map(start + v)];
  // Segments of a stack keep a surface gap between them, cut from their baseline end.
  const inset = g.stacked && start !== 0 ? GAP : 0;
  const size = Math.max(0, Math.abs(b - a) - inset);
  const low = Math.min(a, b);
  if (g.horizontal) {
    return { kind: "rect", rect: { x: v > 0 ? low + inset : low, y: across, w: size, h: g.thickness }, round: v > 0 ? "right" : "left" };
  }
  return { kind: "rect", rect: { x: across, y: v > 0 ? low : low + inset, w: g.thickness, h: size }, round: v > 0 ? "top" : "bottom" };
}

function barModel(input: ModelInput, data: CategoryData): ChartModel {
  const horizontal = input.chart.horizontal;
  const stacked = input.chart.stacked;
  const visible = data.series.map((s, i) => ({ s, i })).filter(({ s }) => !hidden(input, s.label));
  // A stack's extent is its positive and negative sums; a missing value adds nothing.
  const finite = (v: number | undefined): number => (v !== undefined && Number.isFinite(v) ? v : 0);
  const totals = stacked
    ? data.categories.flatMap((_, c) => [visible.reduce((sum, { s }) => sum + Math.max(0, finite(s.values[c])), 0), visible.reduce((sum, { s }) => sum + Math.min(0, finite(s.values[c])), 0)])
    : visible.flatMap(({ s }) => s.values);
  const length = horizontal ? input.width - 2 * PAD : input.height - X_AXIS - PAD;
  const scale = valueScale({ ...input, chart: { ...input.chart, log_y: false } }, totals, [0, length], true);
  const labelWidth = horizontal ? Math.min(input.width / 3, Math.max(0, ...data.categories.map((c) => input.measure(c))) + PAD + 2) : 0;
  const plot = horizontal
    ? { x: labelWidth, y: PAD, w: Math.max(1, input.width - labelWidth - PAD), h: Math.max(1, input.height - PAD - X_AXIS) }
    : plotFor(input, axis("left", onto(scale, [input.height - X_AXIS, PAD])).ticks, null);
  const value = horizontal ? onto(scale, [plot.x, plot.x + plot.w]) : onto(scale, [plot.y + plot.h, plot.y]);
  const band = (horizontal ? plot.h : plot.w) / Math.max(1, data.categories.length);
  const slots = barSlots(band, visible.length, stacked);
  const marks: Mark[] = [];
  data.categories.forEach((category, c) => {
    const stack = { above: 0, below: 0 };
    visible.forEach(({ s, i }, n) => {
      const v = s.values[c] ?? Number.NaN;
      if (!Number.isFinite(v) || v === 0) {
        return;
      }
      const start = stacked ? (v > 0 ? stack.above : stack.below) : 0;
      if (stacked) {
        stack.above += Math.max(0, v);
        stack.below += Math.min(0, v);
      }
      const across = (horizontal ? plot.y : plot.x) + c * band + slots.offset(n);
      const geometry = { value, horizontal, stacked, thickness: slots.thickness };
      marks.push({ ...barRect(geometry, start, v, across), color: seriesColor(input, i), hit: { title: category, rows: [{ label: s.label, value: s.texts[c] ?? "" }] } });
    });
  });
  const positions = (i: number): number => (horizontal ? plot.y : plot.x) + (i + 0.5) * band;
  return {
    width: input.width,
    height: input.height,
    plot,
    xTicks: horizontal ? valueTicks(value, Math.max(2, plot.w / 90)).map((t) => ({ px: t.px, label: t.label })) : categoryTicks(input, data.categories, positions, plot.w),
    yLabels: horizontal ? categoryTicks(input, data.categories, positions, plot.h, false) : [],
    yAxes: horizontal ? [] : [axis("left", value)],
    x: null,
    marks,
    legend: legendOf(
      input,
      data.series.map((s) => s.label),
      "rect"
    ),
    empty: data.categories.length === 0 ? "No groups to show" : null
  };
}

/** A heatmap cell's colour: one sequential ramp, or two arms around a neutral middle when values take both signs. */
function heatColor(colors: ChartColors, value: number, low: number, high: number): string {
  if (low < 0 && high > 0) {
    const most = Math.max(-low, high);
    if (value === 0) {
      return colors.middle;
    }
    return rampColor(value > 0 ? colors.ramp : colors.negativeRamp, Math.abs(value) / most);
  }
  if (high <= 0 && low < 0) {
    // Only losses: the negative arm, the deepest darkest.
    return rampColor(colors.negativeRamp, Math.max(Number.EPSILON, value / low));
  }
  const span = high - low;
  return rampColor(colors.ramp, span === 0 ? 1 : Math.max(Number.EPSILON, (value - low) / span));
}

function heatmapModel(input: ModelInput, data: CategoryData): ChartModel {
  const rows = data.series.filter((s) => !hidden(input, s.label));
  const all = extent(
    rows.flatMap((s) => s.values),
    false
  ) ?? [0, 1];
  const labelWidth = Math.min(input.width / 3, Math.max(0, ...rows.map((s) => input.measure(s.label))) + PAD + 2);
  const plot = { x: labelWidth, y: PAD, w: Math.max(1, input.width - labelWidth - PAD), h: Math.max(1, input.height - PAD - X_AXIS) };
  const [cw, ch] = [plot.w / Math.max(1, data.categories.length), plot.h / Math.max(1, rows.length)];
  const marks: Mark[] = [];
  rows.forEach((s, r) => {
    data.categories.forEach((category, c) => {
      const v = s.values[c] ?? Number.NaN;
      if (!Number.isFinite(v)) {
        return;
      }
      const rect = { x: plot.x + c * cw + GAP / 2, y: plot.y + r * ch + GAP / 2, w: Math.max(0, cw - GAP), h: Math.max(0, ch - GAP) };
      marks.push({ kind: "cell", color: heatColor(input.colors, v, all[0], all[1]), rect, hit: { title: category, rows: [{ label: s.label, value: s.texts[c] ?? "" }] } });
    });
  });
  return {
    width: input.width,
    height: input.height,
    plot,
    xTicks: categoryTicks(input, data.categories, (i) => plot.x + (i + 0.5) * cw, plot.w),
    yLabels: rows.map((s, r) => ({ px: plot.y + (r + 0.5) * ch, label: s.label })),
    yAxes: [],
    x: null,
    marks,
    legend: [],
    empty: marks.length === 0 ? "No values to show" : null
  };
}

/** Builds the draw model for a chart's latest data. */
export function chartModel(input: ModelInput): ChartModel {
  if (input.width < 40 || input.height < 40) {
    return emptyModel(input, "");
  }
  if (input.chart.kind === "bar" || input.chart.kind === "heatmap") {
    if (input.categories === null) {
      return emptyModel(input, "Loading…");
    }
    return input.chart.kind === "bar" ? barModel(input, input.categories) : heatmapModel(input, input.categories);
  }
  const frame = input.frame;
  if (frame === null) {
    return emptyModel(input, "Loading…");
  }
  const content = frame.content;
  switch (content.kind) {
    case "series":
      return lineModel(input, frame, content);
    case "histogram":
      return histogramModel(input, frame, content);
    case "density":
      return densityModel(input, frame, content);
    case "candles":
      return candleModel(input, frame, content);
  }
}
