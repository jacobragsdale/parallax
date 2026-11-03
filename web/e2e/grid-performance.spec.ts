/**
 * Scrolling performance of `<px-grid>` in Chromium with the GPU disabled and the
 * CPU throttled four times, as a stand-in for a Citrix desktop. It repeats the
 * method of the grid experiment (docs/grid-experiment.md) for the canvas grid.
 * Results go to test-results/grid-performance.json.
 */

import { mkdirSync, writeFileSync } from "node:fs";

import { expect, test } from "@playwright/test";
import type { CDPSession, Page } from "@playwright/test";

const PRESETS = ["trades", "pivot"] as const;
const STEPS = 240;

interface Result {
  readonly preset: string;
  readonly mode: string;
  readonly frames: number;
  readonly meanFrameMs: number;
  readonly p95FrameMs: number;
  readonly maxFrameMs: number;
  readonly mainThreadMsPerFrame: number;
  readonly scriptMsPerFrame: number;
  readonly layoutAndStyleMsPerFrame: number;
}

const results: Result[] = [];

async function metrics(cdp: CDPSession): Promise<ReadonlyMap<string, number>> {
  const response = await cdp.send("Performance.getMetrics");
  return new Map(response.metrics.map((m) => [m.name, m.value]));
}

function delta(before: ReadonlyMap<string, number>, after: ReadonlyMap<string, number>, name: string): number {
  return ((after.get(name) ?? 0) - (before.get(name) ?? 0)) * 1000;
}

function percentile(values: readonly number[], p: number): number {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))] ?? 0;
}

async function scroll(page: Page, rowsPerStep: number): Promise<readonly number[]> {
  return page.evaluate(
    async ({ steps, rows }) => {
      const scroller = document.querySelector(".px-scroll");
      if (!(scroller instanceof HTMLElement)) {
        throw new Error("no grid scroller");
      }
      const intervals: number[] = [];
      let last = performance.now();
      for (let i = 0; i < steps; i += 1) {
        scroller.scrollTop += 24 * rows;
        await new Promise<void>((resolve) => {
          requestAnimationFrame(() => {
            resolve();
          });
        });
        const now = performance.now();
        intervals.push(now - last);
        last = now;
      }
      return intervals;
    },
    { steps: STEPS, rows: rowsPerStep }
  );
}

for (const preset of PRESETS) {
  test(`scrolling ${preset}`, async ({ page }) => {
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Performance.enable");
    await page.goto(`/?preset=${preset}`);
    const gridElement = page.locator('[role="grid"]');
    await expect(gridElement).toHaveAttribute("aria-rowcount", /^[1-9]\d*$/, { timeout: 120_000 });
    await page.waitForTimeout(500);
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
    for (const [mode, rows] of [
      ["smooth", 3],
      ["jumping", 40]
    ] as const) {
      const before = await metrics(cdp);
      const intervals = await scroll(page, rows);
      const after = await metrics(cdp);
      const frames = intervals.length;
      results.push({
        preset,
        mode,
        frames,
        meanFrameMs: intervals.reduce((a, b) => a + b, 0) / frames,
        p95FrameMs: percentile(intervals, 0.95),
        maxFrameMs: Math.max(...intervals),
        mainThreadMsPerFrame: delta(before, after, "TaskDuration") / frames,
        scriptMsPerFrame: delta(before, after, "ScriptDuration") / frames,
        layoutAndStyleMsPerFrame: (delta(before, after, "LayoutDuration") + delta(before, after, "RecalcStyleDuration")) / frames
      });
    }
    await cdp.send("Emulation.setCPUThrottlingRate", { rate: 1 });
  });
}

test.afterAll(() => {
  mkdirSync("test-results", { recursive: true });
  writeFileSync("test-results/grid-performance.json", JSON.stringify(results, null, 2));
  console.table(
    results.map((r) => ({
      ...r,
      meanFrameMs: r.meanFrameMs.toFixed(1),
      p95FrameMs: r.p95FrameMs.toFixed(1),
      maxFrameMs: r.maxFrameMs.toFixed(1),
      mainThreadMsPerFrame: r.mainThreadMsPerFrame.toFixed(2),
      scriptMsPerFrame: r.scriptMsPerFrame.toFixed(2),
      layoutAndStyleMsPerFrame: r.layoutAndStyleMsPerFrame.toFixed(2)
    }))
  );
});
