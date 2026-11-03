/** Functional checks of `<px-grid>` against the reference server. */

import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

const ROW = 24;

async function open(page: Page, query: string): Promise<void> {
  await page.goto(`/?${query}`);
  await expect(page.locator('[role="grid"]')).toHaveAttribute("aria-rowcount", /^[1-9]\d*$/, { timeout: 60_000 });
}

async function rowCount(page: Page): Promise<number> {
  return Number(await page.locator('[role="grid"]').getAttribute("aria-rowcount"));
}

async function layout(page: Page): Promise<unknown> {
  const text = await page.getByTestId("layout-json").textContent({ timeout: 5_000 });
  return JSON.parse(text ?? "null");
}

/** Clicks the canvas at a point inside the grid. */
async function clickAt(page: Page, x: number, y: number, modifiers: ("Shift" | "ControlOrMeta")[] = []): Promise<void> {
  await page.locator("canvas.px-canvas").click({ position: { x, y }, modifiers });
}

async function copied(page: Page): Promise<string> {
  await page.keyboard.press("ControlOrMeta+c");
  await expect(page.locator(".px-status-message")).toContainText("Copied", { timeout: 30_000 });
  return page.evaluate(() => navigator.clipboard.readText());
}

test.beforeEach(async ({ context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
});

test("the pivot draws, and the status bar says where the numbers come from", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await open(page, "preset=pivot");
  await expect(page.locator(".px-status-dataset")).toHaveText("positions");
  await expect(page.locator(".px-status-rows")).toContainText("rows");
  await page.locator(".px-status-info").click();
  await expect(page.locator(".px-info")).toContainText("Rows in table");
  await page.locator(".px-scroll").evaluate((element) => {
    element.scrollTo({ left: element.scrollWidth });
  });
  await page.waitForTimeout(300);
  await expect(page.locator(".px-error")).toHaveCount(0);
  expect(errors).toEqual([]);
});

// Row 0 is the grand total; row 1 is the first fund, expanded by default. Its
// marker sits one indent in: 6 px of padding plus 14 px per level.
const FIRST_FUND_MARKER = 24;

test("clicking a group's marker collapses and expands it, and the layout records it", async ({ page }) => {
  await open(page, "preset=pivot");
  const before = await rowCount(page);
  await clickAt(page, FIRST_FUND_MARKER, ROW * 2 + ROW / 2);
  await expect.poll(() => rowCount(page)).toBeLessThan(before);
  expect(await layout(page)).toMatchObject({ expanded: [{ expanded: false }] });
  await clickAt(page, FIRST_FUND_MARKER, ROW * 2 + ROW / 2);
  await expect.poll(() => rowCount(page)).toBe(before);
});

test("clicking a group's label selects it without toggling; a double click toggles", async ({ page }) => {
  await open(page, "preset=pivot");
  const before = await rowCount(page);
  await clickAt(page, 120, ROW * 2 + ROW / 2);
  await page.waitForTimeout(300);
  expect(await rowCount(page)).toBe(before);
  expect((await copied(page)).trim()).toBe("Atlas Global Macro");
  await page.locator("canvas.px-canvas").dblclick({ position: { x: 120, y: ROW * 2 + ROW / 2 } });
  await expect.poll(() => rowCount(page)).toBeLessThan(before);
});

test("clicking a header sorts, and copying carries every digit", async ({ page }) => {
  await open(page, "preset=trades");
  // trade_id is the first column; two clicks sort it descending.
  await clickAt(page, 40, ROW / 2);
  await expect.poll(async () => layout(page)).toMatchObject({ sort: [{ column: "trade_id", descending: false }] });
  await clickAt(page, 40, ROW / 2);
  await expect.poll(async () => layout(page)).toMatchObject({ sort: [{ column: "trade_id", descending: true }] });
  await clickAt(page, 40, ROW + ROW / 2);
  const total = await rowCount(page);
  expect((await copied(page)).trim()).toBe(String(total));
});

/** Counts pixels in a canvas rectangle that differ from its top-left pixel: zero means nothing is drawn there. */
async function inkIn(page: Page, x: number, y: number, width: number, height: number): Promise<number> {
  return page.locator("canvas.px-canvas").evaluate(
    (canvas, box) => {
      if (!(canvas instanceof HTMLCanvasElement)) {
        throw new Error("not a canvas");
      }
      const context = canvas.getContext("2d");
      if (context === null) {
        throw new Error("no context");
      }
      const ratio = window.devicePixelRatio;
      const pixels = context.getImageData(box.x * ratio, box.y * ratio, box.width * ratio, box.height * ratio).data;
      let ink = 0;
      for (let i = 0; i < pixels.length; i += 4) {
        if (pixels[i] !== pixels[0] || pixels[i + 1] !== pixels[1] || pixels[i + 2] !== pixels[2]) {
          ink += 1;
        }
      }
      return ink;
    },
    { x, y, width, height }
  );
}

test("every column in view is drawn, not only the first few", async ({ page }) => {
  await open(page, "preset=trades");
  // The header and first cell of the eighth column, inside its borders.
  await expect.poll(() => inkIn(page, 7 * 140 + 2, 2, 130, ROW - 4)).toBeGreaterThan(0);
  await expect.poll(() => inkIn(page, 7 * 140 + 2, ROW + 2, 130, ROW - 4)).toBeGreaterThan(0);
});

test("dragging a header edge resizes the column into the layout", async ({ page }) => {
  await open(page, "preset=trades");
  const canvas = page.locator("canvas.px-canvas");
  const box = await canvas.boundingBox();
  if (box === null) {
    throw new Error("no canvas");
  }
  await page.mouse.move(box.x + 139, box.y + ROW / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + 200, box.y + ROW / 2, { steps: 4 });
  await page.mouse.up();
  await expect.poll(async () => layout(page)).toMatchObject({ column_widths: { trade_id: 201 } });
});

test("a range copies as tab-separated rows", async ({ page }) => {
  await open(page, "preset=trades");
  await clickAt(page, 40, ROW + ROW / 2);
  await clickAt(page, 200, ROW * 3 + ROW / 2, ["Shift"]);
  const lines = (await copied(page)).split("\r\n");
  expect(lines).toHaveLength(3);
  expect(lines.every((line) => line.split("\t").length === 2)).toBe(true);
  expect(lines[0]?.split("\t")[0]).toBe("1");
});

test("copying more rows than are loaded fetches them first", async ({ page }) => {
  await open(page, "preset=pivot");
  await clickAt(page, 60, ROW + ROW / 2);
  await page.keyboard.press("ControlOrMeta+a");
  const text = await copied(page);
  expect(text.split("\r\n")).toHaveLength(await rowCount(page));
});

test("on a page that is not a secure context, rows fetched for a copy wait for a second Ctrl+C", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(window, "isSecureContext", { value: false });
  });
  await open(page, "preset=pivot");
  await clickAt(page, 120, ROW + ROW / 2);
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("ControlOrMeta+c");
  await expect(page.locator(".px-status-message")).toContainText("press Ctrl+C again", { timeout: 30_000 });
  const text = await copied(page);
  expect(text.split("\r\n")).toHaveLength(await rowCount(page));
});

test("a copy beyond the cell limit is refused with a reason", async ({ page }) => {
  await open(page, "preset=trades");
  await clickAt(page, 40, ROW + ROW / 2);
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("ControlOrMeta+c");
  await expect(page.locator(".px-status-message")).toContainText("cells", { timeout: 30_000 });
});

test("the dark theme sets its tokens", async ({ page }) => {
  await open(page, "preset=pivot&theme=dark");
  const background = await page.locator("px-grid").evaluate((element) => getComputedStyle(element).getPropertyValue("--px-grid-bg").trim());
  expect(background).toBe("#16181d");
});
