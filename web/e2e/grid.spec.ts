/** Functional checks of both grids against the reference server. */

import { expect, test } from "@playwright/test";

test("the DOM grid expands and collapses a pivot group", async ({ page }) => {
  await page.goto("/?grid=dom&preset=pivot");
  const rows = page.locator(".px-row:not(.px-header)");
  await expect(rows.first()).toContainText("Total", { timeout: 60_000 });
  const firstGroup = page.locator("button.px-tree:not([disabled])").nth(1);
  const label = (await firstGroup.textContent())?.trim() ?? "";
  expect(label.startsWith("▾")).toBe(true);
  await firstGroup.click();
  await expect(page.locator("button.px-tree:not([disabled])").nth(1)).toContainText("▸");
  await page.locator("button.px-tree:not([disabled])").nth(1).click();
  await expect(page.locator("button.px-tree:not([disabled])").nth(1)).toContainText("▾");
});

test("scrolling right reaches columns beyond the first window", async ({ page }) => {
  await page.goto("/?grid=dom&preset=pivot");
  const header = page.locator(".px-header");
  await expect(header).toContainText("Bond", { timeout: 60_000 });
  await page.locator(".px-scroll").evaluate((element) => {
    element.scrollTo({ left: element.scrollWidth });
  });
  await expect(header).toContainText("Total |");
});

test("the canvas grid draws without errors and follows horizontal scrolling", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/?grid=canvas&preset=pivot");
  await expect(page.locator('[role="grid"]')).toHaveAttribute("aria-rowcount", /^[1-9]\d*$/, { timeout: 60_000 });
  await page.locator(".px-scroll").evaluate((element) => {
    element.scrollTo({ left: element.scrollWidth });
  });
  await page.waitForTimeout(500);
  await expect(page.locator(".px-error")).toHaveCount(0);
  expect(errors).toEqual([]);
});
