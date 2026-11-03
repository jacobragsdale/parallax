/** `<px-chart>` in the demo: every kind draws, and the legend hides and shows series. */

import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";

/** How many pixels of a canvas differ from its top-left pixel, the surface. */
async function inked(canvas: Locator): Promise<number> {
  return canvas.evaluate((element) => {
    if (!(element instanceof HTMLCanvasElement)) {
      return 0;
    }
    const context = element.getContext("2d");
    if (context === null) {
      return 0;
    }
    const { data } = context.getImageData(0, 0, element.width, element.height);
    let count = 0;
    for (let i = 0; i < data.length; i += 4) {
      if (data[i] !== data[0] || data[i + 1] !== data[1] || data[i + 2] !== data[2]) {
        count += 1;
      }
    }
    return count;
  });
}

async function open(page: Page): Promise<void> {
  await page.goto("/?preset=charts");
  await expect(page.locator("px-chart")).toHaveCount(6);
  // Every chart has drawn its data once its status bar says what it shows.
  await expect(page.locator("px-chart .px-status-rows").filter({ hasText: "loading" })).toHaveCount(0, { timeout: 60_000 });
}

test("every chart kind draws its data, with an accessible name and no errors", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await open(page);
  const names = await page.locator("px-chart canvas").evaluateAll((canvases) => canvases.map((c) => c.getAttribute("aria-label") ?? ""));
  const kinds = ["Line chart of close by ticker", "Candlestick chart of date", "Bar chart of sector", "Histogram of beta by long_short", "Density of points of daily_pnl_usd", "Heatmap of sector"];
  expect(names.map((n, i) => n.startsWith(kinds[i] ?? "?"))).toEqual(kinds.map(() => true));
  for (const canvas of await page.locator("px-chart canvas").all()) {
    // Axes alone ink a few thousand pixels; data inks far more.
    expect(await inked(canvas)).toBeGreaterThan(5_000);
  }
  await expect(page.locator("px-chart .px-error")).toHaveCount(0);
  await expect(page.locator("px-chart").first().locator(".px-status-where")).toHaveText("in browser");
  expect(errors).toEqual([]);
});

test("the legend hides a series and shows it again, keeping the others' colours", async ({ page }) => {
  await open(page);
  const chart = page.locator("px-chart").first();
  const entries = chart.locator(".px-legend-entry");
  await expect(entries).toHaveCount(3);
  const colours = (): Promise<string[]> => entries.locator(".px-legend-key").evaluateAll((keys) => keys.map((k) => getComputedStyle(k).backgroundColor));
  const before = await colours();
  const ink = await inked(chart.locator("canvas"));
  await entries.nth(1).click();
  await expect(entries.nth(1)).toHaveAttribute("aria-pressed", "false");
  await expect.poll(() => inked(chart.locator("canvas"))).toBeLessThan(ink);
  expect(await colours()).toEqual(before);
  await entries.nth(1).click();
  await expect(entries.nth(1)).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => inked(chart.locator("canvas"))).toBe(ink);
});
