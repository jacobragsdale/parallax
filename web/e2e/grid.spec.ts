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

test.describe("px-pivot", () => {
  test("removing a row group from the settings regroups the pivot", async ({ page }) => {
    await open(page, "preset=pivot");
    const before = await rowCount(page);
    await page.getByRole("button", { name: "Remove ticker" }).click();
    await expect.poll(async () => layout(page)).toMatchObject({ group_by: ["fund", "strategy", "sector"] });
    await expect.poll(() => rowCount(page)).toBeLessThan(before);
  });

  test("a value added in the settings becomes a column", async ({ page }) => {
    await open(page, "preset=pivot");
    await page.getByRole("button", { name: "Add value" }).click();
    await expect.poll(async () => layout(page)).toMatchObject({ measures: [{}, {}, {}, {}] });
    await expect(page.locator(".px-error")).toHaveCount(0);
  });

  test("a filter picked from the column's values narrows the pivot", async ({ page }) => {
    await open(page, "preset=pivot");
    const before = await rowCount(page);
    // A new filter opens its value list at once.
    await page.getByRole("button", { name: "Add filter" }).click();
    const first = page.locator(".px-popover input[type=checkbox]").first();
    await expect(first).toBeVisible({ timeout: 30_000 });
    await first.check();
    await expect.poll(() => rowCount(page)).toBeLessThan(before);
    // The list stays open, so a second value can be ticked straight away.
    await expect(page.locator(".px-popover input[type=checkbox]").nth(1)).toBeVisible();
    expect(await layout(page)).toMatchObject({ filters: [{ op: "in", values: [expect.any(String)] }] });
  });

  test("double-clicking a value opens the rows behind its group", async ({ page }) => {
    await open(page, "preset=pivot");
    // Row 1 is the first fund; its first value sits just right of the group column.
    await page
      .locator("px-pivot-main canvas.px-canvas, .px-pivot-main > px-grid canvas.px-canvas")
      .first()
      .dblclick({ position: { x: 320, y: ROW * 2 + ROW / 2 } });
    const drawer = page.locator(".px-drill");
    // The first value column sits under the Bond pivot, so the drawer is the fund's bonds.
    await expect(drawer).toContainText("Rows behind Atlas Global Macro, Bond");
    await expect(drawer.locator(".px-status-rows")).toContainText("rows", { timeout: 30_000 });
    await expect(drawer.locator(".px-error")).toHaveCount(0);
  });

  test("a shared link reopens the same view", async ({ page }) => {
    await open(page, "preset=pivot");
    await page.getByRole("button", { name: "Remove ticker" }).click();
    await page.getByRole("button", { name: "Copy link to this view" }).click();
    await expect(page.getByRole("status").first()).toContainText("links to this view");
    const url = page.url();
    expect(url).toContain("layout=1.");
    await page.goto(url);
    await expect.poll(async () => layout(page)).toMatchObject({ group_by: ["fund", "strategy", "sector"] });
  });

  test("a link to a flat view opens a grid, whatever the preset", async ({ page }) => {
    await open(page, "preset=trades");
    await page.getByRole("button", { name: "Copy link to this view" }).click();
    await expect(page.getByRole("status").first()).toContainText("links to this view");
    const link = new URL(page.url()).searchParams.get("layout") ?? "";
    await page.goto(`/?preset=pivot&layout=${link}`);
    await expect(page.locator(".px-status-dataset")).toHaveText("trades", { timeout: 60_000 });
    await expect(page.locator("px-pivot")).toHaveCount(0);
  });

  test("a value picked as blank matches the empty groups", async ({ page }) => {
    await open(page, "preset=pivot");
    // sector has blanks in the generated positions.
    await page.getByRole("button", { name: "Remove fund" }).click();
    await page.getByRole("button", { name: "Remove strategy" }).click();
    await page.getByRole("button", { name: "Add filter" }).click();
    const blank = page.locator(".px-popover label", { hasText: "(blank)" }).locator("input");
    await expect(blank).toBeVisible({ timeout: 30_000 });
    await blank.check();
    await expect.poll(async () => layout(page)).toMatchObject({ filters: [{ column: "sector", op: "in", values: [null] }] });
    await expect(page.locator(".px-error")).toHaveCount(0);
  });

  test("changing a filter to a list keeps every row until a value is picked", async ({ page }) => {
    await open(page, "preset=pivot");
    const before = await rowCount(page);
    await page.getByRole("button", { name: "Add filter" }).click();
    await page.locator(".px-popover input[type=checkbox]").first().check();
    await expect.poll(() => rowCount(page)).toBeLessThan(before);
    // Moving the committed filter to another column clears its values, so it becomes a draft again.
    await page.getByRole("combobox", { name: "Filter column" }).first().selectOption("strategy");
    await expect.poll(() => rowCount(page)).toBe(before);
    expect(await layout(page)).toMatchObject({ filters: [] });
  });
});

test.describe("export", () => {
  test("Export to Excel downloads a workbook of the view", async ({ page }) => {
    await open(page, "preset=pivot");
    const download = page.waitForEvent("download", { timeout: 60_000 });
    await page.getByRole("button", { name: "Export to Excel" }).click();
    const file = await download;
    expect(file.suggestedFilename()).toMatch(/^positions-\d{4}-\d{2}-\d{2}-\d{4}\.xlsx$/);
    const path = await file.path();
    const bytes = await import("node:fs").then((fs) => fs.readFileSync(path));
    expect(bytes.subarray(0, 2).toString()).toBe("PK");
  });

  test("a refused export says why in the status bar", async ({ page }) => {
    // Answer the post as the server answers a refusal from another origin.
    await page.route("**/export", async (route) => {
      const token = new URLSearchParams(route.request().postData() ?? "").get("token") ?? "";
      const payload = JSON.stringify({ source: "parallax-export", token, error: "the view has 2000000 rows, more than the 1048576 an Excel sheet holds" });
      await route.fulfill({ status: 422, contentType: "text/html", body: `<script>parent.postMessage(${payload}, "*");</script>` });
    });
    await open(page, "preset=pivot");
    await page.getByRole("button", { name: "Export to Excel" }).click();
    await expect(page.locator(".px-status-message")).toContainText("Export failed: the view has 2000000 rows");
  });

  test("CSV downloads every digit of the view", async ({ page }) => {
    await open(page, "preset=pivot");
    const download = page.waitForEvent("download", { timeout: 60_000 });
    await page.getByRole("button", { name: "CSV" }).click();
    const file = await download;
    const text = await import("node:fs").then(async (fs) => fs.readFileSync(await file.path(), "utf8"));
    expect(text.startsWith("\uFEFF")).toBe(true);
    const lines = text.slice(1).split("\n");
    expect(lines[0]?.startsWith("level,fund,strategy,sector,ticker,Bond | sum(market_value_usd),")).toBe(true);
    expect(lines[0]?.endsWith(",Total | count(*)")).toBe(true);
    expect(lines[1]?.startsWith("0,,,,,")).toBe(true);
  });
});
