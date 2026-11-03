/**
 * Every chart in the demo, in both themes, compared pixel for pixel with the screenshots in `e2e/screenshots`. Fonts
 * and text rendering differ between machines, so these run only in the pinned Playwright image: run them with
 * `pnpm screenshots`, and take new ones with `pnpm screenshots --update-snapshots`.
 */

import { expect, test } from "@playwright/test";

test.skip(process.env["PARALLAX_PINNED_IMAGE"] !== "1", "Screenshots are compared only in the pinned Playwright image: run pnpm screenshots.");

const CHARTS = 12;

for (const theme of ["light", "dark"] as const) {
  test(`every chart draws as in its screenshot, ${theme} theme`, async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto(`/?preset=charts&theme=${theme}`);
    const charts = page.locator("demo-charts section[data-card]").filter({ has: page.locator("px-chart") });
    await expect(charts).toHaveCount(CHARTS);
    // Every chart has its data once its status bar says what it shows.
    const rows = page.locator("px-chart .px-status-rows");
    await expect(rows).toHaveCount(CHARTS);
    await expect(rows.filter({ hasText: "loading" })).toHaveCount(0, { timeout: 60_000 });
    for (const chart of await charts.all()) {
      await chart.scrollIntoViewIfNeeded();
      const id = (await chart.getAttribute("data-card")) ?? "";
      // The status bar tells when the data was read, which changes with every run. In the pinned image drawing is
      // deterministic, so every pixel must match exactly: the default tolerance misses a change of a light wash.
      await expect(chart).toHaveScreenshot(`${id}-${theme}.png`, { mask: [chart.locator("px-status-bar")], animations: "disabled", caret: "hide", threshold: 0, maxDiffPixels: 0 });
    }
    expect(errors).toEqual([]);
  });
}

for (const theme of ["light", "dark"] as const) {
  test(`the technical-analysis page draws as in its screenshot, ${theme} theme`, async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto(`/?preset=technical&theme=${theme}`);
    const chart = page.locator("demo-technical section.chart");
    await expect(chart.locator(".px-status-rows")).not.toHaveText(/loading/, { timeout: 60_000 });
    await expect(chart).toHaveScreenshot(`technical-page-${theme}.png`, { mask: [chart.locator("px-status-bar")], animations: "disabled", caret: "hide", threshold: 0, maxDiffPixels: 0 });
    expect(errors).toEqual([]);
  });
}
