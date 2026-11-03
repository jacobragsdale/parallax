/** Datasets copied into the browser: views run in the page's WebAssembly worker. */

import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

async function open(page: Page, preset: string): Promise<void> {
  await page.goto(`/?preset=${preset}`);
  await expect(page.locator('[role="grid"]')).toHaveAttribute("aria-rowcount", /^[1-9]\d*$/, { timeout: 120_000 });
}

/** Selects every cell and returns what Ctrl+C puts on the clipboard. */
async function copyAll(page: Page): Promise<string> {
  await page.locator("canvas.px-canvas").click({ position: { x: 40, y: 40 } });
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("ControlOrMeta+c");
  await expect(page.locator(".px-status-message")).toContainText("Copied", { timeout: 60_000 });
  return page.evaluate(() => navigator.clipboard.readText());
}

test.beforeEach(async ({ context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
});

test("a pivot over the browser's copy shows exactly the server pivot's cells", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await open(page, "pivot");
  await expect(page.locator(".px-status-where")).toHaveCount(0);
  const server = await copyAll(page);
  await open(page, "browser");
  await expect(page.locator(".px-status-where")).toHaveText("in browser");
  const browser = await copyAll(page);
  expect(server.split("\n").length).toBeGreaterThan(100);
  expect(browser).toBe(server);
  await page.locator(".px-status-info").click();
  await expect(page.locator(".px-info")).toContainText("in this browser, on a copy of the server's table");
  expect(errors).toEqual([]);
});

test("the browser's views follow layout changes without the server", async ({ page }) => {
  let sent = 0;
  page.on("websocket", (socket) => {
    socket.on("framesent", () => {
      sent += 1;
    });
  });
  await open(page, "browser");
  const before = Number(await page.locator('[role="grid"]').getAttribute("aria-rowcount"));
  const sentBefore = sent;
  // Collapsing the first fund is answered by the worker, so nothing goes to the server.
  await page.locator("canvas.px-canvas").click({ position: { x: 24, y: 24 * 2 + 12 } });
  await expect(page.locator('[role="grid"]')).not.toHaveAttribute("aria-rowcount", String(before), { timeout: 30_000 });
  await page.locator(".px-scroll").evaluate((element) => {
    element.scrollTo({ top: element.scrollHeight });
  });
  await page.waitForTimeout(500);
  expect(sentBefore).toBeGreaterThan(0);
  expect(sent).toBe(sentBefore);
});
