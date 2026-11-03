/** The demo's own controls: its views, theme, row density and the performance overlay. */

import { expect, test } from "@playwright/test";
import type { Locator, Page } from "@playwright/test";

async function ready(page: Page): Promise<void> {
  await expect(page.locator('[role="grid"]').first()).toHaveAttribute("aria-rowcount", /^[1-9]\d*$/, { timeout: 120_000 });
}

test.beforeEach(async ({ context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
});

test("every view opens from its tab, and the browser-held ones say so", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await ready(page);
  const titles = await page.getByRole("navigation", { name: "Views" }).getByRole("link").allTextContents();
  expect(titles).toEqual(["Positions pivot", "Pivot in the browser", "1,000,000 trades", "Largest trades", "Daily prices"]);
  for (const [title, where] of [
    ["1,000,000 trades", null],
    ["Daily prices", "in browser"],
    ["Pivot in the browser", "in browser"],
    ["Positions pivot", null]
  ] as const) {
    await page.getByRole("link", { name: title }).click();
    await ready(page);
    await expect(page.getByRole("link", { name: title })).toHaveAttribute("aria-current", "page");
    await expect(page.locator(".px-error")).toHaveCount(0);
    if (where === null) {
      await expect(page.locator(".px-status-where")).toHaveCount(0);
    } else {
      await expect(page.locator(".px-status-where")).toHaveText(where);
    }
  }
  expect(errors).toEqual([]);
});

test("the theme and row density follow their pickers and survive moving to another view", async ({ page }) => {
  // A pivot, whose few thousand rows are short enough that the row height shows in the scroll height.
  await page.goto("/?preset=pivot");
  await ready(page);
  const token = (): Promise<string> => page.locator("px-grid").evaluate((element) => getComputedStyle(element).getPropertyValue("--px-grid-bg").trim());
  const contentHeight = (): Promise<number> => page.locator(".px-scroll").evaluate((element) => element.scrollHeight);
  expect(await token()).toBe("#ffffff");
  await page.getByLabel("Theme").selectOption("dark");
  await expect.poll(token).toBe("#16181d");
  const compact = await contentHeight();
  await page.getByLabel("Row height").selectOption("comfortable");
  await expect.poll(contentHeight).toBeGreaterThan(compact);
  await page.getByRole("link", { name: "Largest trades" }).click();
  await ready(page);
  await expect(page.getByRole("link", { name: "Largest trades" })).toHaveAttribute("aria-current", "page");
  await expect(page.getByLabel("Theme")).toHaveValue("dark");
  await expect(page.getByLabel("Row height")).toHaveValue("comfortable");
  expect(await token()).toBe("#16181d");
});

test("the performance overlay measures the page and copies its report", async ({ page }) => {
  await page.goto("/?preset=trades");
  await ready(page);
  await page.getByRole("button", { name: "Performance" }).click();
  const figure = (key: string): Locator => page.locator(`demo-performance [data-figure="${key}"]`);
  await expect(figure("frame")).toContainText("median", { timeout: 10_000 });
  // Scrolling draws and asks for rows, which gives the drawing and request-to-draw figures.
  for (const top of [2_000, 20_000, 200_000, 2_000_000]) {
    await page.locator(".px-scroll").evaluate((element, to) => {
      element.scrollTo({ top: to });
    }, top);
    await page.waitForTimeout(300);
  }
  await expect(figure("latency")).toContainText("median", { timeout: 10_000 });
  await expect(figure("draw")).toContainText(/[\d.]+ ms per frame/, { timeout: 10_000 });
  // The heap figure shows at once; the whole tab's, which an isolated page allows, arrives when the browser answers.
  await expect(figure("memory")).toContainText("MB", { timeout: 10_000 });
  await expect(figure("memory-scope")).toHaveText(/^(whole tab, workers included|JavaScript heap of the page, without workers or canvases)$/);
  await page.locator("demo-performance").getByRole("button", { name: "Copy report" }).click();
  await expect(page.locator("demo-performance [role=status]")).toHaveText("Copied");
  const report = await page.evaluate(() => navigator.clipboard.readText());
  expect(report).toContain("parallax performance report");
  expect(report).toContain("Showing: trades, grid, light theme, compact rows");
  expect(report).toMatch(/Rows asked to drawn: [\d.]+ ms median/);
  // examples/experiment.toml isolates the page, but a server already running may not.
  const isolated = await page.evaluate(() => crossOriginIsolated);
  expect(report).toContain(`Cross-origin isolated: ${isolated ? "yes" : "no"}`);
  expect(report).toMatch(/Memory: \d+ MB/);
});
