/**
 * Chart frames: a chart the engine reduced to its plot, decoded from Arrow IPC.
 *
 * A chart frame crosses an I/O boundary, so each column is checked against the
 * type the protocol promises before drawing code reads it. Positions arrive as
 * doubles on the axis; every value a tooltip shows arrives as the engine's text,
 * formatted as grid cells are, so the chart never formats a number itself.
 */

import { DataType, Table, tableFromIPC } from "apache-arrow";
import type { Vector } from "apache-arrow";

import { FrameError } from "./frame";
import type { ChartFrame } from "./proto/parallax_pb";

/** A labelled tick on the x axis. */
export interface ChartTickData {
  readonly x: number;
  readonly label: string;
}

/** A line or area series: points in order, with NaN y values at breaks. */
export interface SeriesData {
  readonly label: string;
  readonly column: string;
  /** Drawn against the right y axis. */
  readonly right: boolean;
  readonly x: Float64Array;
  readonly y: Float64Array;
  readonly xText: readonly (string | null)[];
  readonly yText: readonly (string | null)[];
  /** Each point's x value as exact JSON, for filters that select a range. */
  readonly xJson: readonly (string | null)[];
}

/** One series of a histogram: a count per bin. */
export interface HistogramSeries {
  readonly label: string;
  readonly from: Float64Array;
  readonly to: Float64Array;
  readonly counts: readonly number[];
  readonly fromText: readonly (string | null)[];
  readonly toText: readonly (string | null)[];
}

/** Candlestick or OHLC buckets; a missing price is NaN, with null text. */
export interface CandleData {
  readonly interval: string;
  readonly from: Float64Array;
  readonly to: Float64Array;
  readonly open: Float64Array;
  readonly high: Float64Array;
  readonly low: Float64Array;
  readonly close: Float64Array;
  readonly xText: readonly (string | null)[];
  readonly openText: readonly (string | null)[];
  readonly highText: readonly (string | null)[];
  readonly lowText: readonly (string | null)[];
  readonly closeText: readonly (string | null)[];
  /** Each bucket's start, and the next bucket's start, as exact JSON. */
  readonly xJson: readonly (string | null)[];
  readonly endJson: readonly (string | null)[];
}

/** What a chart frame holds. */
export type ChartContent =
  | { readonly kind: "series"; readonly series: readonly SeriesData[] }
  | { readonly kind: "density"; readonly columns: number; readonly rows: number; readonly yFrom: number; readonly yTo: number; readonly counts: Uint32Array }
  | { readonly kind: "histogram"; readonly series: readonly HistogramSeries[] }
  | { readonly kind: "candles"; readonly candles: CandleData };

/** A decoded, checked chart frame. */
export interface ChartFrameData {
  readonly viewId: number;
  readonly configVersion: bigint;
  readonly loadVersion: bigint;
  readonly sourceAsOfMicros: bigint;
  readonly pushed: boolean;
  /** The x range shown, in axis positions. */
  readonly xFrom: number;
  readonly xTo: number;
  /** The data's whole x range, in axis positions, which zooming and panning stay within. */
  readonly extentFrom: number;
  readonly extentTo: number;
  readonly ticks: readonly ChartTickData[];
  readonly content: ChartContent;
  /** When the chart window this frame answers was asked for, by `performance.now()`. */
  readonly requestedAtMs?: number;
}

/** The columns of a chart batch, by name. */
class Columns {
  constructor(private readonly table: Table) {}

  public has(name: string): boolean {
    return this.table.schema.fields.some((f) => f.name === name);
  }

  private vector(name: string): Vector {
    const index = this.table.schema.fields.findIndex((f) => f.name === name);
    const vector = index < 0 ? null : this.table.getChildAt(index);
    if (vector === null) {
      throw new FrameError(`a chart frame lacks its ${name} column`);
    }
    return vector;
  }

  public floats(name: string): Float64Array {
    const vector = this.vector(name);
    const values: unknown = vector.toArray();
    if (!DataType.isFloat(vector.type) || !(values instanceof Float64Array)) {
      throw new FrameError(`a chart frame's ${name} column must hold doubles`);
    }
    return values;
  }

  public counts32(name: string): Uint32Array {
    const vector = this.vector(name);
    const values: unknown = vector.toArray();
    if (!DataType.isInt(vector.type) || !(values instanceof Uint32Array)) {
      throw new FrameError(`a chart frame's ${name} column must hold 32-bit counts`);
    }
    return values;
  }

  public counts64(name: string): readonly number[] {
    const vector = this.vector(name);
    const values: unknown = vector.toArray();
    if (!DataType.isInt(vector.type) || !(values instanceof BigUint64Array)) {
      throw new FrameError(`a chart frame's ${name} column must hold 64-bit counts`);
    }
    return Array.from(values, (v) => {
      if (v > BigInt(Number.MAX_SAFE_INTEGER)) {
        throw new FrameError(`a chart frame's ${name} column holds a count beyond 2^53`);
      }
      return Number(v);
    });
  }

  public texts(name: string): readonly (string | null)[] {
    const vector = this.vector(name);
    if (!DataType.isUtf8(vector.type)) {
      throw new FrameError(`a chart frame's ${name} column must hold text`);
    }
    return Array.from({ length: vector.length }, (_, i): string | null => {
      const value: unknown = vector.get(i);
      return typeof value === "string" ? value : null;
    });
  }

  public get rows(): number {
    return this.table.numRows;
  }
}

/** The contiguous rows of each series id, which the engine sends series by series. */
function runs(ids: Uint32Array, count: number): readonly (readonly [number, number])[] {
  const out: [number, number][] = Array.from({ length: count }, () => [0, 0]);
  let start = 0;
  for (let i = 1; i <= ids.length; i += 1) {
    const id = ids[start];
    if (i < ids.length && ids[i] === id) {
      continue;
    }
    if (id === undefined || id >= count) {
      throw new FrameError("a chart frame names a series it does not list");
    }
    if (i < ids.length && (ids[i] ?? 0) <= id) {
      throw new FrameError("a chart frame's series must come in order");
    }
    out[id] = [start, i];
    start = i;
  }
  return out;
}

function decodeContent(frame: ChartFrame, columns: Columns): ChartContent {
  if (columns.has("y")) {
    const ids = columns.counts32("series");
    const [x, y, xText, yText, xJson] = [columns.floats("x"), columns.floats("y"), columns.texts("x_text"), columns.texts("y_text"), columns.texts("x_json")];
    const spans = runs(ids, frame.series.length);
    return {
      kind: "series",
      series: frame.series.map((s, i) => {
        const [from, to] = spans[i] ?? [0, 0];
        return {
          label: s.label,
          column: s.column,
          right: s.right,
          x: x.subarray(from, to),
          y: y.subarray(from, to),
          xText: xText.slice(from, to),
          yText: yText.slice(from, to),
          xJson: xJson.slice(from, to)
        };
      })
    };
  }
  if (columns.has("open")) {
    return {
      kind: "candles",
      candles: {
        interval: frame.interval,
        from: columns.floats("from"),
        to: columns.floats("to"),
        open: columns.floats("open"),
        high: columns.floats("high"),
        low: columns.floats("low"),
        close: columns.floats("close"),
        xText: columns.texts("x_text"),
        openText: columns.texts("open_text"),
        highText: columns.texts("high_text"),
        lowText: columns.texts("low_text"),
        closeText: columns.texts("close_text"),
        xJson: columns.texts("x_json"),
        endJson: columns.texts("end_json")
      }
    };
  }
  if (columns.has("from")) {
    const ids = columns.counts32("series");
    const [from, to, counts, fromText, toText] = [columns.floats("from"), columns.floats("to"), columns.counts64("count"), columns.texts("from_text"), columns.texts("to_text")];
    const spans = runs(ids, frame.series.length);
    return {
      kind: "histogram",
      series: frame.series.map((s, i) => {
        const [a, b] = spans[i] ?? [0, 0];
        return { label: s.label, from: from.subarray(a, b), to: to.subarray(a, b), counts: counts.slice(a, b), fromText: fromText.slice(a, b), toText: toText.slice(a, b) };
      })
    };
  }
  const counts = columns.counts32("count");
  if (counts.length !== frame.columns * frame.rows) {
    throw new FrameError("a density's cells do not match its size");
  }
  return { kind: "density", columns: frame.columns, rows: frame.rows, yFrom: frame.yFrom, yTo: frame.yTo, counts };
}

function isRange(from: number, to: number): boolean {
  return Number.isFinite(from) && Number.isFinite(to) && to >= from;
}

/** Decodes and checks a chart frame. */
export function decodeChartFrame(frame: ChartFrame): ChartFrameData {
  const decoded = tableFromIPC(frame.batch);
  if (!(decoded instanceof Table)) {
    throw new FrameError("a chart frame's batch must decode synchronously");
  }
  if (!isRange(frame.xFrom, frame.xTo)) {
    throw new FrameError("a chart frame's x range is not a range");
  }
  if (!isRange(frame.extentFrom, frame.extentTo)) {
    throw new FrameError("a chart frame's x extent is not a range");
  }
  return {
    viewId: frame.viewId,
    configVersion: frame.configVersion,
    loadVersion: frame.loadVersion,
    sourceAsOfMicros: frame.sourceAsOfMicros,
    pushed: frame.pushed,
    xFrom: frame.xFrom,
    xTo: frame.xTo,
    extentFrom: frame.extentFrom,
    extentTo: frame.extentTo,
    ticks: frame.ticks.map((t) => ({ x: t.x, label: t.label })),
    content: decodeContent(frame, new Columns(decoded))
  };
}
