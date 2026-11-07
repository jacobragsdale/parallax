import { chromium } from "@playwright/test";
import { ARGS, browserSession } from "./cdp.mjs";
const browser = await chromium.launch({ args: ARGS });
const page = await browser.newPage();
await page.goto("http://127.0.0.1:8797/");
await page.evaluate(async () => {
  window.spike.startEngine();
  await window.spike.generate(200000);
  await window.spike.load();
});
const b = await browserSession(browser);
const time = async (label) => {
  const runs = [];
  for (let i = 0; i < 3; i++) runs.push(await page.evaluate(async () => (await window.spike.open("reference")).ms));
  const main = await page.evaluate(() => {
    const t = performance.now();
    let x = 0;
    for (let i = 0; i < 3e7; i++) x += i % 7;
    return [performance.now() - t, x][0];
  });
  console.log(label, "reference pivot ms", runs.map((r) => r.toFixed(0)).join(" "), "| main-thread loop ms", main.toFixed(0));
};
await time("unthrottled");
const pageCdp = await page.context().newCDPSession(page);
await pageCdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
await time("page throttled 4x");
const worker = await b.attach("worker", (t) => t.url.includes("/wasm/worker.js"));
try {
  await worker("Emulation.setCPUThrottlingRate", { rate: 4 });
  console.log("worker accepted Emulation.setCPUThrottlingRate");
} catch (e) {
  console.log("worker refused:", e.message);
}
await time("page and worker throttled 4x");
await pageCdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
await time("worker only throttled 4x");
console.log(await b.renderers());
console.log("worker heap", await worker("Runtime.getHeapUsage"));
await browser.close();
