import { signal } from "@angular/core";

import type { Measurement } from "../connection";
import { describe, expect, it } from "vitest";

import type { CandleData, ChartFrameData, SeriesData } from "../chart-frame";
import { parseLayout } from "../layout";
import type { LayoutChart, LayoutDrawing } from "../layout";
import { CHART_COLORS } from "../theme";
import { DrawingEditor, measureView } from "./drawing-editor";
import { anchorPosition, drawingAt, drawingFrom, handleAt, moved, seriesAt, shapesOf, snap } from "./drawings";
import type { Shape } from "./drawings";
import { chartModel } from "./model";
import type { ChartModel } from "./model";

/** Ten daily candles, day 0 to day 9, priced from 100 to 109. */
const DAYS = Array.from({ length: 10 }, (_, d) => d);
const CANDLES: CandleData = {
  interval: "day",
  from: Float64Array.from(DAYS),
  to: Float64Array.from(DAYS.map((d) => d + 1)),
  open: Float64Array.from(DAYS.map((d) => 100 + d)),
  high: Float64Array.from(DAYS.map((d) => 101 + d)),
  low: Float64Array.from(DAYS.map((d) => 99 + d)),
  close: Float64Array.from(DAYS.map((d) => 100.5 + d)),
  xText: DAYS.map((d) => `day ${String(d)}`),
  openText: DAYS.map(String),
  highText: DAYS.map(String),
  lowText: DAYS.map(String),
  closeText: DAYS.map(String),
  volume: null,
  volumeText: null,
  xJson: DAYS.map((d) => JSON.stringify(`2026-01-${String(d + 1).padStart(2, "0")}`)),
  endJson: DAYS.map((d) => JSON.stringify(`2026-01-${String(d + 2).padStart(2, "0")}`))
};

function chart(drawings: readonly unknown[] = [], logY = false): LayoutChart {
  const layout = parseLayout({ chart: { kind: "candlestick", x: "date", log_y: logY, ohlc: { open: "o", high: "h", low: "l", close: "c" }, drawings } });
  if (layout.chart === null) {
    throw new Error("no chart");
  }
  return layout.chart;
}

function frame(anchors: ReadonlyMap<string, number> = new Map()): ChartFrameData {
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
    indicators: [],
    anchors,
    events: [],
    eventsTotal: 0
  };
}

function model(c: LayoutChart, f: ChartFrameData): ChartModel {
  return chartModel({ chart: c, width: 500, height: 300, colors: CHART_COLORS.light, muted: "#999999", measure: (t) => t.length * 6, frame: f, categories: null });
}

const measure = (t: string): number => t.length * 6;

describe("drawings placed on the chart", () => {
  it("finds a number anchor however its JSON writes it", () => {
    const numbers: ChartFrameData = {
      ...frame(new Map([["2", 40]])),
      content: { kind: "series", series: [{ label: "y", column: "y", right: false, x: Float64Array.from([7]), y: Float64Array.from([1]), xText: ["7"], yText: ["1"], xJson: ["7.0"] }] }
    };
    // The engine writes 2.0 as "2.0"; the anchor holds the number 2 either way.
    expect(anchorPosition(numbers, 2)).toBe(40);
    expect(anchorPosition(numbers, 7)).toBe(7);
  });

  it("places an anchor where the engine says, or at the frame's point it was snapped to", () => {
    // The engine placed an anchor on day 20, past this frame's points.
    const f = frame(
      new Map([
        ['"2026-01-21"', 20],
        ['"holiday"', Number.NaN]
      ])
    );
    expect(anchorPosition(f, "2026-01-21")).toBe(20);
    expect(anchorPosition(f, "2026-01-04")).toBe(3);
    expect(anchorPosition(f, "holiday")).toBeNull();
    expect(anchorPosition(f, "2027-01-01")).toBeNull();
  });

  it("draws each kind where its anchors are, with Fibonacci's levels from the move's end back to its start", () => {
    const drawings = [
      { kind: "trend", from: { x: "2026-01-01", y: 100 }, to: { x: "2026-01-10", y: 109 } },
      { kind: "horizontal", y: 105 },
      { kind: "fibonacci", from: { x: "2026-01-02", y: 100 }, to: { x: "2026-01-06", y: 110 } },
      { kind: "note", at: { x: "2026-01-10", y: 104 }, text: "A long note near the edge" },
      { kind: "rectangle", from: { x: "2026-01-21", y: 100 }, to: { x: "2026-01-03", y: 104 } }
    ];
    const c = chart(drawings);
    const f = frame();
    const m = model(c, f);
    const shapes = shapesOf(c.drawings, f, m, measure);
    // The rectangle's corner on day 20 is in no frame and not placed by the engine yet: it is left out.
    expect(shapes.map((s) => s.kind)).toEqual(["trend", "horizontal", "fibonacci", "note"]);
    const [x, y] = [m.x, m.yAxes[0]?.scale];
    if (x === null || y === undefined) {
      throw new Error("no scales");
    }
    const fib = shapes[2];
    const levels = fib?.kind === "fibonacci" ? fib.levels.map((l) => y.invert(l.y)) : [];
    const expected = [110, 110 - 2.36, 110 - 3.82, 105, 110 - 6.18, 110 - 7.86, 100];
    expect(levels.length).toBe(expected.length);
    levels.forEach((level, i) => {
      expect(level).toBeCloseTo(expected[i] ?? Number.NaN, 9);
    });
    // A note near the right edge reads to the left of its point.
    const note = shapes[3];
    expect(note?.kind === "note" && note.box.x + note.box.w < note.at.x).toBe(true);
    // Below zero on a log scale, a drawing cannot be placed.
    const log = chart([{ kind: "horizontal", y: -1 }], true);
    expect(shapesOf(log.drawings, f, model(log, f), measure)).toEqual([]);
  });

  it("picks the drawing nearest the pointer, its handles, and a rectangle from inside", () => {
    const shapes: Shape[] = [
      { kind: "trend", index: 0, a: { x: 0, y: 0 }, b: { x: 100, y: 100 } },
      { kind: "rectangle", index: 1, a: { x: 200, y: 0 }, b: { x: 300, y: 50 } }
    ];
    expect(drawingAt(shapes, { x: 50, y: 54 })).toBe(0);
    expect(drawingAt(shapes, { x: 50, y: 60 })).toBeNull();
    expect(drawingAt(shapes, { x: 250, y: 25 })).toBe(1);
    const [trend] = shapes;
    if (trend === undefined) {
      throw new Error("no trend");
    }
    expect(handleAt(trend, { x: 98, y: 103 })).toBe(1);
    expect(handleAt(trend, { x: 50, y: 50 })).toBeNull();
  });

  it("snaps an anchor to the nearest candle's exact x and the price a pixel tells apart", () => {
    const f = frame();
    const m = model(chart(), f);
    const [x, y] = [m.x, m.yAxes[0]?.scale];
    if (x === null || y === undefined) {
      throw new Error("no scales");
    }
    const at = snap(f, m, { x: x.map(4.4), y: y.map(104.123456) });
    expect(at?.anchor.x).toBe("2026-01-05");
    expect(at?.point.x).toBeCloseTo(x.map(4));
    // A pixel here is worth a few hundredths, so the price keeps two decimals.
    const perPixel = Math.abs(y.invert(0) - y.invert(1));
    expect(perPixel).toBeGreaterThan(0.01);
    expect(perPixel).toBeLessThan(0.1);
    expect(at?.anchor.y).toBe(104.12);
  });

  it("makes each kind from two snaps, and moves one anchor at a time", () => {
    const a = { anchor: { x: "2026-01-01", y: 100 }, point: { x: 0, y: 0 } };
    const b = { anchor: { x: "2026-01-05", y: 104 }, point: { x: 40, y: 40 } };
    expect(drawingFrom("horizontal", a, b)).toEqual({ kind: "horizontal", y: 100 });
    expect(drawingFrom("note", a, b, "Hi")).toEqual({ kind: "note", at: a.anchor, text: "Hi" });
    const fib = drawingFrom("fibonacci", a, b);
    expect(fib).toEqual({ kind: "fibonacci", from: a.anchor, to: b.anchor });
    expect(moved(fib, 1, a)).toEqual({ kind: "fibonacci", from: a.anchor, to: a.anchor });
    expect(moved({ kind: "horizontal", y: 1 }, 0, b)).toEqual({ kind: "horizontal", y: 104 });
  });
});

describe("the drawing editor", () => {
  const MEASURED: Measurement = { fromX: "day 2", toX: "day 6", fromPosition: 2, toPosition: 6, fromValue: "102.50", toValue: "106.50", change: "4.00", percent: "3.90%", rows: 4 };

  function editor(start: readonly LayoutDrawing[] = []): {
    readonly edit: DrawingEditor;
    readonly saved: LayoutDrawing[][];
    readonly m: ChartModel;
    readonly drawings: ReturnType<typeof signal<readonly LayoutDrawing[]>>;
    readonly asked: (readonly [number, number])[];
  } {
    const f = frame();
    const m = model(chart(), f);
    const drawings = signal<readonly LayoutDrawing[]>(start);
    const saved: LayoutDrawing[][] = [];
    const asked: (readonly [number, number])[] = [];
    const edit = new DrawingEditor({
      model: signal(m),
      frame: signal(f),
      drawings,
      save: (d) => {
        saved.push([...d]);
        drawings.set(d);
      },
      measure,
      hidden: signal([]),
      measureChart: (from, to) => {
        asked.push([from, to]);
        return Promise.resolve(MEASURED);
      }
    });
    return { edit, saved, m, drawings, asked };
  }
  const point = (m: ChartModel, day: number, price: number): { x: number; y: number } => ({ x: m.x?.map(day) ?? 0, y: m.yAxes[0]?.scale.map(price) ?? 0 });

  it("makes a trend line by pressing, dragging and letting go, then selects it", () => {
    const { edit, saved, m } = editor();
    edit.pick("trend");
    expect(edit.down(point(m, 1, 101))).toBe(true);
    edit.move(point(m, 5, 105));
    // While dragging, the line is drawn but not saved.
    expect(edit.shown().length).toBe(1);
    expect(saved).toEqual([]);
    expect(edit.up()).toBe(true);
    expect(saved).toEqual([[{ kind: "trend", from: { x: "2026-01-02", y: 101 }, to: { x: "2026-01-06", y: 105 } }]]);
    expect(edit.tool()).toBeNull();
    expect(edit.selected()).toBe(0);
  });

  it("makes nothing from a press without a drag, or below the price's plot", () => {
    const { edit, saved, m } = editor();
    edit.pick("rectangle");
    edit.down(point(m, 3, 103));
    edit.up();
    expect(saved).toEqual([]);
    // Under the plot, in its axis or a pane, a press is not a drawing.
    expect(edit.down({ x: m.x?.map(3) ?? 0, y: m.plot.y + m.plot.h + 10 })).toBe(false);
    // A drag past the plot holds the anchor at its edge.
    edit.down(point(m, 3, 103));
    edit.move({ x: m.x?.map(6) ?? 0, y: m.plot.y + m.plot.h + 200 });
    edit.up();
    const bottom = m.yAxes[0]?.scale.invert(m.plot.y + m.plot.h) ?? Number.NaN;
    const last = saved.at(-1)?.at(-1);
    expect(last?.kind === "rectangle" ? last.to.y : null).toBeCloseTo(bottom, 1);
  });

  it("selects a drawing, moves its anchor by its handle, and deletes it", () => {
    const { edit, saved, m } = editor([{ kind: "trend", from: { x: "2026-01-02", y: 101 }, to: { x: "2026-01-06", y: 105 } }]);
    // A press on the line selects it and is not a pan; a press on nothing clears the selection.
    expect(edit.down(point(m, 3, 103))).toBe(true);
    expect(edit.selected()).toBe(0);
    edit.up();
    expect(edit.down(point(m, 5, 105))).toBe(true);
    edit.move(point(m, 8, 107));
    edit.up();
    expect(saved.at(-1)).toEqual([{ kind: "trend", from: { x: "2026-01-02", y: 101 }, to: { x: "2026-01-09", y: 107 } }]);
    expect(edit.remove()).toBe(true);
    expect(saved.at(-1)).toEqual([]);
    expect(edit.down(point(m, 3, 109))).toBe(false);
  });

  it("places a note once its text is typed, and drops it on Escape", () => {
    const { edit, saved, m } = editor();
    edit.pick("note");
    edit.down(point(m, 2, 102));
    expect(edit.note()?.anchor.x).toBe("2026-01-03");
    // The tool is down while the text is typed: a press elsewhere does not move the note.
    expect(edit.tool()).toBeNull();
    edit.down(point(m, 7, 107));
    expect(edit.note()?.anchor.x).toBe("2026-01-03");
    edit.noteDone(null);
    expect(saved).toEqual([]);
    edit.pick("note");
    edit.down(point(m, 2, 102));
    edit.noteDone("Breakout");
    expect(saved).toEqual([[{ kind: "note", at: { x: "2026-01-03", y: 102 }, text: "Breakout" }]]);
  });

  it("forgets a selection the layout no longer holds, and a drag the browser cancelled", () => {
    const first: LayoutDrawing = { kind: "horizontal", y: 101 };
    const second: LayoutDrawing = { kind: "horizontal", y: 105 };
    const { edit, saved, m, drawings } = editor([first, second]);
    edit.down(point(m, 3, 105));
    expect(edit.selected()).toBe(1);
    // The host reorders the drawings: the place now holds another drawing, which Delete must not remove.
    drawings.set([second, first]);
    expect(edit.selected()).toBeNull();
    expect(edit.remove()).toBe(false);
    expect(saved).toEqual([]);
    // A drag cancelled by the browser makes nothing, even when a later press lets go.
    edit.pick("trend");
    edit.down(point(m, 1, 101));
    edit.move(point(m, 5, 105));
    edit.abandon();
    expect(edit.up()).toBe(false);
    expect(saved).toEqual([]);
  });

  it("measures across a band dragged: asks the engine at the snapped places and shows its answer", async () => {
    const { edit, saved, m, asked } = editor();
    edit.pick("measure");
    edit.down(point(m, 2, 102));
    edit.move(point(m, 6, 106));
    expect(edit.measuring()?.state).toBe("dragging");
    edit.up();
    expect(edit.measuring()?.state).toBe("asking");
    await Promise.resolve();
    expect(edit.measuring()).toMatchObject({ state: "measured", measurement: MEASURED, series: "" });
    expect(asked).toEqual([[2, 6]]);
    // Drawn on another scale, as after a zoom, the band is where its days are now, and its card inside the plot.
    const measuring = edit.measuring();
    if (measuring === null) {
      throw new Error("no measurement");
    }
    const zoomed = chartModel({ chart: chart(), width: 500, height: 300, colors: CHART_COLORS.light, muted: "#999999", measure, frame: { ...frame(), xFrom: 4, xTo: 8 }, categories: null });
    const view = measureView(measuring, zoomed);
    expect(view?.band.x).toBeCloseTo(zoomed.x?.map(2) ?? Number.NaN);
    expect(view?.lines.map((l) => l.value)).toEqual(["4.00 (3.90%)", "4", "102.50 → 106.50", "day 2 → day 6"]);
    expect((view?.card.x ?? 0) + 220).toBeLessThanOrEqual(zoomed.plot.x + zoomed.plot.w);
    // A measurement is not a drawing, and a press puts it away.
    expect(saved).toEqual([]);
    edit.down(point(m, 8, 108));
    expect(edit.measuring()).toBeNull();
  });

  it("puts the tool down and drops the selection on Escape", () => {
    const { edit } = editor([{ kind: "horizontal", y: 104 }]);
    edit.pick("fibonacci");
    expect(edit.cancel()).toBe(true);
    expect(edit.tool()).toBeNull();
    expect(edit.cancel()).toBe(false);
  });
});

describe("the series a measurement measures", () => {
  it("is the shown series whose line is nearest the press", () => {
    const series = (label: string, base: number): SeriesData => ({
      label,
      column: "close",
      right: false,
      x: Float64Array.from(DAYS),
      y: Float64Array.from(DAYS.map((d) => base + d)),
      xText: DAYS.map(String),
      yText: DAYS.map(String),
      xJson: DAYS.map(String)
    });
    const layout = parseLayout({ chart: { kind: "line", x: "date", y: ["close"], series: "ticker" } }).chart;
    if (layout === null) {
      throw new Error("no chart");
    }
    const f: ChartFrameData = { ...frame(), content: { kind: "series", series: [series("close AAPL", 100), series("close MSFT", 200)] } };
    const m = model(layout, f);
    const near = (price: number): { x: number; y: number } => ({ x: m.x?.map(4) ?? 0, y: m.yAxes[0]?.scale.map(price) ?? 0 });
    expect(seriesAt(f, m, near(195), [])).toEqual({ label: "close MSFT", key: "MSFT" });
    expect(seriesAt(f, m, near(105), [])).toEqual({ label: "close AAPL", key: "AAPL" });
    // A hidden series is never measured.
    expect(seriesAt(f, m, near(195), ["close MSFT"])).toEqual({ label: "close AAPL", key: "AAPL" });
  });
});
