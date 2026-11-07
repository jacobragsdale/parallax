import { chromium } from "@playwright/test";
const [url, sizes] = process.argv.slice(2);
const browser = await chromium.launch({ channel: process.env.CHANNEL, args: ["--disable-gpu", "--disable-gpu-compositing", "--disable-software-rasterizer"] });
const page = await browser.newPage();
page.on("pageerror", (e) => console.log("pageerror:", e.message));
await page.goto(url);
await page.fill("#sizes", sizes);
const t = Date.now();
await page.click("#run");
await page.waitForFunction(() => document.querySelector("#report").value.includes("finishedAt"), null, { timeout: 600000 });
const report = JSON.parse(await page.inputValue("#report"));
console.log("took", (Date.now() - t) / 1000, "s; webgl", report.webgl, "cores", report.cores);
console.log(JSON.stringify(report.failed ?? null));
for (const s of report.sizes)
  console.log(s.rows, "load", s.load.ms.toFixed(0), "pivots", s.pivots.map((p) => `${p.layout}=${p.ms.toFixed(0)}`).join(" "), "wasm", (s.wasmBytes / 1e6).toFixed(0), "MB");
await browser.close();
