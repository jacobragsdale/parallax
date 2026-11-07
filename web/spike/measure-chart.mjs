// Spike 0.3: how long a page with one small chart takes, cold and warm.
//
//   node spike/measure-chart.mjs --url http://127.0.0.1:8797/chart.html [--profiles 5] \
//       [--reloads 5] [--throttle none|cdp4] [--label NAME] [--out results.json]
//
// Each profile is a fresh Chromium user data directory, so its first load is
// cold: nothing in the HTTP cache or the compiled-code cache. Its reloads are
// warm, and a relaunch on the same profile is warm from disk. Every figure is
// milliseconds from the start of navigation, on the page's clock; the worker's
// marks are shifted onto it. The script reports the run of each kind that drew
// its chart soonest.
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { chromium } from "@playwright/test";

import { ARGS } from "./cdp.mjs";

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const i = args.indexOf(name);
  return i === -1 ? fallback : args[i + 1];
};
const url = option("--url", "http://127.0.0.1:8797/chart.html");
const profiles = Number(option("--profiles", "5"));
const reloads = Number(option("--reloads", "5"));
const throttle = option("--throttle", "none");
const label = option("--label", throttle);
const out = option("--out", null);
const root = option("--profile-root", tmpdir());

async function timeline(page) {
  return page.evaluate(async () => {
    const t = await window.chartTimeline;
    return t;
  });
}

function figures(t) {
  const w = t.worker;
  return {
    workerScriptStarted: w["px-worker-start"],
    wasmFetchStart: t.wasm?.fetchStart,
    wasmDownloaded: t.wasm?.responseEnd,
    wasmTransferBytes: t.wasm?.transferSize,
    compiled: w["px-compiled"],
    compileAfterDownload: w["px-compiled"] - (t.wasm?.responseEnd ?? NaN),
    instantiated: w["px-instantiated"],
    engineReady: w["px-ready"],
    pageScript: t.marks.script,
    workerCreated: t.marks.workerCreated,
    loaded: t.marks.loaded,
    chartFrame: t.marks.chartFrame,
    painted: t.marks.painted,
    drawn: t.marks.drawn
  };
}

async function open(dir) {
  const context = await chromium.launchPersistentContext(dir, { args: ARGS, viewport: { width: 1000, height: 700 } });
  const page = context.pages()[0] ?? (await context.newPage());
  page.on("pageerror", (e) => console.log("pageerror:", e.message));
  if (throttle === "cdp4") {
    const cdp = await context.newCDPSession(page);
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
  }
  return { context, page };
}

const runs = { cold: [], warm: [], relaunched: [] };
for (let p = 0; p < profiles; p += 1) {
  const dir = mkdtempSync(join(root, "px-chart-"));
  try {
    let { context, page } = await open(dir);
    await page.goto(url);
    runs.cold.push(figures(await timeline(page)));
    for (let r = 0; r < reloads; r += 1) {
      await page.reload();
      runs.warm.push(figures(await timeline(page)));
    }
    await context.close();
    ({ context, page } = await open(dir));
    await page.goto(url);
    runs.relaunched.push(figures(await timeline(page)));
    await context.close();
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

// The fastest whole run of each kind, by when the chart was drawn, so that its steps add up.
const best = Object.fromEntries(Object.entries(runs).map(([kind, list]) => [kind, list.reduce((a, b) => (b.drawn < a.drawn ? b : a))]));
console.log(
  label,
  JSON.stringify(best, (k, v) => (typeof v === "number" ? Math.round(v * 10) / 10 : v), 1)
);
if (out !== null) {
  writeFileSync(out, JSON.stringify({ label, url, throttle, profiles, reloads, best, runs }, null, 1));
}
