import { describe, expect, it } from "vitest";

import type { CandleData, ChartFrameData, IndicatorLineData, SeriesData } from "../chart-frame";
import { parseLayout } from "../layout";
import type { LayoutChart } from "../layout";
import { CHART_COLORS } from "../theme";
import { indicatorLabel, indicatorStyles, paneDomain, paneHeight, paneScale, paneSpecs } from "./indicators";
import { indicatorRows } from "./interaction";
import { chartModel, plotBottom } from "./model";
import type { ChartModel, Mark } from "./model";

const COLORS = CHART_COLORS.light;

function chart(indicators: unknown, kind = "candlestick"): LayoutChart {
  const layout = parseLayout({ chart: { kind, x: "date", ohlc: { open: "o", high: "h", low: "l", close: "c" }, volume: "v", indicators } });
  if (layout.chart === null) {
    throw new Error("no chart");
  }
  return layout.chart;
}

function line(indicator: number, name: string, points: readonly (readonly [number, number])[], series = ""): IndicatorLineData {
  return {
    indicator,
    line: name,
    series,
    x: Float64Array.from(points.map(([x]) => x)),
    y: Float64Array.from(points.map(([, y]) => y)),
    xText: points.map(([x]) => `day ${String(x)}`),
    yText: points.map(([, y]) => (Number.isNaN(y) ? null : y.toFixed(2)))
  };
}

/** Two weekly candles over days 0 to 9, rising then falling, with their volume. */
const CANDLES: CandleData = {
  interval: "week",
  from: Float64Array.from([0, 5]),
  to: Float64Array.from([5, 10]),
  open: Float64Array.from([10, 12]),
  high: Float64Array.from([13, 13]),
  low: Float64Array.from([9, 10]),
  close: Float64Array.from([12, 11]),
  xText: ["week 0", "week 5"],
  openText: ["10", "12"],
  highText: ["13", "13"],
  lowText: ["9", "10"],
  closeText: ["12", "11"],
  volume: Float64Array.from([3000, 1000]),
  volumeText: ["3,000", "1,000"],
  xJson: ["0", "5"],
  endJson: ["5", "10"]
};

function frame(indicators: readonly IndicatorLineData[]): ChartFrameData {
  return {
    viewId: 1,
    configVersion: 0n,
    loadVersion: 1n,
    sourceAsOfMicros: 0n,
    pushed: false,
    xFrom: 0,
    xTo: 10,
    extentFrom: 0,
    extentTo: 10,
    ticks: [],
    content: { kind: "candles", candles: CANDLES },
    indicators
  };
}

function model(c: LayoutChart, f: ChartFrameData): ChartModel {
  return chartModel({ chart: c, width: 600, height: 500, colors: COLORS, muted: "#999999", measure: (t) => t.length * 6, frame: f, categories: null });
}

const DAYS = Array.from({ length: 10 }, (_, d) => d);

describe("indicator labels, panes and styles", () => {
  it("names each indicator with its parameters", () => {
    const c = chart([{ kind: "sma" }, { kind: "bollinger", period: 10, deviations: 1.5 }, { kind: "macd" }, { kind: "stochastic" }, { kind: "vwap" }, { kind: "volume" }]);
    expect(c.indicators.map(indicatorLabel)).toEqual(["SMA 20", "BB 10 1.5", "MACD 12 26 9", "Stoch 14 3 3", "VWAP", "Volume"]);
    // Averages, bands and VWAP go over the price; the rest get panes, in order.
    expect(paneSpecs(c).map((p) => [p.title, p.domain, p.guides])).toEqual([
      ["MACD 12 26 9", null, [0]],
      ["Stoch 14 3 3", [0, 100], [20, 80]],
      ["Volume", null, []]
    ]);
  });

  it("gives overlays the slots after the series, and a pane's second line the next slot", () => {
    const c = chart([{ kind: "sma" }, { kind: "bollinger" }, { kind: "macd" }]);
    const f = frame([
      line(0, "sma", [[0, 1]]),
      line(1, "upper", [[0, 2]]),
      line(1, "middle", [[0, 1]]),
      line(1, "lower", [[0, 0]]),
      line(2, "macd", [[0, 1]]),
      line(2, "signal", [[0, 1]]),
      line(2, "histogram", [[0, 0]])
    ]);
    const styles = indicatorStyles(c, f, COLORS, "#999", 0);
    expect(styles.map((s) => [s.label, s.group, s.color, s.pane, s.columns, s.dashed])).toEqual([
      ["SMA 20", "SMA 20", COLORS.series[0], null, false, false],
      ["BB 20 2 upper", "BB 20 2", COLORS.series[1], null, false, false],
      ["BB 20 2 middle", "BB 20 2", COLORS.series[1], null, false, true],
      ["BB 20 2 lower", "BB 20 2", COLORS.series[1], null, false, false],
      ["MACD", "MACD 12 26 9", COLORS.series[0], 0, false, false],
      ["Signal", "MACD 12 26 9", COLORS.series[1], 0, false, false],
      ["Histogram", "MACD 12 26 9", COLORS.series[0], 0, true, false]
    ]);
    // Over a line chart of one series, overlays start after it.
    expect(indicatorStyles(c, f, COLORS, "#999", 1)[0]?.color).toBe(COLORS.series[1]);
  });

  it("keeps each series' colour in a pane shared by several, dashing the second line, and ties lines to their series", () => {
    const lines = chart([{ kind: "macd" }], "line");
    const both = frame([line(0, "macd", [[0, 1]], "AAPL"), line(0, "signal", [[0, 1]], "AAPL"), line(0, "macd", [[0, 1]], "MSFT"), line(0, "signal", [[0, 1]], "MSFT")]);
    const styles = indicatorStyles({ ...lines, y: ["close"] }, both, COLORS, "#999", 2);
    expect(styles.map((s) => [s.label, s.color, s.dashed, s.follows])).toEqual([
      ["MACD AAPL", COLORS.series[0], false, "close AAPL"],
      ["Signal AAPL", COLORS.series[0], true, "close AAPL"],
      ["MACD MSFT", COLORS.series[1], false, "close MSFT"],
      ["Signal MSFT", COLORS.series[1], true, "close MSFT"]
    ]);
  });

  it("keeps panes between a fifth and two fifths of the height, and fits their ranges", () => {
    expect(paneHeight(0, 400)).toBe(0);
    expect(paneHeight(1, 400)).toBe(80);
    expect(paneHeight(1, 1000)).toBe(140);
    // Five panes in 400 pixels share three fifths of it; four in 160 would be too short to read, so there are none.
    expect(paneHeight(5, 400)).toBeCloseTo(240 / 5 - 10);
    expect(paneHeight(4, 160)).toBe(0);
    const [rsi, macd, volume] = paneSpecs(chart([{ kind: "rsi" }, { kind: "macd" }, { kind: "volume" }]));
    if (rsi === undefined || macd === undefined || volume === undefined) {
      throw new Error("no panes");
    }
    expect(paneDomain(rsi, [55, 60], 80)).toEqual([0, 100]);
    const [low, high] = paneDomain(macd, [2, 7], 80);
    expect(low).toBeLessThanOrEqual(0);
    expect(high).toBeGreaterThanOrEqual(7);
    expect(paneDomain(volume, [3000, 1000], 80)[0]).toBe(0);
    // No tick is labelled at a pane's very top, where it would meet the axis above.
    const box = { x: 0, y: 100, w: 300, h: 80 };
    expect(paneScale([0, 100], box).ticks.every((t) => t.px >= box.y + 8)).toBe(true);
  });
});

function kinds<K extends Mark["kind"]>(marks: readonly Mark[], kind: K): Extract<Mark, { kind: K }>[] {
  return marks.filter((m): m is Extract<Mark, { kind: K }> => m.kind === kind);
}

describe("panes and overlays in the draw model", () => {
  const c = chart([{ kind: "bollinger" }, { kind: "rsi" }, { kind: "macd" }, { kind: "volume" }]);
  const f = frame([
    line(
      0,
      "upper",
      DAYS.map((d) => [d, 20])
    ),
    line(
      0,
      "middle",
      DAYS.map((d) => [d, 11])
    ),
    line(
      0,
      "lower",
      DAYS.map((d) => [d, 2])
    ),
    line(
      1,
      "rsi",
      DAYS.map((d) => [d, 40 + d])
    ),
    line(
      2,
      "macd",
      DAYS.map((d) => [d, d - 5])
    ),
    line(
      2,
      "signal",
      DAYS.map((d) => [d, 0])
    ),
    line(
      2,
      "histogram",
      DAYS.map((d) => [d, d - 5])
    )
  ]);
  const m = model(c, f);

  it("stacks the panes under the price on the same x axis, with the x axis under the last", () => {
    expect(m.panes.map((p) => p.title)).toEqual(["RSI 14", "MACD 12 26 9", "Volume"]);
    let above = m.plot.y + m.plot.h;
    for (const pane of m.panes) {
      expect(pane.plot.x).toBe(m.plot.x);
      expect(pane.plot.w).toBe(m.plot.w);
      expect(pane.plot.y).toBeGreaterThan(above);
      above = pane.plot.y + pane.plot.h;
    }
    expect(plotBottom(m)).toBe(above);
    expect(plotBottom(m)).toBeLessThanOrEqual(m.height - 20);
    expect(m.panes[0]?.guides.length).toBe(2);
  });

  it("widens the price's scale to hold the bands, and washes between them", () => {
    const y = m.yAxes[0]?.scale;
    expect(y?.domain[0]).toBeLessThanOrEqual(2);
    expect(y?.domain[1]).toBeGreaterThanOrEqual(20);
    expect(kinds(m.marks, "band").length).toBe(1);
    expect(kinds(m.marks, "line").filter((l) => l.dashed).length).toBe(1);
    expect(m.legend.map((e) => e.label)).toEqual(["BB 20 2"]);
  });

  it("draws MACD's histogram up and down from zero, and volume in each candle's direction", () => {
    const [macd, volume] = [m.panes[1], m.panes[2]];
    const columns = kinds(macd?.marks ?? [], "columns");
    // Days 5 to 9 are at or above zero, 0 to 4 below it.
    expect(columns.map((col) => [col.color, col.rects.length / 4])).toEqual([
      [COLORS.up, 5],
      [COLORS.down, 5]
    ]);
    const bars = kinds(volume?.marks ?? [], "columns");
    expect(bars.map((b) => [b.color, b.rects.length / 4])).toEqual([
      [COLORS.up, 1],
      [COLORS.down, 1]
    ]);
    // The rising week traded three times the falling one.
    const [up, down] = [bars[0]?.rects[3] ?? 0, bars[1]?.rects[3] ?? 0];
    expect(up / down).toBeCloseTo(3);
  });

  it("leaves a hidden overlay out of the plot and its scale", () => {
    const hidden = model({ ...c, hidden: ["BB 20 2"] }, f);
    expect(kinds(hidden.marks, "band").length).toBe(0);
    expect(hidden.yAxes[0]?.scale.domain[1]).toBeLessThan(20);
    expect(hidden.legend.map((e) => [e.label, e.hidden])).toEqual([["BB 20 2", true]]);
  });

  it("reads each indicator at the last day of the candle under the crosshair", () => {
    // Anywhere in the second week reads day 9, the day its close comes from.
    const rows = indicatorRows(f, m, [], 6.2);
    expect(rows.map((r) => [r.label, r.value])).toEqual([
      ["BB 20 2 upper, day 9", "20.00"],
      ["BB 20 2 middle, day 9", "11.00"],
      ["BB 20 2 lower, day 9", "2.00"],
      ["RSI 14, day 9", "49.00"],
      ["MACD, day 9", "4.00"],
      ["Signal, day 9", "0.00"],
      ["Histogram, day 9", "4.00"]
    ]);
    expect(indicatorRows(f, m, ["BB 20 2"], 1).map((r) => r.label)[0]).toBe("RSI 14, day 4");
    // Late in the first week is still the first week: its candle, its last day.
    expect(indicatorRows(f, m, ["BB 20 2"], 4.8).map((r) => r.label)[0]).toBe("RSI 14, day 4");
  });

  it("washes between the bands only where neither has a gap", () => {
    const gappy = DAYS.map((d): readonly [number, number] => [d, d === 5 ? Number.NaN : 20]);
    const low = DAYS.map((d): readonly [number, number] => [d, d === 5 ? Number.NaN : 2]);
    const g = model(chart([{ kind: "bollinger" }]), frame([line(0, "upper", gappy), line(0, "middle", gappy), line(0, "lower", low)]));
    // Two washes, one each side of the gap, none across it.
    const bands = kinds(g.marks, "band");
    expect(bands.length).toBe(2);
    expect(bands.every((b) => b.upper.length === b.lower.length)).toBe(true);
  });
});

describe("indicators over a line chart of several series", () => {
  it("hides a series' indicators with it", () => {
    const c = parseLayout({ chart: { kind: "line", x: "date", y: ["close"], series: "ticker", indicators: [{ kind: "sma" }] } }).chart;
    if (c === null) {
      throw new Error("no chart");
    }
    const series = (label: string, y: number): SeriesData => ({
      label,
      column: "close",
      right: false,
      x: Float64Array.from(DAYS),
      y: Float64Array.from(DAYS.map(() => y)),
      xText: DAYS.map(String),
      yText: DAYS.map(() => String(y)),
      xJson: DAYS.map(String)
    });
    const f: ChartFrameData = {
      ...frame([
        line(
          0,
          "sma",
          DAYS.map((d) => [d, 100]),
          "AAPL"
        ),
        line(
          0,
          "sma",
          DAYS.map((d) => [d, 1]),
          "MSFT"
        )
      ]),
      content: { kind: "series", series: [series("close AAPL", 100), series("close MSFT", 1)] }
    };
    const all = model(c, f);
    expect(all.legend.map((e) => e.label)).toEqual(["close AAPL", "close MSFT", "SMA 20 AAPL", "SMA 20 MSFT"]);
    const without = model({ ...c, hidden: ["close AAPL"] }, f);
    // AAPL's average goes with it: not drawn, not in the scale, not in the tooltip.
    expect(kinds(without.marks, "line").length).toBe(2);
    expect(without.yAxes[0]?.scale.domain[1]).toBeLessThan(100);
    expect(indicatorRows(f, without, ["close AAPL"], 3).map((r) => r.label)).toEqual(["SMA 20 MSFT"]);
  });
});
