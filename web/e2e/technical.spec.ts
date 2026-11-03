/** The demo's technical-analysis page: its controls remake the chart, and its grid follows a brush. */

import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";

async function open(page: Page): Promise<Locator> {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/?preset=technical");
  const chart = page.locator("demo-technical px-chart");
  await expect(chart.locator(".px-status-rows")).not.toHaveText(/loading/, { timeout: 60_000 });
  expect(errors).toEqual([]);
  return chart;
}

/** The rows of the tooltip at the middle of the price, as their labels. */
async function readout(page: Page, chart: Locator): Promise<string[]> {
  const box = await chart.locator(".px-chart-plot").boundingBox();
  await page.mouse.move((box?.x ?? 0) + (box?.width ?? 0) / 2, (box?.y ?? 0) + (box?.height ?? 0) * 0.2);
  const tip = chart.locator(".px-chart-tip");
  await expect(tip).toBeVisible();
  return (await tip.locator(".px-tip-row span:last-child").allTextContents()).map((l) => l.replace(/, \d{4}-\d{2}-\d{2}$/, ""));
}

test("the controls start where they say, and an indicator turned off leaves the chart", async ({ page }) => {
  const chart = await open(page);
  await expect(page.getByRole("combobox", { name: "Since" })).toHaveValue("2024-01-01");
  await expect(page.getByRole("combobox", { name: "Chart kind" })).toHaveValue("candlestick");
  expect(await readout(page, chart)).toContain("RSI 14");
  await page.getByLabel("RSI 14").uncheck();
  await expect.poll(() => readout(page, chart)).not.toContain("RSI 14");
  await page.getByLabel("ATR 14").check();
  await expect.poll(() => readout(page, chart)).toContain("ATR 14");
});

test("comparing three tickers reads percentages and turns off what does not fit", async ({ page }) => {
  const chart = await open(page);
  await page.getByLabel("Compare three tickers").check();
  // Averages are in prices and ATR needs candles: neither fits a comparison.
  await expect(page.getByLabel("SMA 50")).toBeDisabled();
  await expect(page.getByLabel("ATR 14")).toBeDisabled();
  await expect(page.getByRole("combobox", { name: "Ticker" })).toBeDisabled();
  const box = await chart.locator(".px-chart-plot").boundingBox();
  await page.mouse.move((box?.x ?? 0) + (box?.width ?? 0) / 2, (box?.y ?? 0) + (box?.height ?? 0) * 0.2);
  await expect(chart.locator(".px-chart-tip .px-tip-row strong").first()).toHaveText(/%$/);
});

test("Shift and drag lists the brushed days, and another ticker shows all of its own", async ({ page }) => {
  const chart = await open(page);
  const rows = page.locator("demo-technical .rows .px-status-rows");
  await expect(rows).toHaveText(/^[\d,]+ rows$/);
  const count = async (): Promise<number> => Number((await rows.textContent())?.replace(/\D/g, "") ?? "0");
  const all = await count();
  const box = await chart.locator(".px-chart-plot").boundingBox();
  const y = (box?.y ?? 0) + (box?.height ?? 0) * 0.2;
  await page.keyboard.down("Shift");
  await page.mouse.move((box?.x ?? 0) + (box?.width ?? 0) * 0.4, y);
  await page.mouse.down();
  await page.mouse.move((box?.x ?? 0) + (box?.width ?? 0) * 0.6, y, { steps: 5 });
  await page.mouse.up();
  await page.keyboard.up("Shift");
  await expect.poll(count).toBeLessThan(all / 2);
  await page.getByRole("combobox", { name: "Ticker" }).selectOption("NV");
  await expect.poll(count).toBe(all);
  // NV trades near 50, QYE near 10,000: the chart is NV's now.
  const close = async (): Promise<number> => {
    await page.mouse.move((box?.x ?? 0) + (box?.width ?? 0) / 2, y);
    const text = await chart.locator(".px-chart-tip .px-tip-row").filter({ hasText: "Close" }).locator("strong").textContent();
    return Number(text?.replace(/,/g, "") ?? "NaN");
  };
  await expect.poll(close).toBeLessThan(1000);
});
