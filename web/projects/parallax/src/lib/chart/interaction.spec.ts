import { describe, expect, it } from "vitest";

import type { CandleData, ChartContent, ChartFrameData, SeriesData } from "../chart-frame";
import { parseLayout } from "../layout";
import type { LayoutChart } from "../layout";
import { CHART_COLORS } from "../theme";
import { brushFilters, markAt, nearestIndex, panRange, readSeries, zoomRange } from "./interaction";
import { chartModel } from "./model";
import type { ChartModel } from "./model";

function chart(partial: Partial<LayoutChart> & Pick<LayoutChart, "kind">): LayoutChart {
  const layout = parseLayout({ chart: partial });
  if (layout.chart === null) {
    throw new Error("no chart");
  }
  return layout.chart;
}

function frame(content: ChartContent, xFrom = 0, xTo = 10): ChartFrameData {
  return { viewId: 1, configVersion: 0n, loadVersion: 1n, sourceAsOfMicros: 0n, pushed: false, xFrom, xTo, extentFrom: xFrom, extentTo: xTo, ticks: [], content, indicators: [], anchors: new Map() };
}

function model(c: LayoutChart, f: ChartFrameData): ChartModel {
  return chartModel({ chart: c, width: 400, height: 300, colors: CHART_COLORS.light, muted: "#999999", measure: (t) => t.length * 6, frame: f, categories: null });
}

/** A series at the given x positions, with `null` for a gap. */
function series(label: string, points: readonly (readonly [number, number | null])[]): SeriesData {
  return {
    label,
    column: "close",
    right: false,
    x: Float64Array.from(points.map(([x]) => x)),
    y: Float64Array.from(points.map(([, y]) => y ?? Number.NaN)),
    xText: points.map(([x]) => `day ${String(x)}`),
    yText: points.map(([, y]) => (y === null ? null : y.toFixed(2))),
    xJson: points.map(([x]) => JSON.stringify(`2024-01-${String(x + 1).padStart(2, "0")}`))
  };
}

const CANDLES: CandleData = {
  interval: "day",
  from: Float64Array.from([0, 1, 2]),
  to: Float64Array.from([1, 2, 3]),
  open: Float64Array.from([10, 12, 11]),
  high: Float64Array.from([13, 13, 12]),
  low: Float64Array.from([9, 10, 10]),
  close: Float64Array.from([12, 10, 11.5]),
  xText: ["Mon", "Tue", "Wed"],
  openText: ["10", "12", "11"],
  highText: ["13", "13", "12"],
  lowText: ["9", "10", "10"],
  closeText: ["12", "10", "11.5"],
  volume: null,
  volumeText: null,
  xJson: ['"2024-01-01"', '"2024-01-02"', '"2024-01-03"'],
  endJson: ['"2024-01-02"', '"2024-01-03"', '"2024-01-04"']
};

describe("nearestIndex", () => {
  it("finds the nearest value, the earlier on a tie, and -1 for nothing", () => {
    const xs = Float64Array.from([0, 2, 4, 10]);
    expect(nearestIndex(new Float64Array(0), 3)).toBe(-1);
    expect(nearestIndex(xs, -5)).toBe(0);
    expect(nearestIndex(xs, 2)).toBe(1);
    expect(nearestIndex(xs, 2.9)).toBe(1);
    expect(nearestIndex(xs, 3)).toBe(1);
    expect(nearestIndex(xs, 3.1)).toBe(2);
    expect(nearestIndex(xs, 8)).toBe(3);
    expect(nearestIndex(xs, 99)).toBe(3);
  });
});

describe("readSeries", () => {
  const content: ChartContent = {
    kind: "series",
    series: [
      series("AAPL", [
        [0, 1],
        [2, 2],
        [4, 3]
      ]),
      series("MSFT", [
        [0, 10],
        [2, null],
        [3, 30],
        [4, 40]
      ])
    ]
  };
  const f = frame(content, 0, 4);

  it("snaps to the nearest point of any series and reads every series that has a value there", () => {
    const m = model(chart({ kind: "line" }), f);
    const at3 = readSeries(f, m, [], 3.2);
    // Only MSFT has a point at 3.
    expect(at3?.title).toBe("day 3");
    expect(at3?.rows).toEqual([{ value: "30.00", label: "MSFT", color: CHART_COLORS.light.series[1] }]);
    expect(at3?.crosshair).toBeCloseTo(m.x?.map(3) ?? Number.NaN);
    // At 2 MSFT has a gap, which says nothing rather than a wrong value.
    expect(readSeries(f, m, [], 2.1)?.rows.map((r) => r.label)).toEqual(["AAPL"]);
    expect(readSeries(f, m, [], 0)?.rows.map((r) => [r.label, r.value])).toEqual([
      ["AAPL", "1.00"],
      ["MSFT", "10.00"]
    ]);
  });

  it("ignores hidden series, keeping the shown ones' colours", () => {
    const m = model(chart({ kind: "line", hidden: ["MSFT"] }), f);
    const reading = readSeries(f, m, ["MSFT"], 3.2);
    // Hidden, MSFT's point at 3 is not snapped to: the nearest shown point is AAPL's at 4.
    expect(reading?.title).toBe("day 4");
    expect(reading?.rows).toEqual([{ value: "3.00", label: "AAPL", color: CHART_COLORS.light.series[0] }]);
    expect(readSeries(f, m, ["AAPL", "MSFT"], 3)).toBeNull();
  });

  it("never snaps to a break between segments, so the title is always a drawn point's", () => {
    const gappy = frame(
      {
        kind: "series",
        series: [
          series("AAPL", [
            [0, 1],
            [1, null],
            [2, null],
            [3, 4]
          ])
        ]
      },
      0,
      3
    );
    const m = model(chart({ kind: "line" }), gappy);
    expect(readSeries(gappy, m, [], 1.1)?.title).toBe("day 0");
    expect(readSeries(gappy, m, [], 1.9)?.title).toBe("day 3");
    expect(readSeries(gappy, m, [], 1.9)?.rows).toEqual([{ value: "4.00", label: "AAPL", color: CHART_COLORS.light.series[0] }]);
  });

  it("reads another series' nearest point a pixel or two away under its own x", () => {
    // A thousand days over the plot: points a day apart are under a pixel apart.
    const reduced = frame(
      {
        kind: "series",
        series: [
          series("AAPL", [
            [0, 1],
            [500, 2],
            [1000, 3]
          ]),
          series("MSFT", [
            [0, 10],
            [501, 20],
            [1000, 30]
          ]),
          series("NVDA", [
            [0, 5],
            [460, 6],
            [1000, 7]
          ])
        ]
      },
      0,
      1000
    );
    const m = model(chart({ kind: "line" }), reduced);
    const reading = readSeries(reduced, m, [], 500);
    expect(reading?.title).toBe("day 500");
    // NVDA's nearest point is forty days, several pixels, away: it says nothing rather than another day's value as this one's.
    expect(reading?.rows.map((r) => [r.label, r.value])).toEqual([
      ["AAPL", "2.00"],
      ["MSFT, day 501", "20.00"]
    ]);
  });

  it("reads nothing from a frame that is not series", () => {
    const candles = frame({ kind: "candles", candles: CANDLES }, 0, 3);
    expect(readSeries(candles, model(chart({ kind: "candlestick" }), candles), [], 1)).toBeNull();
  });
});

describe("markAt", () => {
  const candles = frame({ kind: "candles", candles: CANDLES }, 0, 3);
  const m = model(chart({ kind: "candlestick" }), candles);
  const marks = m.marks.filter((mark) => mark.kind === "candle");

  it("reads the candle nearest the pointer, within its hit area", () => {
    const [mon, tue] = [marks[0], marks[1]];
    if (mon === undefined || tue === undefined) {
      throw new Error("no candles");
    }
    const reading = markAt(m, mon.x + 1, 10);
    expect(reading?.title).toBe("Mon");
    // Day candles are titled by their date alone; longer buckets name their period, which the model tests check.
    expect(reading?.rows.map((r) => [r.label, r.value])).toEqual([
      ["Open", "10"],
      ["High", "13"],
      ["Low", "9"],
      ["Close", "12"]
    ]);
    expect(reading?.crosshair).toBe(mon.x);
    expect(markAt(m, tue.x - 1, 10)?.title).toBe("Tue");
  });

  it("reads a bar a few pixels outside it, and nothing further away", () => {
    const bins = { from: Float64Array.from([0, 5]), to: Float64Array.from([5, 10]), fromText: ["0", "5"], toText: ["5", "10"] };
    const hist = frame({ kind: "histogram", series: [{ label: "Beta", counts: [3, 1], ...bins }] });
    const hm = model(chart({ kind: "histogram" }), hist);
    const bar = hm.marks.find((mark) => mark.kind === "rect");
    if (bar === undefined) {
      throw new Error("no bars");
    }
    expect(markAt(hm, bar.rect.x + bar.rect.w / 2, bar.rect.y - 3)?.title).toBe("0 to 5");
    expect(markAt(hm, bar.rect.x + bar.rect.w / 2, bar.rect.y - 10)).toBeNull();
  });
});

describe("zoomRange and panRange", () => {
  const full = [0, 100] as const;

  it("zooms around the anchor, keeping its place in the range", () => {
    const zoomed = zoomRange([0, 100], 25, 0.5, full);
    expect(zoomed).toEqual([12.5, 62.5]);
  });

  it("returns null once zooming out reaches everything, and moves rather than shrinks at an edge", () => {
    expect(zoomRange([10, 90], 50, 1.25, full)).toBeNull();
    expect(zoomRange([0, 40], 0, 1.25, full)).toEqual([0, 50]);
    expect(zoomRange([60, 100], 100, 1.25, full)).toEqual([50, 100]);
  });

  it("stops zooming in at a millionth of the range", () => {
    let range: readonly [number, number] | null = [0, 100];
    for (let i = 0; i < 200 && range !== null; i += 1) {
      range = zoomRange(range, 50, 0.5, full);
    }
    expect(range?.[1] ?? 0).toBeGreaterThan(range?.[0] ?? 0);
    expect((range?.[1] ?? 0) - (range?.[0] ?? 0)).toBeCloseTo(100e-6);
  });

  it("pans by a delta and keeps the width at the edges", () => {
    expect(panRange([10, 30], 5, full)).toEqual([15, 35]);
    expect(panRange([10, 30], -50, full)).toEqual([0, 20]);
    expect(panRange([80, 100], 10, full)).toEqual([80, 100]);
    // Without a known full range, panning is free.
    expect(panRange([10, 30], -50, null)).toEqual([-40, -20]);
  });
});

describe("brushFilters", () => {
  const content: ChartContent = {
    kind: "series",
    series: [
      series("AAPL", [
        [0, 1],
        [2, 2],
        [4, 3]
      ]),
      series("MSFT", [
        [1, 10],
        [3, 30],
        [5, 50]
      ])
    ]
  };
  const f = frame(content, 0, 5);

  it("selects from the first point inside the range to the last, inclusive, with the exact values sent, and snaps to them", () => {
    expect(brushFilters(f, "date", [], [0.5, 4.2])).toEqual({
      filters: [
        { column: "date", op: "ge", value: "2024-01-02", values: [] },
        { column: "date", op: "le", value: "2024-01-05", values: [] }
      ],
      range: [1, 4]
    });
    // Brushed right to left, the same.
    expect(brushFilters(f, "date", [], [4.2, 0.5])).toEqual(brushFilters(f, "date", [], [0.5, 4.2]));
  });

  it("ignores hidden series and selects nothing from an empty range", () => {
    expect(brushFilters(f, "date", ["AAPL"], [0.5, 4.2])?.filters.map((x) => x.value)).toEqual(["2024-01-02", "2024-01-04"]);
    expect(brushFilters(f, "date", [], [5.5, 9])).toBeNull();
  });

  it("never bounds a selection by a break between segments", () => {
    const gappy: ChartContent = {
      kind: "series",
      series: [
        series("AAPL", [
          [0, 1],
          [1, null],
          [2, 2],
          [3, null],
          [4, 3]
        ])
      ]
    };
    const brushed = brushFilters(frame(gappy), "date", [], [0.5, 3.5]);
    expect(brushed?.filters.map((x) => x.value)).toEqual(["2024-01-03", "2024-01-03"]);
    expect(brushed?.range).toEqual([2, 2]);
    expect(brushFilters(frame(gappy), "date", [], [0.5, 1.5])).toBeNull();
  });

  it("selects every candle bucket the range touches, whole, up to the next bucket's start", () => {
    const candles = frame({ kind: "candles", candles: CANDLES }, 0, 3);
    // From the middle of Monday's bucket to the middle of Tuesday's: both, whole.
    expect(brushFilters(candles, "date", [], [0.5, 1.5])).toEqual({
      filters: [
        { column: "date", op: "ge", value: "2024-01-01", values: [] },
        { column: "date", op: "lt", value: "2024-01-03", values: [] }
      ],
      range: [0, 2]
    });
    expect(brushFilters(candles, "date", [], [3.5, 9])).toBeNull();
  });

  it("selects nothing when a bound's value is unknown", () => {
    const unknown: ChartContent = { kind: "series", series: [{ ...series("AAPL", [[0, 1]]), xJson: [null] }] };
    expect(brushFilters(frame(unknown), "date", [], [-1, 1])).toBeNull();
  });
});
