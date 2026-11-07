// Spike 0.3: what the page sees when the engine runs out of WebAssembly memory.
// Serve a build linked with `-C link-arg=--max-memory=268435456` (256 MB), then:
//   node spike/oom.mjs --url http://127.0.0.1:8797/
import { chromium } from "@playwright/test";

import { ARGS } from "./cdp.mjs";

const url = process.argv[process.argv.indexOf("--url") + 1] ?? "http://127.0.0.1:8797/";
const browser = await chromium.launch({ args: ARGS });
const page = await browser.newPage();
page.on("pageerror", (e) => console.log("pageerror:", e.message));
await page.goto(url);
const result = await page.evaluate(async () => {
  const s = window.spike;
  const step = (promise) =>
    Promise.race([
      promise.then(
        (v) => ({ ok: v }),
        (e) => ({ error: String(e?.message ?? e) })
      ),
      new Promise((r) => setTimeout(() => r({ timedOut: "no answer in 10 s" }), 10_000))
    ]);
  const out = {};
  s.startEngine();
  await s.ready();
  await s.generate(250_000);
  out.load250k = await step(s.load());
  out.pivotAfter = await step(s.open("fund"));
  out.statsAfter = await step(s.stats());
  s.startEngine();
  await s.ready();
  await s.generate(50_000);
  out.freshLoad50k = await step(s.load());
  out.freshPivot = await step(s.open("fund"));
  return out;
});
console.log(JSON.stringify(result, null, 1));
await browser.close();
