import { describe, expect, it } from "vitest";

import type { ChartContent, ChartFrameData } from "../chart-frame";
import { parseLayout } from "../layout";
import type { LayoutChart } from "../layout";
import { CHART_COLORS } from "../theme";
import { bucketTitle, chartModel, describeChart } from "./model";
import type { CategoryData, Mark, ModelInput } from "./model";

function chart(partial: Partial<LayoutChart> & Pick<LayoutChart, "kind">): LayoutChart {
  const layout = parseLayout({ chart: partial });
  if (layout.chart === null) {
    throw new Error("no chart");
  }
  return layout.chart;
}

function frame(content: ChartContent): ChartFrameData {
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
    ticks: [{ x: 5, label: "Mid" }],
    content,
    indicators: []
  };
}

function input(c: LayoutChart, data: { readonly frame?: ChartFrameData; readonly categories?: CategoryData }): ModelInput {
  return { chart: c, width: 400, height: 300, colors: CHART_COLORS.light, muted: "#999999", measure: (t) => t.length * 6, frame: data.frame ?? null, categories: data.categories ?? null };
}

function series(label: string, y: readonly number[], right = false): Extract<ChartContent, { kind: "series" }>["series"][number] {
  return { label, column: "close", right, x: Float64Array.from(y.map((_, i) => i)), y: Float64Array.from(y), xText: y.map(() => "x"), yText: y.map(String), xJson: y.map((_, i) => String(i)) };
}

function kinds<K extends Mark["kind"]>(marks: readonly Mark[], kind: K): Extract<Mark, { kind: K }>[] {
  return marks.filter((m): m is Extract<Mark, { kind: K }> => m.kind === kind);
}

describe("chartModel", () => {
  it("keeps each series' colour when another is hidden, and breaks lines at gaps", () => {
    const content: ChartContent = { kind: "series", series: [series("close AAPL", [1, 2, 3]), series("close MSFT", [4, Number.NaN, 6])] };
    const all = chartModel(input(chart({ kind: "line" }), { frame: frame(content) }));
    const lines = kinds(all.marks, "line");
    expect(lines.map((l) => l.color)).toEqual([CHART_COLORS.light.series[0], CHART_COLORS.light.series[1]]);
    expect(lines[1]?.segments.length).toBe(2);
    expect(all.legend.map((e) => e.shape)).toEqual(["line", "line"]);
    const hidden = chartModel(input(chart({ kind: "line", hidden: ["close AAPL"] }), { frame: frame(content) }));
    expect(kinds(hidden.marks, "line").map((l) => l.color)).toEqual([CHART_COLORS.light.series[1]]);
    expect(hidden.legend[0]?.hidden).toBe(true);
    // The x ticks the engine labelled sit where the x scale puts them.
    expect(all.xTicks[0]?.px).toBeCloseTo(all.plot.x + all.plot.w / 2);
  });

  it("needs no legend for one series, gives right-axis series their own axis, and fills areas to zero", () => {
    const one = chartModel(input(chart({ kind: "area" }), { frame: frame({ kind: "series", series: [series("close", [5, 10])] }) }));
    expect(one.legend).toEqual([]);
    const area = kinds(one.marks, "line")[0];
    expect(area?.baseline).toBeCloseTo(one.plot.y + one.plot.h);
    const two = chartModel(input(chart({ kind: "line" }), { frame: frame({ kind: "series", series: [series("price", [100, 101]), series("volume", [5e6, 7e6], true)] }) }));
    expect(two.yAxes.map((a) => a.side)).toEqual(["left", "right"]);
    expect(two.yAxes[1]?.ticks.some((t) => t.label.endsWith("M"))).toBe(true);
  });

  it("drops values at or below zero from a log scale", () => {
    const model = chartModel(input(chart({ kind: "line", log_y: true }), { frame: frame({ kind: "series", series: [series("p", [10, 0, 100, 1000])] }) }));
    expect(model.yAxes[0]?.scale.log).toBe(true);
    expect(kinds(model.marks, "line")[0]?.segments.map((s) => s.length / 2)).toEqual([1, 2]);
  });

  it("draws histogram bins side by side with a surface gap and the engine's edge texts", () => {
    const bins = { from: Float64Array.from([0, 5]), to: Float64Array.from([5, 10]), fromText: ["0.00", "5.00"], toText: ["5.00", "10.00"] };
    const content: ChartContent = {
      kind: "histogram",
      series: [
        { label: "Long", counts: [3, 0], ...bins },
        { label: "Short", counts: [1, 2], ...bins }
      ]
    };
    const model = chartModel(input(chart({ kind: "histogram" }), { frame: frame(content) }));
    const rects = kinds(model.marks, "rect");
    expect(rects.length).toBe(3);
    const [first, second] = [rects[0], rects[1]];
    expect(first?.hit.title).toBe("0.00 to 5.00");
    // Two series share the first bin, each half of it less the gap.
    expect((first?.rect.w ?? 0) + (second?.rect.w ?? 0)).toBeCloseTo(model.plot.w / 2 - 2);
    expect(rects.every((r) => r.round === "top")).toBe(true);
  });

  it("draws rising candles hollow and falling ones filled, skipping buckets missing a price", () => {
    const candles = {
      interval: "day",
      from: Float64Array.from([0, 1, 2]),
      to: Float64Array.from([1, 2, 3]),
      open: Float64Array.from([10, 12, 11]),
      high: Float64Array.from([13, 13, 12]),
      low: Float64Array.from([9, 10, 10]),
      close: Float64Array.from([12, 10, Number.NaN]),
      xText: ["Mon", "Tue", "Wed"],
      openText: ["10", "12", "11"],
      highText: ["13", "13", "12"],
      lowText: ["9", "10", "10"],
      closeText: ["12", "10", null],
      volume: null,
      volumeText: null,
      xJson: ["0", "1", "2"],
      endJson: ["1", "2", "3"]
    };
    const model = chartModel(input(chart({ kind: "candlestick" }), { frame: frame({ kind: "candles", candles }) }));
    const [up, down, ...rest] = kinds(model.marks, "candle");
    expect(rest).toEqual([]);
    expect([up?.hollow, up?.color]).toEqual([true, CHART_COLORS.light.up]);
    expect([down?.hollow, down?.color]).toEqual([false, CHART_COLORS.light.down]);
    expect(up?.hit.rows.map((r) => r.value)).toEqual(["10", "13", "9", "12"]);
    expect((up?.width ?? 99) <= 24).toBe(true);
    const bars = chartModel(input(chart({ kind: "ohlc" }), { frame: frame({ kind: "candles", candles }) }));
    expect(kinds(bars.marks, "candle").every((c) => c.bars)).toBe(true);
  });

  const groups: CategoryData = {
    categories: ["Atlas", "Cobalt"],
    series: [
      { label: "Equity", values: [10, -4], texts: ["10.00", "(4.00)"] },
      { label: "Bond", values: [5, 6], texts: ["5.00", "6.00"] }
    ]
  };

  it("caps bars at 24 px, rounds their data ends and keeps the baseline square", () => {
    const model = chartModel(input(chart({ kind: "bar" }), { categories: groups }));
    const rects = kinds(model.marks, "rect");
    expect(rects.length).toBe(4);
    expect(rects.every((r) => r.rect.w <= 24)).toBe(true);
    expect(rects.find((r) => r.hit.rows[0]?.value === "(4.00)")?.round).toBe("bottom");
    expect(model.xTicks.map((t) => t.label)).toEqual(["Atlas", "Cobalt"]);
    // Bars grow from zero.
    expect(model.yAxes[0]?.scale.domain[0]).toBeLessThanOrEqual(-4);
  });

  it("stacks segments with a surface gap, and lays bars across when horizontal", () => {
    const stacked = chartModel(input(chart({ kind: "bar", stacked: true }), { categories: groups }));
    const atlas = kinds(stacked.marks, "rect").filter((r) => r.hit.title === "Atlas");
    const [equity, bond] = [atlas[0]?.rect, atlas[1]?.rect];
    // Bond sits on top of equity, two pixels apart.
    expect((equity?.y ?? 0) - ((bond?.y ?? 0) + (bond?.h ?? 0))).toBeCloseTo(2);
    const across = chartModel(input(chart({ kind: "bar", horizontal: true }), { categories: groups }));
    const rects = kinds(across.marks, "rect");
    expect(rects.every((r) => r.rect.h <= 24)).toBe(true);
    expect(rects.find((r) => r.hit.rows[0]?.value === "10.00")?.round).toBe("right");
    expect(across.yLabels.map((l) => l.label)).toEqual(["Atlas", "Cobalt"]);
  });

  it("colours a heatmap on two arms around a neutral middle when values take both signs", () => {
    const model = chartModel(input(chart({ kind: "heatmap" }), { categories: { categories: ["a", "b", "c"], series: [{ label: "x", values: [-8, 0, 8], texts: ["-8", "0", "8"] }] } }));
    const colors = kinds(model.marks, "cell").map((c) => c.color);
    expect(colors[0]).toBe(CHART_COLORS.light.negativeRamp[6]);
    expect(colors[1]).toBe(CHART_COLORS.light.middle);
    expect(colors[2]).toBe(CHART_COLORS.light.ramp[6]);
  });

  it("shades a density by the logarithm of its counts, the busiest cell darkest", () => {
    const content: ChartContent = { kind: "density", columns: 2, rows: 1, yFrom: 0, yTo: 1, counts: Uint32Array.from([1, 1000]) };
    const cells = kinds(chartModel(input(chart({ kind: "scatter" }), { frame: frame(content) })).marks, "cell");
    expect(cells.map((c) => c.color)).toEqual([CHART_COLORS.light.ramp[0], CHART_COLORS.light.ramp[6]]);
  });

  it("says so when a zoomed range holds no points, rather than drawing bare axes", () => {
    const model = chartModel(input(chart({ kind: "line" }), { frame: frame({ kind: "series", series: [series("a", []), series("b", [])] }) }));
    expect(model.marks).toEqual([]);
    expect(model.empty).toBe("No data in this range");
  });

  it("handles a density as large as a 4K plot", () => {
    const counts = new Uint32Array(960 * 540);
    counts[7] = 3;
    const content: ChartContent = { kind: "density", columns: 960, rows: 540, yFrom: 0, yTo: 1, counts };
    expect(kinds(chartModel(input(chart({ kind: "scatter" }), { frame: frame(content) })).marks, "cell").length).toBe(1);
  });

  it("draws a heatmap of only losses on the negative arm, the deepest darkest", () => {
    const model = chartModel(input(chart({ kind: "heatmap" }), { categories: { categories: ["a", "b"], series: [{ label: "x", values: [-8, -1], texts: ["-8", "-1"] }] } }));
    const colors = kinds(model.marks, "cell").map((c) => c.color);
    expect(colors[0]).toBe(CHART_COLORS.light.negativeRamp[6]);
    expect(CHART_COLORS.light.negativeRamp.indexOf(colors[1] ?? "")).toBeLessThan(6);
  });

  it("thins horizontal bar labels by line height", () => {
    const many: CategoryData = { categories: Array.from({ length: 80 }, (_, i) => `T${String(i)}`), series: [{ label: "v", values: Array.from({ length: 80 }, () => 1), texts: [] }] };
    const model = chartModel(input(chart({ kind: "bar", horizontal: true }), { categories: many }));
    const gaps = model.yLabels.slice(1).map((l, i) => l.px - (model.yLabels[i]?.px ?? 0));
    expect(model.yLabels.length).toBeGreaterThan(5);
    expect(Math.min(...gaps)).toBeGreaterThanOrEqual(16);
  });

  it("says when there is nothing to draw yet", () => {
    expect(chartModel(input(chart({ kind: "line" }), {})).empty).toBe("Loading…");
    expect(chartModel(input(chart({ kind: "bar" }), { categories: { categories: [], series: [] } })).empty).toBe("No groups to show");
  });
});

describe("bucketTitle", () => {
  it("names the period a bucket starts, and a day by its date alone", () => {
    expect(bucketTitle("day", "2024-06-03")).toBe("2024-06-03");
    expect(bucketTitle("week", "2024-06-03")).toBe("Week of 2024-06-03");
    expect(bucketTitle("month", "2024-06-01")).toBe("Month of 2024-06-01");
    expect(bucketTitle("5m", "2024-06-03 09:30")).toBe("5 minutes from 2024-06-03 09:30");
    expect(bucketTitle("1m", "09:30")).toBe("1 minute from 09:30");
    expect(bucketTitle("2y", "2020-01-01")).toBe("2 years from 2020-01-01");
  });
});

describe("describeChart", () => {
  it("names the kind and shape, what is plotted and split by, and the dataset", () => {
    const bars = { dataset: "positions", group_by: ["sector"], split_by: ["asset_class"], measures: [{ column: "market_value_usd", aggregate: "sum" }] };
    expect(describeChart(parseLayout({ ...bars, chart: { kind: "bar", stacked: true } }))).toBe("Stacked bar chart of sector by asset_class from positions");
    expect(describeChart(parseLayout({ ...bars, chart: { kind: "bar", horizontal: true } }))).toBe("Horizontal bar chart of sector by asset_class from positions");
    expect(describeChart(parseLayout({ ...bars, chart: { kind: "bar", stacked: true, horizontal: true } }))).toBe("Stacked horizontal bar chart of sector by asset_class from positions");
    expect(describeChart(parseLayout({ ...bars, split_by: [], chart: { kind: "bar" } }))).toBe("Bar chart of sector from positions");
    expect(describeChart(parseLayout({ ...bars, chart: { kind: "heatmap" } }))).toBe("Heatmap of sector by asset_class from positions");
    expect(describeChart(parseLayout({ dataset: "prices", chart: { kind: "line", x: "date", y: ["close"], y_right: ["volume"], series: "ticker" } }))).toBe(
      "Line chart of close, volume by ticker from prices"
    );
    expect(describeChart(parseLayout({ dataset: "prices", chart: { kind: "candlestick", x: "date" } }))).toBe("Candlestick chart of date from prices");
  });
});
