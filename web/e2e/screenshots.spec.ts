/**
 * Every chart in the demo, in both themes, compared with the screenshots in `e2e/screenshots`. Fonts and text
 * rendering differ between machines, so these run only in the pinned Playwright image: run them with
 * `pnpm screenshots`, and take new ones with `pnpm screenshots --update-snapshots`.
 */

import { expect, test } from "@playwright/test";

for (const theme of ["light", "dark"] as const) {
  test(`every chart draws as in its screenshot, ${theme} theme`, async ({ page }) => {
    await page.goto(`/?preset=charts&theme=${theme}`);
    const charts = page.locator("demo-charts section").filter({ has: page.locator("px-chart") });
    await expect(charts).toHaveCount(10);
    await expect(page.locator("px-chart .px-status-rows").filter({ hasText: "loading" })).toHaveCount(0, { timeout: 60_000 });
    for (const [i, chart] of (await charts.all()).entries()) {
      await chart.scrollIntoViewIfNeeded();
      const title = (await chart.locator("h2").textContent()) ?? String(i);
      const name = `${String(i + 1).padStart(2, "0")}-${title
        .toLowerCase()
        .replace(/[^a-z0-9]+/g, "-")
        .slice(0, 40)
        .replace(/^-|-$/g, "")}-${theme}.png`;
      // The status bar tells when the data was read, which changes with every run. In the pinned image drawing is
      // deterministic, so every pixel must match exactly: the default tolerance misses a change of a light wash.
      await expect(chart).toHaveScreenshot(name, { mask: [chart.locator("px-status-bar")], animations: "disabled", caret: "hide", threshold: 0, maxDiffPixels: 0 });
    }
  });
}
