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

/** The chart of a demo card, by the card's id. */
function card(page: Page, id: string): Locator {
  return page.locator(`section[data-card="${id}"] px-chart`);
}

async function open(page: Page): Promise<void> {
  await page.goto("/?preset=charts");
  await expect(page.locator("px-chart")).toHaveCount(11);
  // Every chart has drawn its data once its status bar says what it shows.
  await expect(page.locator("px-chart .px-status-rows").filter({ hasText: "loading" })).toHaveCount(0, { timeout: 60_000 });
}

test("every chart kind draws its data, with an accessible name and no errors", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await open(page);
  const names = await page.locator("px-chart canvas").evaluateAll((canvases) => canvases.map((c) => c.getAttribute("aria-label") ?? ""));
  const kinds = [
    "Candlestick chart of date",
    "Line chart of close by ticker",
    "Candlestick chart of date",
    "Line chart of close, volume",
    "OHLC chart of date",
    "Area chart of close",
    "Stacked bar chart of sector by asset_class",
    "Horizontal bar chart of sector by long_short",
    "Histogram of beta by long_short",
    "Density of points of daily_pnl_usd",
    "Heatmap of sector by asset_class"
  ];
  expect(names.map((n, i) => n.startsWith(kinds[i] ?? "?"))).toEqual(kinds.map(() => true));
  for (const canvas of await page.locator("px-chart canvas").all()) {
    // Axes alone ink a few thousand pixels; data inks far more.
    expect(await inked(canvas)).toBeGreaterThan(5_000);
  }
  await expect(page.locator("px-chart .px-error")).toHaveCount(0);
  await expect(card(page, "closes-log").locator(".px-status-where")).toHaveText("in browser");
  expect(errors).toEqual([]);
});

test("the legend hides a series and shows it again, keeping the others' colours", async ({ page }) => {
  await open(page);
  const chart = card(page, "closes-log");
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

/** The middle of a chart's plot, in page pixels. */
async function centre(plot: Locator): Promise<{ readonly x: number; readonly y: number; readonly width: number }> {
  // Mouse coordinates are the viewport's: the plot must be in it.
  await plot.scrollIntoViewIfNeeded();
  const box = await plot.boundingBox();
  if (box === null) {
    throw new Error("the plot is not on the page");
  }
  return { x: box.x + box.width / 2, y: box.y + box.height / 2, width: box.width };
}

test("hovering a line reads every ticker at the nearest day, and the candles follow the crosshair", async ({ page }) => {
  await open(page);
  const [line, candles] = [card(page, "closes-log"), card(page, "candles")];
  const at = await centre(line.locator(".px-chart-plot"));
  await page.mouse.move(at.x, at.y);
  const tip = line.locator(".px-chart-tip");
  await expect(tip).toBeVisible();
  await expect(tip.locator(".px-tip-title")).toHaveText(/\d/);
  // One row per ticker, each a value and the ticker's name.
  await expect(tip.locator(".px-tip-row")).toHaveCount(3);
  for (const ticker of ["QYE", "ZREZ", "NV"]) {
    await expect(tip.locator(".px-tip-row").filter({ hasText: ticker }).locator("strong")).toHaveText(/^-?[\d,.]+$/);
  }
  // The candles, of the same dataset, x and axis, read the bucket under the same day.
  await expect(candles.locator(".px-chart-crosshair")).toBeVisible();
  await expect(candles.locator(".px-chart-tip .px-tip-row")).toHaveCount(4);
  await page.mouse.move(at.x, 5);
  await expect(page.locator(".px-chart-tip")).toHaveCount(0);
});

test("the wheel zooms, dragging pans, and a double-click or 0 shows everything", async ({ page }) => {
  await open(page);
  const candles = card(page, "candles");
  const plot = candles.locator(".px-chart-plot");
  const status = candles.locator(".px-status-rows");
  await expect(status).not.toContainText("zoomed");
  const at = await centre(plot);
  await page.mouse.move(at.x, at.y);
  for (let i = 0; i < 20; i += 1) {
    await page.mouse.wheel(0, -100);
  }
  // Zoomed in far enough, each candle is a day.
  await expect(status).toContainText("day candles");
  await expect(status).toContainText("zoomed");
  const title = candles.locator(".px-chart-tip .px-tip-title");
  await page.mouse.move(at.x, at.y);
  const before = await title.textContent();
  await page.mouse.down();
  await page.mouse.move(at.x - at.width / 4, at.y, { steps: 5 });
  await page.mouse.up();
  await page.mouse.move(at.x + 1, at.y);
  await page.mouse.move(at.x, at.y);
  // Dragging left brings later days under the pointer.
  await expect(title).not.toHaveText(before ?? "");
  await page.mouse.dblclick(at.x, at.y);
  await expect(status).not.toContainText("zoomed");
  await plot.focus();
  await page.keyboard.press("+");
  await expect(status).toContainText("zoomed");
  await page.keyboard.press("0");
  await expect(status).not.toContainText("zoomed");
});

test("Shift and drag on the candles lists the selected days in the grid, and Escape lists them all again", async ({ page }) => {
  await open(page);
  const candles = card(page, "candles");
  const grid = page.locator("section").filter({ has: page.locator("px-grid") });
  const rows = grid.locator(".px-status-rows");
  await expect(rows).toHaveText(/^[\d,]+ rows$/);
  const count = async (): Promise<number> => Number((await rows.textContent())?.replace(/\D/g, "") ?? "0");
  const all = await count();
  const at = await centre(candles.locator(".px-chart-plot"));
  await page.keyboard.down("Shift");
  await page.mouse.move(at.x - at.width / 8, at.y);
  await page.mouse.down();
  await page.mouse.move(at.x + at.width / 8, at.y, { steps: 5 });
  await page.mouse.up();
  await page.keyboard.up("Shift");
  await expect(candles.locator(".px-chart-brush")).toBeVisible();
  await expect(grid.locator("h2")).toHaveText("QYE's days in the selected candles");
  // A quarter of the plot holds about a quarter of the days.
  await expect.poll(count).toBeLessThan(all / 2);
  expect(await count()).toBeGreaterThan(all / 8);
  await candles.locator(".px-chart-plot").press("Escape");
  await expect(candles.locator(".px-chart-brush")).toHaveCount(0);
  await expect.poll(count).toBe(all);
});

test("the technical card stacks RSI, MACD and volume under the candles, and its tooltip reads every indicator", async ({ page }) => {
  await open(page);
  const chart = card(page, "technical");
  await expect(chart.locator(".px-legend-entry")).toHaveText(["SMA 50", "BB 20 2"]);
  // Panes add their own inked area under the price.
  expect(await inked(chart.locator("canvas"))).toBeGreaterThan(40_000);
  const at = await centre(chart.locator(".px-chart-plot"));
  await page.mouse.move(at.x, at.y);
  const tip = chart.locator(".px-chart-tip");
  await expect(tip.locator(".px-tip-title")).toHaveText(/^Week of \d{4}-\d{2}-\d{2}$/);
  // The candle's prices and volume, then every indicator line.
  const labels = (await tip.locator(".px-tip-row span:last-child").allTextContents()).map((l) => l.replace(/, \d{4}-\d{2}-\d{2}$/, ""));
  expect(labels).toEqual(["Open", "High", "Low", "Close", "Volume", "SMA 50", "BB 20 2 upper", "BB 20 2 middle", "BB 20 2 lower", "RSI 14", "MACD", "Signal", "Histogram"]);
  // Hiding the bands takes them out of the tooltip too.
  await chart.locator(".px-legend-entry").filter({ hasText: "BB 20 2" }).click();
  await page.mouse.move(at.x + 1, at.y);
  await expect(tip.locator(".px-tip-row")).toHaveCount(10);
});

test("the drawing tools draw a trend line and a note, select one and delete it", async ({ page }) => {
  await open(page);
  const chart = card(page, "technical");
  const canvas = chart.locator("canvas");
  const plot = chart.locator(".px-chart-plot");
  const centred = await centre(plot);
  // The price's plot is the top of the chart, above its panes.
  const box = await plot.boundingBox();
  const at = { ...centred, y: (box?.y ?? 0) + (box?.height ?? 0) * 0.2 };
  // Away from the pointer, so no tooltip covers what is drawn.
  await page.mouse.move(at.x, at.y - 400);
  const before = await inked(canvas);
  const remove = chart.getByRole("button", { name: "Delete drawing" });
  await expect(remove).toBeDisabled();
  await chart.getByRole("button", { name: "Trend line" }).click();
  await expect(chart.getByRole("button", { name: "Trend line" })).toHaveAttribute("aria-pressed", "true");
  await page.mouse.move(at.x - at.width / 4, at.y + 40);
  await page.mouse.down();
  await page.mouse.move(at.x + at.width / 4, at.y - 30, { steps: 5 });
  await page.mouse.up();
  // The new line is selected and the tool put down.
  await expect(remove).toBeEnabled();
  await expect(chart.getByRole("button", { name: "Trend line" })).toHaveAttribute("aria-pressed", "false");
  await page.mouse.move(at.x, at.y - 400);
  await expect.poll(() => inked(canvas)).not.toBe(before);
  // Delete removes it, and the chart draws as before.
  await plot.focus();
  await page.keyboard.press("Delete");
  await expect(remove).toBeDisabled();
  await expect.poll(() => inked(canvas)).toBe(before);
  // A note asks for its text and keeps it on Enter.
  await chart.getByRole("button", { name: "Note" }).click();
  await page.mouse.click(at.x, at.y);
  const text = chart.getByRole("textbox", { name: "Note text" });
  await expect(text).toBeFocused();
  await text.fill("Breakout");
  await text.press("Enter");
  await expect(text).toHaveCount(0);
  await expect(remove).toBeEnabled();
  await page.mouse.move(at.x, at.y - 400);
  await expect.poll(() => inked(canvas)).not.toBe(before);
});
