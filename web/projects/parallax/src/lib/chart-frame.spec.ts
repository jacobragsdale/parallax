import { create, fromBinary } from "@bufbuild/protobuf";
import type { MessageInitShape } from "@bufbuild/protobuf";
import { Table, Utf8, tableFromArrays, tableToIPC, vectorFromArray } from "apache-arrow";
import type { Vector } from "apache-arrow";
import { describe, expect, it, vi } from "vitest";

import { decodeChartFrame } from "./chart-frame";
import { Connection } from "./connection";
import type { ChartFrameData } from "./chart-frame";
import { ChartFrameSchema, ClientMessageSchema } from "./proto/parallax_pb";
import type { ChartFrame } from "./proto/parallax_pb";

type Columns = Readonly<Record<string, Float64Array | Uint32Array | BigUint64Array | readonly (string | null)[]>>;

/** A batch as the engine writes it: text columns are plain UTF-8, not dictionaries. */
function batch(columns: Columns): Uint8Array {
  const numeric = tableFromArrays(Object.fromEntries(Object.entries(columns).filter((e): e is [string, Float64Array | Uint32Array | BigUint64Array] => !Array.isArray(e[1]))));
  const vectors: Record<string, Vector> = {};
  for (const [name, values] of Object.entries(columns)) {
    const vector = Array.isArray(values) ? vectorFromArray(values, new Utf8()) : numeric.getChild(name);
    if (vector === null) {
      throw new Error(`no column ${name}`);
    }
    vectors[name] = vector;
  }
  return tableToIPC(new Table(vectors));
}

function chartFrame(columns: Columns, extra: Omit<MessageInitShape<typeof ChartFrameSchema>, "$typeName"> = {}): ChartFrame {
  return create(ChartFrameSchema, { viewId: 3, xFrom: 0, xTo: 10, extentFrom: 0, extentTo: 10, ticks: [{ x: 0, label: "Jan 2" }], batch: batch(columns), ...extra });
}

describe("decodeChartFrame", () => {
  it("splits points into their series, with the engine's text and breaks", () => {
    const frame = chartFrame(
      {
        series: new Uint32Array([0, 0, 0, 1, 1]),
        x: new Float64Array([1, 2, 3, 1, 4]),
        y: new Float64Array([10, Number.NaN, 12, 5, 6]),
        x_text: ["Jan 2", "Jan 3", "Jan 4", "Jan 2", "Jan 5"],
        y_text: ["10.00", null, "12.00", "5.00", "6.00"],
        x_json: ['"2026-01-02"', '"2026-01-03"', '"2026-01-04"', '"2026-01-02"', '"2026-01-05"']
      },
      {
        series: [
          { label: "close AAPL", column: "close" },
          { label: "close MSFT", column: "close", right: true }
        ]
      }
    );
    const decoded = decodeChartFrame(frame);
    expect(decoded.ticks).toEqual([{ x: 0, label: "Jan 2" }]);
    if (decoded.content.kind !== "series") {
      throw new Error("expected series");
    }
    const [aapl, msft] = decoded.content.series;
    expect(aapl?.label).toBe("close AAPL");
    expect(Array.from(aapl?.x ?? [])).toEqual([1, 2, 3]);
    expect(aapl?.yText).toEqual(["10.00", null, "12.00"]);
    expect(Number.isNaN(aapl?.y[1])).toBe(true);
    expect(msft?.right).toBe(true);
    expect(Array.from(msft?.y ?? [])).toEqual([5, 6]);
  });

  it("refuses series out of order, unlisted series, and a density of the wrong size", () => {
    const points = { x: new Float64Array([1, 2]), y: new Float64Array([1, 2]), x_text: ["a", "b"], y_text: ["a", "b"], x_json: ["1", "2"] };
    const series = [
      { label: "a", column: "y" },
      { label: "b", column: "y" }
    ];
    expect(() => decodeChartFrame(chartFrame({ series: new Uint32Array([1, 0]), ...points }, { series }))).toThrow("in order");
    expect(() => decodeChartFrame(chartFrame({ series: new Uint32Array([0, 5]), ...points }, { series }))).toThrow("does not list");
    expect(() => decodeChartFrame(chartFrame({ count: new Uint32Array([1, 2, 3]) }, { columns: 2, rows: 2 }))).toThrow("size");
    expect(() => decodeChartFrame(chartFrame({ series: new Uint32Array([0, 0]), ...points }, { series, extentFrom: 5, extentTo: 1 }))).toThrow("extent");
    expect(() => decodeChartFrame(chartFrame({ series: new Uint32Array([0, 0]), ...points }, { series, extentTo: Number.NaN }))).toThrow("extent");
    expect(() => decodeChartFrame(chartFrame({ series: new Uint32Array([0]), x: new Float64Array([1]), y: ["not a number"], x_text: ["a"], y_text: ["a"], x_json: ["1"] }, { series }))).toThrow(
      "doubles"
    );
  });

  it("reads histograms, densities and candles", () => {
    const histogram = decodeChartFrame(
      chartFrame(
        {
          series: new Uint32Array([0, 0]),
          from: new Float64Array([0, 5]),
          to: new Float64Array([5, 10]),
          count: new BigUint64Array([3n, 4n]),
          from_text: ["0.00", "5.00"],
          to_text: ["5.00", "10.00"]
        },
        { series: [{ label: "beta", column: "beta" }] }
      )
    );
    expect(histogram.content.kind === "histogram" && histogram.content.series[0]?.counts).toEqual([3, 4]);
    const density = decodeChartFrame(chartFrame({ count: new Uint32Array([1, 0, 2, 5]) }, { columns: 2, rows: 2, yFrom: -1, yTo: 1 }));
    expect(density.content.kind === "density" && Array.from(density.content.counts)).toEqual([1, 0, 2, 5]);
    const prices = { open: new Float64Array([1]), high: new Float64Array([2]), low: new Float64Array([0.5]), close: new Float64Array([Number.NaN]) };
    const texts = { open_text: ["1.00"], high_text: ["2.00"], low_text: ["0.50"], close_text: [null], x_json: ['"2026-11-02"'], end_json: ['"2026-11-09"'] };
    const candles = decodeChartFrame(chartFrame({ from: new Float64Array([0]), to: new Float64Array([7]), x_text: ["Nov 2"], ...prices, ...texts }, { interval: "week" }));
    expect(candles.content.kind === "candles" && candles.content.candles.interval).toBe("week");
    expect(candles.content.kind === "candles" && candles.content.candles.closeText).toEqual([null]);
    // Without a volume indicator, candles have no volume.
    expect(candles.content.kind === "candles" && candles.content.candles.volume).toBeNull();
    const withVolume = decodeChartFrame(
      chartFrame(
        { from: new Float64Array([0]), to: new Float64Array([7]), x_text: ["Nov 2"], ...prices, ...texts, volume: new Float64Array([Number.NaN]), volume_text: ["overflow"] },
        { interval: "week" }
      )
    );
    expect(withVolume.content.kind === "candles" && withVolume.content.candles.volumeText).toEqual(["overflow"]);
    expect(withVolume.indicators).toEqual([]);
  });
});

describe("chart windows", () => {
  it("reads indicator lines from their own batch, line by line", () => {
    const points = { x: new Float64Array([1, 2]), y: new Float64Array([1, 2]), x_text: ["a", "b"], y_text: ["a", "b"], x_json: ["1", "2"] };
    const lines = { line: new Uint32Array([0, 0, 1]), x: new Float64Array([1, 2, 2]), y: new Float64Array([Number.NaN, 10.5, 55]), x_text: [null, "b", "b"], y_text: [null, "10.50", "55.00"] };
    const indicatorLines = [
      { indicator: 0, line: "sma", series: "AAPL" },
      { indicator: 1, line: "rsi", series: "AAPL" }
    ];
    const frame = decodeChartFrame(chartFrame({ series: new Uint32Array([0, 0]), ...points }, { series: [{ label: "close AAPL", column: "close" }], indicatorLines, indicatorBatch: batch(lines) }));
    expect(frame.indicators.map((l) => [l.indicator, l.line, l.series, Array.from(l.x), l.yText])).toEqual([
      [0, "sma", "AAPL", [1, 2], [null, "10.50"]],
      [1, "rsi", "AAPL", [2], ["55.00"]]
    ]);
    // A line the batch names but the frame does not list, or lines out of order, are refused.
    expect(() =>
      decodeChartFrame(
        chartFrame({ series: new Uint32Array([0, 0]), ...points }, { series: [{ label: "close", column: "close" }], indicatorLines: indicatorLines.slice(0, 1), indicatorBatch: batch(lines) })
      )
    ).toThrow("does not list");
    const backwards = { ...lines, line: new Uint32Array([1, 0, 0]) };
    expect(() =>
      decodeChartFrame(chartFrame({ series: new Uint32Array([0, 0]), ...points }, { series: [{ label: "close", column: "close" }], indicatorLines, indicatorBatch: batch(backwards) }))
    ).toThrow("in order");
  });

  it("keep one request in flight and send the newest waiting one when it is answered", () => {
    let now = 100;
    const clock = vi.spyOn(performance, "now").mockImplementation(() => now);
    const sent: Uint8Array[] = [];
    const c = new Connection({
      send: (b) => {
        sent.push(b);
      },
      close: () => undefined
    });
    const seen: ChartFrameData[] = [];
    const view = c.open("prices", "{}", {
      frame: () => undefined,
      chart: (f) => {
        seen.push(f);
      }
    });
    view.setChartWindow({ width: 800, height: 400 });
    now = 110;
    view.setChartWindow({ x: [0, 5], width: 800, height: 400 });
    now = 120;
    view.setChartWindow({ x: [0, 6], width: 800, height: 400 });
    const windows = (): unknown[] => sent.map((b) => fromBinary(ClientMessageSchema, b).body).filter((b) => b.case === "setChartWindow");
    expect(windows().length).toBe(1);
    const answer = decodeChartFrame(chartFrame({ count: new Uint32Array([0]) }, { columns: 1, rows: 1, viewId: view.id }));
    // Hello was request 1, opening the view 2, the first window 3.
    view.receiveChartFrame(answer, 3n);
    expect(windows().length).toBe(2);
    expect(windows()[1]).toMatchObject({ value: { xFrom: 0, xTo: 6, width: 800 } });
    view.receiveChartFrame(answer, 4n);
    expect(seen.map((f) => f.requestedAtMs)).toEqual([100, 120]);
    // Zoom to A, pan to B, pan back to A while A is in flight: the chart ends on A.
    view.setChartWindow({ x: [1, 2], width: 800, height: 400 });
    view.setChartWindow({ x: [3, 4], width: 800, height: 400 });
    view.setChartWindow({ x: [1, 2], width: 800, height: 400 });
    view.receiveChartFrame(answer, 5n);
    expect(windows().length).toBe(3);
    clock.mockRestore();
  });
});
