/**
 * Spike 0.3: parallax with no server.
 *
 * The page stands in for an app: it gets Arrow IPC bytes (generated in a worker
 * of their own, as an API response would arrive), moves them to parallax's
 * engine worker, opens pivots through the library's `Connection`, and saves
 * exports as Blob downloads. Playwright drives it through `window.spike` on the
 * build host; on a Citrix desktop, the Run button runs the same steps and shows a
 * report to copy.
 */

import { BrowserEngine } from "../projects/parallax/src/lib/browser-engine";
import type { WorkerPort, WorkerStats } from "../projects/parallax/src/lib/browser-engine";
import type { Connection, Viewport, ViewHandle } from "../projects/parallax/src/lib/connection";
import type { FrameData } from "../projects/parallax/src/lib/frame";

/** Fixed load times, so that native and WebAssembly replies can be compared byte for byte. */
const NOW_MICROS = 1_761_840_000_000_000;
const AS_OF_MICROS = 1_761_836_400_000_000;

export const LAYOUTS = {
  /** Three sums by fund. */
  fund: `{"version":2,"dataset":"positions","group_by":["fund"],"measures":[{"column":"market_value_usd","aggregate":"sum"},{"column":"daily_pnl_usd","aggregate":"sum"},{"column":"weight_in_fund","aggregate":"sum"}]}`,
  /** The roadmap's reference pivot: fund, strategy, sector and ticker by asset class. */
  reference: `{"version":2,"dataset":"positions","group_by":["fund","strategy","sector","ticker"],"split_by":["asset_class"],"measures":[{"column":"market_value_usd","aggregate":"sum"},{"column":"daily_pnl_usd","aggregate":"sum"},{"column":"weight_in_fund","aggregate":"sum"}],"group_sort":{"by":{"measure":0},"descending":true}}`,
  /** A regroup: sector and country by currency, with a mean and a count. */
  regroup: `{"version":2,"dataset":"positions","group_by":["sector","country"],"split_by":["currency"],"measures":[{"column":"notional_usd","aggregate":"sum"},{"column":"beta","aggregate":"mean"},{"aggregate":"count_all"}]}`,
  /** A pivot small enough to export with Excel outlines, expanded two levels. */
  outlined: `{"version":2,"dataset":"positions","group_by":["fund","strategy","sector"],"split_by":["long_short"],"measures":[{"column":"market_value_usd","aggregate":"sum"},{"column":"daily_pnl_usd","aggregate":"sum"},{"column":"beta","aggregate":"mean"},{"aggregate":"count_all"}],"expand_depth":2,"formats":{"market_value_usd":{"abbreviate":"millions","negative":"parentheses"},"beta":{"decimals":3}}}`,
  /** Every column, flat, sorted by market value. */
  flat: `{"version":2,"dataset":"positions","sort":[{"column":"market_value_usd","descending":true}]}`
} as const;

export type LayoutName = keyof typeof LAYOUTS;

/** The window a grid of about 60 rows and 15 columns asks for. */
const WINDOW: Viewport = { firstRow: 0, rowCount: 60, firstColumn: 0, columnCount: 15 };

/** Messages to and from the engine, recorded for the native comparison. */
const tap = { recording: false, outbound: [] as Uint8Array[], replies: [] as Uint8Array[] };

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null;
}

/** The engine's worker, with its protocol messages recorded while `tap.recording` is on. */
function tappedWorker(url: string): WorkerPort {
  const worker = new Worker(new URL(url, location.href), { type: "module", name: "parallax" });
  return {
    postMessage: (message, options) => {
      if (tap.recording && isRecord(message) && message["kind"] === "message" && message["bytes"] instanceof Uint8Array) {
        tap.outbound.push(message["bytes"].slice());
      }
      worker.postMessage(message, options);
    },
    addEventListener: (type, listener) => {
      worker.addEventListener(type, (event) => {
        if (tap.recording && event instanceof MessageEvent && isRecord(event.data) && event.data["kind"] === "reply" && event.data["bytes"] instanceof Uint8Array) {
          tap.replies.push(event.data["bytes"].slice());
        }
        listener(event);
      });
    },
    terminate: () => {
      worker.terminate();
    }
  };
}

let engine: BrowserEngine | null = null;
let held: ArrayBuffer | null = null;
const views: ViewHandle[] = [];

function connection(): Connection {
  if (engine === null) {
    throw new Error("start the engine first");
  }
  return engine.connection;
}

/** Starts a fresh engine worker, ending any earlier one so that its memory is released. */
export function startEngine(): void {
  engine?.connection.close();
  views.length = 0;
  engine = new BrowserEngine(tappedWorker("wasm/worker.js"));
}

/** Waits until the engine has started and said hello. */
export async function ready(): Promise<void> {
  await connection().dataset("positions");
}

/** The longest gap between animation frames while the probe ran: how long the page's main thread was blocked. */
const probe = { on: false, last: 0, maxGapMs: 0, frames: 0 };

export function probeStart(): void {
  Object.assign(probe, { on: true, last: performance.now(), maxGapMs: 0, frames: 0 });
  const tick = (now: number): void => {
    if (!probe.on) {
      return;
    }
    probe.maxGapMs = Math.max(probe.maxGapMs, now - probe.last);
    probe.last = now;
    probe.frames += 1;
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

export function probeStop(): { readonly maxGapMs: number; readonly frames: number } {
  probe.on = false;
  return { maxGapMs: probe.maxGapMs, frames: probe.frames };
}

export interface Generated {
  readonly rows: number;
  readonly bytes: number;
  readonly ms: number;
  readonly sha256: string | null;
}

/** Generates `rows` positions in a worker of their own and holds the bytes, as an app holds its API's response. */
export function generate(rows: number, seed = 42, hash = false): Promise<Generated> {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL("datagen/datagen-worker.js", location.href), { type: "module", name: "datagen" });
    worker.addEventListener("message", (event: MessageEvent<unknown>) => {
      worker.terminate();
      const data = event.data;
      if (isRecord(data) && data["bytes"] instanceof ArrayBuffer) {
        held = data["bytes"];
        resolve({ rows, bytes: held.byteLength, ms: Number(data["ms"]), sha256: typeof data["sha256"] === "string" ? data["sha256"] : null });
      } else {
        reject(new Error(isRecord(data) ? String(data["error"]) : "the generator failed"));
      }
    });
    worker.addEventListener("error", (event) => {
      worker.terminate();
      reject(new Error(`the generator did not start: ${event.message}`));
    });
    worker.postMessage({ rows, seed, hash });
  });
}

export interface LoadResult {
  readonly rows: number;
  /** From posting the bytes to the worker's answer, on the page's clock. */
  readonly ms: number;
  /** The engine's decode and load, on the worker's clock. */
  readonly engineMs: number;
  readonly wasmBytes: number;
  /** How long `postMessage` blocked the page. */
  readonly postMs: number;
}

/** Moves the held bytes to the engine and loads them as `positions`. */
export async function load(primaryKey: string | null = "position_id", inPlace = false): Promise<LoadResult> {
  if (engine === null || held === null) {
    throw new Error("start the engine and generate data first");
  }
  const bytes = held;
  held = null;
  const started = performance.now();
  const loading = engine.load("positions", bytes, { primaryKey, nowMicros: NOW_MICROS, sourceAsOfMicros: AS_OF_MICROS, inPlace });
  const posted = performance.now();
  const loaded = await loading;
  return { rows: loaded.rows, ms: performance.now() - started, engineMs: loaded.ms, wasmBytes: loaded.wasmBytes, postMs: posted - started };
}

export interface PivotResult {
  readonly layout: string;
  /** From opening the view, or sending the update, to its first frame decoded on the page. */
  readonly ms: number;
  readonly totalRows: number;
  readonly totalColumns: number;
}

function summary(name: string, ms: number, frame: FrameData): PivotResult {
  return { layout: name, ms, totalRows: frame.totalRows, totalColumns: frame.totalColumns };
}

/** Closes every open view, so that the engine holds only the next one's tree. */
function closeViews(): void {
  for (const view of views.splice(0)) {
    view.close();
  }
}

/** Opens a view of `positions` with a layout, closing earlier ones, and waits for its first window. */
export function open(name: LayoutName): Promise<PivotResult> {
  closeViews();
  return new Promise((resolve, reject) => {
    const started = performance.now();
    let done = false;
    const view = connection().open("positions", LAYOUTS[name], {
      frame: (frame) => {
        if (!done) {
          done = true;
          resolve(summary(name, performance.now() - started, frame));
        }
      },
      error: (message) => {
        if (!done) {
          done = true;
          reject(new Error(message));
        }
      }
    });
    views.push(view);
    view.setViewport(WINDOW);
  });
}

/** Changes an open view's layout in place, through `ViewHandle.update`, and times the frame answering it. */
export function relayout(name: LayoutName): Promise<PivotResult> {
  const conn = connection();
  closeViews();
  return new Promise((resolve, reject) => {
    let started = 0;
    let phase: "opening" | "updating" | "done" = "opening";
    const view = conn.open("positions", LAYOUTS.fund, {
      frame: (frame) => {
        if (phase === "opening") {
          phase = "updating";
          started = performance.now();
          view.update(LAYOUTS[name], () => WINDOW);
        } else if (phase === "updating" && frame.changedAtMs !== undefined) {
          phase = "done";
          resolve(summary(name, performance.now() - started, frame));
        }
      },
      error: (message) => {
        if (phase !== "done") {
          phase = "done";
          reject(new Error(message));
        }
      }
    });
    views.push(view);
    view.setViewport(WINDOW);
  });
}

export interface ExportResult {
  readonly format: string;
  readonly layout: string;
  /** From asking the worker to the download starting, on the page's clock. */
  readonly ms: number;
  readonly engineMs: number;
  readonly bytes: number;
  readonly wasmBytes: number;
}

/** Exports a layout's view in the worker and saves it as a Blob download. */
export async function exportView(name: LayoutName, format: "xlsx" | "csv"): Promise<ExportResult> {
  if (engine === null) {
    throw new Error("start the engine first");
  }
  const started = performance.now();
  const result = await engine.export(LAYOUTS[name], format);
  const type = format === "xlsx" ? "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" : "text/csv";
  const blob = new Blob([result.bytes as Uint8Array<ArrayBuffer>], { type });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = `positions-${name}.${format}`;
  document.body.append(link);
  link.click();
  link.remove();
  setTimeout(() => {
    URL.revokeObjectURL(url);
  }, 60_000);
  return { format, layout: name, ms: performance.now() - started, engineMs: result.ms, bytes: result.bytes.byteLength, wasmBytes: result.wasmBytes };
}

/** The engine's memory and start-up times, with the page's JavaScript heap where Chromium reports it. */
export async function stats(): Promise<WorkerStats & { readonly pageHeapBytes: number | null; readonly pageTimeOrigin: number }> {
  if (engine === null) {
    throw new Error("start the engine first");
  }
  const memory = (performance as Performance & { memory?: { usedJSHeapSize: number } }).memory;
  return { ...(await engine.stats()), pageHeapBytes: memory?.usedJSHeapSize ?? null, pageTimeOrigin: performance.timeOrigin };
}

function base64(chunks: readonly Uint8Array[]): string {
  // Each message is length-prefixed, little-endian, as the native replay reads them.
  const total = chunks.reduce((n, c) => n + 4 + c.byteLength, 0);
  const out = new Uint8Array(total);
  const view = new DataView(out.buffer);
  let at = 0;
  for (const chunk of chunks) {
    view.setUint32(at, chunk.byteLength, true);
    out.set(chunk, at + 4);
    at += 4 + chunk.byteLength;
  }
  let text = "";
  for (let i = 0; i < out.length; i += 0x8000) {
    text += String.fromCharCode(...out.subarray(i, i + 0x8000));
  }
  return btoa(text);
}

/** Starts or stops recording protocol messages; stopping returns them, length-prefixed, as base64. */
export function record(on: boolean): { readonly outbound: string; readonly replies: string } | null {
  if (on) {
    tap.outbound.length = 0;
    tap.replies.length = 0;
    tap.recording = true;
    return null;
  }
  tap.recording = false;
  return { outbound: base64(tap.outbound), replies: base64(tap.replies) };
}

/** Whether this browser can make a WebGL context; on a desktop without a GPU it cannot. */
export function webgl(): boolean {
  return document.createElement("canvas").getContext("webgl") !== null;
}

/** The fewest milliseconds of several runs: the build host's timings vary up to twofold. */
async function best<T extends { readonly ms: number }>(runs: number, step: () => Promise<T>): Promise<T> {
  let found: T | null = null;
  for (let i = 0; i < runs; i += 1) {
    const result = await step();
    if (found === null || result.ms < found.ms) {
      found = result;
    }
  }
  if (found === null) {
    throw new Error("no runs");
  }
  return found;
}

export interface SizeReport {
  readonly rows: number;
  readonly arrowBytes: number;
  readonly generateMs: number;
  readonly load: LoadResult;
  readonly pivots: readonly PivotResult[];
  readonly wasmBytes: number;
  readonly pageHeapBytes: number | null;
}

/** Loads one size into a fresh engine and times each pivot, best of `runs`. */
export async function measureSize(rows: number, runs = 3): Promise<SizeReport> {
  startEngine();
  await ready();
  const generated = await generate(rows);
  const loaded = await load();
  const pivots: PivotResult[] = [];
  for (const name of ["fund", "reference", "regroup"] as const) {
    pivots.push(await best(runs, () => open(name)));
  }
  pivots.push({ ...(await best(runs, () => relayout("reference"))), layout: "fund → reference, in place" });
  const s = await stats();
  return { rows, arrowBytes: generated.bytes, generateMs: generated.ms, load: loaded, pivots, wasmBytes: s.wasmBytes, pageHeapBytes: s.pageHeapBytes };
}

declare global {
  interface Window {
    spike: typeof api;
  }
}

const api = { startEngine, ready, probeStart, probeStop, generate, load, open, relayout, exportView, stats, record, webgl, measureSize, LAYOUTS };
window.spike = api;

const REPORT_KEY = "parallax-spike-rows-report";

/** The Run button: every size in the box, smallest first, stopping at the first failure. */
async function run(): Promise<void> {
  const output = document.querySelector<HTMLTextAreaElement>("#report");
  const sizes = (document.querySelector<HTMLInputElement>("#sizes")?.value ?? "")
    .split(/[ ,]+/)
    .map(Number)
    .filter((n) => Number.isInteger(n) && n > 0);
  const report: Record<string, unknown> = {
    userAgent: navigator.userAgent,
    cores: navigator.hardwareConcurrency,
    deviceMemoryGb: (navigator as Navigator & { deviceMemory?: number }).deviceMemory ?? null,
    webgl: webgl(),
    startedAt: new Date().toISOString(),
    sizes: [] as SizeReport[]
  };
  const show = (): void => {
    const text = JSON.stringify(report, null, 1);
    if (output !== null) {
      output.value = text;
    }
    // Kept so that a tab that runs out of memory and crashes shows the report so far when reloaded.
    try {
      localStorage.setItem(REPORT_KEY, text);
    } catch {
      // Without storage, only this tab holds the report.
    }
  };
  for (const rows of sizes) {
    try {
      (report["sizes"] as SizeReport[]).push(await measureSize(rows));
    } catch (error: unknown) {
      report["failed"] = { rows, message: error instanceof Error ? error.message : String(error) };
      show();
      break;
    }
    show();
  }
  report["finishedAt"] = new Date().toISOString();
  show();
}

try {
  const kept = localStorage.getItem(REPORT_KEY);
  const box = document.querySelector<HTMLTextAreaElement>("#report");
  if (kept !== null && box !== null) {
    box.value = kept;
  }
} catch {
  // Nothing kept.
}

document.querySelector("#run")?.addEventListener("click", () => {
  void run();
});
/** The Save button: loads 50,000 rows into a fresh engine and saves the outlined pivot as xlsx and the flat view as CSV. */
async function save(): Promise<void> {
  const output = document.querySelector<HTMLTextAreaElement>("#report");
  startEngine();
  await ready();
  await generate(50_000);
  await load();
  const saved = [await exportView("outlined", "xlsx"), await exportView("flat", "csv")];
  if (output !== null) {
    output.value = `${output.value}\n${JSON.stringify({ saved }, null, 1)}`;
  }
}

document.querySelector("#save")?.addEventListener("click", () => {
  void save();
});
document.querySelector("#copy")?.addEventListener("click", () => {
  void navigator.clipboard.writeText(document.querySelector<HTMLTextAreaElement>("#report")?.value ?? "");
});
