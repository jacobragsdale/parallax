// Spike 0.3: how many positions parallax loads and pivots within its budgets.
//
//   node spike/measure-rows.mjs --url http://127.0.0.1:8797/ --sizes 50000,100000 \
//       [--runs 3] [--throttle none|cdp4] [--key position_id|none] [--decode copy|inplace] [--label NAME] [--out results.json]
//
// Each size runs in a fresh headless Chromium with the GPU off. The page
// generates the rows in a worker of its own, then the script times the load
// into parallax's engine worker and each pivot (the best of --runs), and reads
// the engine's memory, both JavaScript heaps after garbage collection, and the
// renderer process's resident memory, now and at its peak during the load.
// `cdp4` throttles the page's main thread four times through DevTools; it does
// not reach the worker. For a slower worker, run this script inside a CPU quota:
//   systemd-run --user --scope -p CPUQuota=25% -p CPUQuotaPeriodSec=10ms -- node spike/measure-rows.mjs ...
import { readFileSync, writeFileSync } from "node:fs";

import { chromium } from "@playwright/test";

import { ARGS, browserSession } from "./cdp.mjs";

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const i = args.indexOf(name);
  return i === -1 ? fallback : args[i + 1];
};
const url = option("--url", "http://127.0.0.1:8797/");
const sizes = option("--sizes", "50000,100000").split(",").map(Number);
const runs = Number(option("--runs", "3"));
const throttle = option("--throttle", "none");
const label = option("--label", throttle);
const out = option("--out", null);
const key = option("--key", "position_id");
const decode = option("--decode", "copy");

function availableGb() {
  const kb = Number(/MemAvailable:\s+(\d+)/.exec(readFileSync("/proc/meminfo", "utf8"))?.[1] ?? 0);
  return kb / 1024 / 1024;
}

const results = [];
for (const rows of sizes) {
  if (availableGb() < 3) {
    console.log(`stopping before ${rows} rows: ${availableGb().toFixed(1)} GB available`);
    break;
  }
  const browser = await chromium.launch({ args: ARGS });
  const result = { rows, label };
  try {
    const page = await browser.newPage();
    page.on("pageerror", (e) => console.log("pageerror:", e.message));
    await page.goto(url);
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Performance.enable");
    const b = await browserSession(browser);
    await page.evaluate(async () => {
      window.spike.startEngine();
      await window.spike.ready();
    });
    const worker = await b.attach("worker", (t) => t.url.includes("/wasm/worker.js"));
    const generated = await page.evaluate((n) => window.spike.generate(n), rows);
    result.arrowBytes = generated.bytes;
    const gc = async () => {
      await cdp.send("HeapProfiler.collectGarbage");
      await worker("HeapProfiler.collectGarbage");
    };
    await gc();
    const [renderer] = await b.renderers();
    result.rssBeforeLoad = renderer.rss;
    // Reset the peak, so that it covers the load alone, not the generator's worker.
    writeFileSync(`/proc/${renderer.pid}/clear_refs`, "5");
    if (throttle === "cdp4") {
      await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
    }
    const taskBefore = (await cdp.send("Performance.getMetrics")).metrics.find((m) => m.name === "TaskDuration").value;
    await page.evaluate(() => window.spike.probeStart());
    result.key = key;
    result.decode = decode;
    result.load = await page.evaluate(([k, d]) => window.spike.load(k === "none" ? null : k, d === "inplace"), [key, decode]);
    result.loadProbe = await page.evaluate(() => window.spike.probeStop());
    const [loaded] = await b.renderers();
    result.rssAfterLoad = loaded.rss;
    result.rssPeakDuringLoad = loaded.peak;
    await gc();
    result.pageHeap = await cdp.send("Runtime.getHeapUsage");
    result.workerHeap = await worker("Runtime.getHeapUsage");
    result.rssAfterGc = (await b.renderers())[0].rss;
    result.pivots = [];
    await page.evaluate(() => window.spike.probeStart());
    for (const name of ["fund", "reference", "regroup"]) {
      const times = [];
      let last = null;
      for (let i = 0; i < runs; i += 1) {
        last = await page.evaluate((n) => window.spike.open(n), name);
        times.push(last.ms);
      }
      result.pivots.push({ layout: name, bestMs: Math.min(...times), times, totalRows: last.totalRows, totalColumns: last.totalColumns });
    }
    const times = [];
    for (let i = 0; i < runs; i += 1) {
      times.push((await page.evaluate(() => window.spike.relayout("reference"))).ms);
    }
    result.pivots.push({ layout: "fund to reference, in place", bestMs: Math.min(...times), times });
    result.pivotProbe = await page.evaluate(() => window.spike.probeStop());
    const taskAfter = (await cdp.send("Performance.getMetrics")).metrics.find((m) => m.name === "TaskDuration").value;
    result.mainThreadTaskMs = (taskAfter - taskBefore) * 1000;
    const s = await page.evaluate(() => window.spike.stats());
    result.wasmBytes = s.wasmBytes;
    const [end] = await b.renderers();
    result.rssEnd = end.rss;
    result.rssPeakFromLoad = end.peak;
  } catch (error) {
    result.error = String(error?.message ?? error);
  } finally {
    await browser.close();
  }
  const mb = (n) => (n === undefined ? "?" : (n / 1e6).toFixed(0));
  console.log(
    `${label} ${rows} rows: arrow ${mb(result.arrowBytes)} MB, load ${result.load?.ms?.toFixed(0)} ms (engine ${result.load?.engineMs?.toFixed(0)}), ` +
      (result.pivots ?? []).map((p) => `${p.layout} ${p.bestMs.toFixed(0)} ms`).join(", ") +
      `, wasm ${mb(result.wasmBytes)} MB, rss ${mb(result.rssAfterGc)} MB (peak ${mb(result.rssPeakFromLoad)}), page heap ${mb(result.pageHeap?.usedSize)} MB, worker heap ${mb(result.workerHeap?.usedSize)} MB` +
      `, main-thread gap ${result.loadProbe?.maxGapMs?.toFixed(0)}/${result.pivotProbe?.maxGapMs?.toFixed(0)} ms${result.error ? `, ERROR ${result.error}` : ""}`
  );
  results.push(result);
  if (out !== null) {
    writeFileSync(out, JSON.stringify(results, null, 1));
  }
  if (result.error) {
    break;
  }
}
