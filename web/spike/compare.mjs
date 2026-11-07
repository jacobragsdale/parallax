// Spike 0.3: the browser half of the native comparison, and the export downloads.
//
//   node spike/compare.mjs --url http://127.0.0.1:8797/ --rows 100000 --out DIR
//
// Generates positions in the page (keeping their SHA-256), loads them into the
// engine worker, records every protocol message and reply while it opens the
// pivots, and saves xlsx and CSV exports through the page's Blob downloads.
// `spike_native` then replays the recorded messages natively.
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

import { chromium } from "@playwright/test";

import { ARGS } from "./cdp.mjs";

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const i = args.indexOf(name);
  return i === -1 ? fallback : args[i + 1];
};
const url = option("--url", "http://127.0.0.1:8797/");
const rows = Number(option("--rows", "100000"));
const dir = option("--out", "compare");
const exports = option("--exports", "outlined:xlsx,outlined:csv,flat:xlsx,flat:csv").split(",");
mkdirSync(dir, { recursive: true });

const browser = await chromium.launch({ args: ARGS });
const page = await browser.newPage({ acceptDownloads: true });
page.on("pageerror", (e) => console.log("pageerror:", e.message));
await page.goto(url);
const run = await page.evaluate(async (n) => {
  const s = window.spike;
  s.startEngine();
  await s.ready();
  const generated = await s.generate(n, 42, true);
  const loaded = await s.load();
  s.record(true);
  const pivots = [];
  for (const name of ["reference", "outlined", "regroup", "fund"]) {
    pivots.push(await s.open(name));
  }
  pivots.push(await s.relayout("reference"));
  return { generated, loaded, pivots, recorded: s.record(false) };
}, rows);
writeFileSync(join(dir, "browser-outbound.bin"), Buffer.from(run.recorded.outbound, "base64"));
writeFileSync(join(dir, "browser-replies.bin"), Buffer.from(run.recorded.replies, "base64"));
delete run.recorded;
run.exports = [];
for (const item of exports) {
  const [name, format] = item.split(":");
  const before = (await page.evaluate(() => window.spike.stats())).wasmBytes;
  const [download, result] = await Promise.all([page.waitForEvent("download"), page.evaluate(([n, f]) => window.spike.exportView(n, f), [name, format])]);
  const path = join(dir, `browser-${name}.${format}`);
  await download.saveAs(path);
  run.exports.push({ ...result, wasmBefore: before, suggestedFilename: download.suggestedFilename(), path });
  console.log(
    `export ${name}.${format}: ${result.bytes} bytes in ${result.ms.toFixed(0)} ms (engine ${result.engineMs.toFixed(0)}), wasm ${(before / 1e6).toFixed(0)} -> ${(result.wasmBytes / 1e6).toFixed(0)} MB`
  );
}
await browser.close();
writeFileSync(join(dir, "browser-run.json"), JSON.stringify(run, null, 1));
console.log(JSON.stringify({ sha256: run.generated.sha256, bytes: run.generated.bytes, load: run.loaded, pivots: run.pivots }, null, 1));
