/**
 * Spike 0.3: a page whose only parallax content is one small chart.
 *
 * It builds a 100-point table in JavaScript, as the small-data `<Chart rows>` API
 * would, loads it into parallax's engine worker, asks for the chart's reduction
 * and draws it with the library's own model and Canvas 2D painter. It records when
 * each step finished, from the start of navigation, and keeps every load's figures
 * in sessionStorage, so that reloading shows the cold load beside the warm ones.
 */

import { makeTable, tableToIPC } from "apache-arrow";

import { BrowserEngine } from "../projects/parallax/src/lib/browser-engine";
import { chartModel } from "../projects/parallax/src/lib/chart/model";
import { paintChart } from "../projects/parallax/src/lib/chart/paint";
import { parseLayout } from "../projects/parallax/src/lib/layout";
import { CHART_COLORS, defaultTheme } from "../projects/parallax/src/lib/theme";

const marks: Record<string, number> = { script: performance.now() };
const POINTS = 100;
const WIDTH = 640;
const HEIGHT = 320;

const LAYOUT = `{"version":2,"dataset":"series","chart":{"kind":"line","x":"day","y":["exposure"]}}`;

/** A 100-point series, built in the page as Arrow, the way the small-data API would build it. */
function series(): ArrayBuffer {
  const days = Int32Array.from({ length: POINTS }, (_, i) => i);
  const exposure = Float64Array.from({ length: POINTS }, (_, i) => 100 + 10 * Math.sin(i / 8) + i / 5);
  const table = makeTable({ day: days, exposure });
  const bytes = tableToIPC(table, "stream");
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

interface Timeline {
  readonly at: string;
  readonly marks: Readonly<Record<string, number>>;
  readonly worker: Readonly<Record<string, number>>;
  readonly wasm: { readonly transferSize: number; readonly encodedBodySize: number; readonly decodedBodySize: number; readonly fetchStart: number; readonly responseEnd: number } | null;
  readonly navigation: { readonly responseEnd: number; readonly domContentLoaded: number } | null;
  readonly bundle: { readonly transferSize: number; readonly encodedBodySize: number; readonly decodedBodySize: number; readonly responseEnd: number } | null;
}

async function main(): Promise<Timeline> {
  const engine = BrowserEngine.start("wasm/worker.js");
  marks["workerCreated"] = performance.now();
  await engine.load("series", series());
  marks["loaded"] = performance.now();
  const frame = await new Promise<Parameters<NonNullable<Parameters<typeof engine.connection.open>[2]["chart"]>>[0]>((resolve, reject) => {
    const view = engine.connection.open("series", LAYOUT, {
      frame: () => undefined,
      chart: (chart) => {
        resolve(chart);
      },
      error: (message) => {
        reject(new Error(message));
      }
    });
    view.setChartWindow({ width: WIDTH, height: HEIGHT });
  });
  marks["chartFrame"] = performance.now();
  const canvas = document.querySelector("canvas");
  const context = canvas?.getContext("2d", { alpha: false }) ?? null;
  if (canvas === null || context === null) {
    throw new Error("no canvas");
  }
  const theme = defaultTheme("light");
  const measurer = document.createElement("canvas").getContext("2d");
  const font = `${theme.fontSizeSmall} ${theme.fontFamily}`;
  const chart = parseLayout(JSON.parse(LAYOUT)).chart;
  if (chart === null) {
    throw new Error("the layout has no chart");
  }
  const model = chartModel({
    chart,
    width: WIDTH,
    height: HEIGHT,
    colors: CHART_COLORS.light,
    muted: theme.muted,
    measure: (text) => {
      if (measurer === null) {
        return text.length * 7;
      }
      measurer.font = font;
      return measurer.measureText(text).width;
    },
    frame,
    categories: null
  });
  const ratio = window.devicePixelRatio;
  canvas.width = Math.round(WIDTH * ratio);
  canvas.height = Math.round(HEIGHT * ratio);
  canvas.style.width = `${String(WIDTH)}px`;
  canvas.style.height = `${String(HEIGHT)}px`;
  context.setTransform(ratio, 0, 0, ratio, 0, 0);
  paintChart(context, model, theme);
  marks["painted"] = performance.now();
  // The chart is on screen once the browser has presented the frame that holds it.
  await new Promise<void>((resolve) => {
    requestAnimationFrame(() => {
      setTimeout(resolve, 0);
    });
  });
  marks["drawn"] = performance.now();

  const stats = await engine.stats();
  // The worker's clock starts later than the page's; shift its marks onto the page's.
  const shift = stats.timeOrigin - performance.timeOrigin;
  const worker = Object.fromEntries(Object.entries(stats.marks).map(([name, at]) => [name, at + shift]));
  const wasmEntry = stats.resources.find((r) => r.name.endsWith(".wasm")) ?? null;
  const nav = performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined;
  const bundleEntry = (performance.getEntriesByType("resource") as PerformanceResourceTiming[]).find((r) => r.name.endsWith("chart.js")) ?? null;
  return {
    at: new Date().toISOString(),
    marks,
    worker,
    wasm:
      wasmEntry === null
        ? null
        : {
            transferSize: wasmEntry.transferSize,
            encodedBodySize: wasmEntry.encodedBodySize,
            decodedBodySize: wasmEntry.decodedBodySize,
            fetchStart: wasmEntry.startTime + shift,
            responseEnd: wasmEntry.responseEnd + shift
          },
    navigation: nav === undefined ? null : { responseEnd: nav.responseEnd, domContentLoaded: nav.domContentLoadedEventEnd },
    bundle:
      bundleEntry === null
        ? null
        : { transferSize: bundleEntry.transferSize, encodedBodySize: bundleEntry.encodedBodySize, decodedBodySize: bundleEntry.decodedBodySize, responseEnd: bundleEntry.responseEnd }
  };
}

declare global {
  interface Window {
    chartTimeline: Promise<Timeline>;
  }
}

const KEY = "parallax-spike-chart";
window.chartTimeline = main();
window.chartTimeline.then(
  (timeline) => {
    let earlier: Timeline[] = [];
    try {
      earlier = JSON.parse(sessionStorage.getItem(KEY) ?? "[]") as Timeline[];
    } catch {
      earlier = [];
    }
    const all = [...earlier, timeline];
    try {
      sessionStorage.setItem(KEY, JSON.stringify(all));
    } catch {
      // Without storage, the page shows this load only.
    }
    const output = document.querySelector<HTMLTextAreaElement>("#report");
    if (output !== null) {
      output.value = JSON.stringify({ userAgent: navigator.userAgent, cores: navigator.hardwareConcurrency, loads: all }, null, 1);
    }
    document.title = `drawn in ${timeline.marks["drawn"]?.toFixed(0) ?? "?"} ms`;
  },
  (error: unknown) => {
    document.title = `failed: ${error instanceof Error ? error.message : String(error)}`;
  }
);
document.querySelector("#copy")?.addEventListener("click", () => {
  void navigator.clipboard.writeText(document.querySelector<HTMLTextAreaElement>("#report")?.value ?? "");
});
document.querySelector("#clear")?.addEventListener("click", () => {
  sessionStorage.removeItem(KEY);
});
