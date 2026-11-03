/**
 * Indicators on a chart: averages, bands and VWAP drawn over the price, and
 * RSI, MACD, ATR, the stochastic and volume in panes stacked under it on the
 * same x axis. The engine computed every value over every row and sent its
 * text; this decides where each line goes, its colour, and its label.
 */

import type { CandleData, ChartFrameData, IndicatorLineData } from "../chart-frame";
import type { LayoutChart, LayoutIndicator } from "../layout";
import type { ChartColors } from "../theme";
import { linearScale, niceDomain, valueTicks } from "./scale";
import type { Scale } from "./scale";

/** A pixel rectangle. */
interface Box {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
}

/** How one indicator line is drawn and named, in the order of the frame's lines. */
export interface IndicatorStyle {
  /** Its label in a tooltip, such as "SMA 20", "BB 20 2 upper" or "%K". */
  readonly label: string;
  /** The legend entry or pane it belongs to: overlays share a legend entry per indicator and series. */
  readonly group: string;
  readonly color: string;
  /** The pane it is drawn in, by index, or null over the price. */
  readonly pane: number | null;
  /** Bars from zero rather than a line, for MACD's histogram. */
  readonly columns: boolean;
  /** A dashed line: Bollinger Bands' middle, or a pane's second line when several series share its colours. */
  readonly dashed: boolean;
  /** The legend label of the series it follows, which hides it too, or "" for none. */
  readonly follows: string;
}

/** A pane of its own for one indicator. */
export interface PaneSpec {
  readonly title: string;
  /** A fixed value range, as for oscillators from 0 to 100, or null to fit the values. */
  readonly domain: readonly [number, number] | null;
  /** Lines drawn across the pane at these values, such as RSI's 30 and 70. */
  readonly guides: readonly number[];
  /** Whether the pane holds a candle chart's volume. */
  readonly volume: boolean;
}

const OVERLAYS: ReadonlySet<LayoutIndicator["kind"]> = new Set(["sma", "ema", "wma", "bollinger", "vwap"]);

/** Whether an indicator is drawn over the price, rather than in a pane of its own. */
export function overlays(indicator: LayoutIndicator): boolean {
  return OVERLAYS.has(indicator.kind);
}

/** An indicator's name with its parameters, as in its legend entry or pane title. */
export function indicatorLabel(indicator: LayoutIndicator): string {
  switch (indicator.kind) {
    case "sma":
    case "ema":
    case "wma":
    case "rsi":
    case "atr":
      return `${indicator.kind.toUpperCase()} ${String(indicator.period)}`;
    case "bollinger":
      return `BB ${String(indicator.period)} ${String(indicator.deviations)}`;
    case "macd":
      return `MACD ${String(indicator.fast)} ${String(indicator.slow)} ${String(indicator.signal)}`;
    case "stochastic":
      return `Stoch ${String(indicator.k)} ${String(indicator.k_smooth)} ${String(indicator.d)}`;
    case "vwap":
      return "VWAP";
    case "volume":
      return "Volume";
  }
}

/** What a line of an indicator is called in a tooltip. */
const LINE_NAMES: Readonly<Record<string, string>> = { macd: "MACD", signal: "Signal", histogram: "Histogram", k: "%K", d: "%D" };

/** The panes a chart stacks under its price, in the order of its indicators. */
export function paneSpecs(chart: LayoutChart): readonly PaneSpec[] {
  return chart.indicators.filter((i) => !overlays(i)).map(paneSpec);
}

function paneSpec(indicator: LayoutIndicator): PaneSpec {
  const title = indicatorLabel(indicator);
  switch (indicator.kind) {
    case "rsi":
      return { title, domain: [0, 100], guides: [30, 70], volume: false };
    case "stochastic":
      return { title, domain: [0, 100], guides: [20, 80], volume: false };
    case "macd":
      return { title, domain: null, guides: [0], volume: false };
    case "volume":
      return { title, domain: null, guides: [], volume: true };
    case "atr":
    case "sma":
    case "ema":
    case "wma":
    case "bollinger":
    case "vwap":
      return { title, domain: null, guides: [], volume: false };
  }
}

/** Where each indicator of a chart is drawn: its pane, by index among the panes, or null over the price. */
function paneIndexes(chart: LayoutChart): readonly (number | null)[] {
  let next = 0;
  return chart.indicators.map((i) => {
    if (overlays(i)) {
      return null;
    }
    next += 1;
    return next - 1;
  });
}

/**
 * The style of every indicator line in a frame. Overlays take the palette's slots after the plotted series, one per
 * indicator and series, so an average never shares a colour with a price; a pane's lines take the first slots, its
 * histogram the up and down colours.
 */
export function indicatorStyles(chart: LayoutChart, frame: ChartFrameData, colors: ChartColors, muted: string, plotted: number): readonly IndicatorStyle[] {
  // Lines follow a series of a line chart, whose legend label is its price column and the series' value.
  const price = chart.y[0] ?? "";
  const panes = paneIndexes(chart);
  const groups = new Map<string, number>();
  const order = [...new Set(frame.indicators.map((l) => l.series))];
  const many = order.length > 1;
  const color = (slot: number): string => colors.series[slot] ?? muted;
  return frame.indicators.map((line): IndicatorStyle => {
    const indicator = chart.indicators[line.indicator] ?? { kind: "volume" };
    const series = many && line.series !== "" ? ` ${line.series}` : "";
    const group = `${indicatorLabel(indicator)}${series}`;
    const follows = line.series === "" || price === "" ? "" : `${price} ${line.series}`;
    const pane = panes[line.indicator] ?? null;
    if (pane === null) {
      if (!groups.has(group)) {
        groups.set(group, groups.size);
      }
      return overlayStyle(indicator, line.line, group, follows, color(plotted + (groups.get(group) ?? 0)));
    }
    return paneStyle(line, { group, follows, pane, series, many, order, color });
  });
}

function overlayStyle(indicator: LayoutIndicator, line: string, group: string, follows: string, color: string): IndicatorStyle {
  const part = indicator.kind === "bollinger" ? ` ${line}` : "";
  return { label: `${group}${part}`, group, color, pane: null, columns: false, dashed: line === "middle", follows };
}

/** What a pane's line needs to know of its neighbours. */
interface PaneContext {
  readonly group: string;
  readonly follows: string;
  readonly pane: number;
  readonly series: string;
  readonly many: boolean;
  readonly order: readonly string[];
  readonly color: (slot: number) => string;
}

/**
 * In a pane, a second line such as MACD's signal takes the next slot; with several series, each keeps its own slot and
 * the second line is dashed instead.
 */
function paneStyle(line: IndicatorLineData, c: PaneContext): IndicatorStyle {
  const second = line.line === "signal" || line.line === "d";
  const slot = c.many ? c.order.indexOf(line.series) : Number(second);
  const label = `${LINE_NAMES[line.line] ?? c.group}${c.series}`;
  return { label, group: c.group, color: c.color(slot), pane: c.pane, columns: line.line === "histogram", dashed: c.many && second, follows: c.follows };
}

/** The shortest pane worth drawing. */
const SHORTEST = 16;

/**
 * Pixel heights of panes: about a fifth of the plot each, never leaving the price less than two fifths. Zero when that
 * leaves panes too short to read, and then none are drawn.
 */
export function paneHeight(count: number, available: number): number {
  if (count === 0) {
    return 0;
  }
  const wanted = Math.min(140, Math.max(48, Math.round(available / 5)));
  const most = (available * 0.6) / count - PANE_GAP;
  const height = Math.min(wanted, most);
  return height < SHORTEST ? 0 : height;
}

/** Pixels between the price and the first pane, and between panes. */
export const PANE_GAP = 10;

/** A pane's value range: fixed, or fitted to its values with zero in view. */
export function paneDomain(spec: PaneSpec, values: readonly number[], height: number): readonly [number, number] {
  if (spec.domain !== null) {
    return spec.domain;
  }
  let [low, high] = [spec.volume ? 0 : Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY];
  for (const v of values) {
    if (Number.isFinite(v)) {
      low = Math.min(low, v);
      high = Math.max(high, v);
    }
  }
  if (!(low <= high)) {
    return [0, 1];
  }
  return niceDomain(low, high, Math.max(2, height / 28), spec.guides.includes(0) || spec.volume);
}

/** The values a pane draws, for fitting its range. */
export function paneValues(lines: readonly IndicatorLineData[], styles: readonly IndicatorStyle[], pane: number, candles: CandleData | null, spec: PaneSpec): number[] {
  if (spec.volume) {
    return candles?.volume === null || candles === null ? [] : Array.from(candles.volume);
  }
  return lines.flatMap((l, i) => (styles[i]?.pane === pane ? Array.from(l.y) : []));
}

/** A pane's scale and ticks for its box. */
export function paneScale(domain: readonly [number, number], box: Box): { readonly scale: Scale; readonly ticks: ReturnType<typeof valueTicks> } {
  const scale = linearScale(domain, [box.y + box.h, box.y]);
  // A tick at the very top would be labelled into the axis above it.
  const ticks = valueTicks(scale, Math.max(2, box.h / 28)).filter((t) => t.px >= box.y + 8);
  return { scale, ticks };
}

/** A polyline's runs between breaks, as pixel pairs; values a scale cannot place, such as zero on a log axis, break it too. */
export function segmentsOf(xs: Float64Array, ys: Float64Array, x: Scale, y: Scale): Float64Array[] {
  const segments: Float64Array[] = [];
  let current: number[] = [];
  for (let p = 0; p < xs.length; p += 1) {
    const value = ys[p] ?? Number.NaN;
    if (Number.isNaN(value) || (y.log && value <= 0)) {
      if (current.length > 0) {
        segments.push(Float64Array.from(current));
      }
      current = [];
      continue;
    }
    current.push(x.map(xs[p] ?? 0), y.map(value));
  }
  if (current.length > 0) {
    segments.push(Float64Array.from(current));
  }
  return segments;
}
