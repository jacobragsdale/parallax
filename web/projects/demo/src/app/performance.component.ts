/**
 * The performance overlay, for checking the demo on a Citrix VDI. It shows the
 * figures the roadmap's budgets name and copies them as a report:
 *
 * - frame time, from `requestAnimationFrame`, as the median and 95th percentile
 *   of the last few seconds the tab was visible. While open, the overlay asks
 *   for a frame at every vsync to time them, which costs a little main-thread time
 *   of its own;
 * - main-thread time blocked by long frames, the Long Animation Frames API's
 *   blocking duration;
 * - the grid's own drawing time per frame, and the time from asking for rows to
 *   drawing them, from the grid's counters;
 * - tab memory, from `measureUserAgentSpecificMemory` when the page is
 *   cross-origin isolated, which then counts the worker too, or else Chromium's
 *   main-thread JavaScript heap.
 */

import { ChangeDetectionStrategy, Component, DestroyRef, computed, inject, input, signal } from "@angular/core";
import type { DrawStats } from "parallax";

/** How many recent frame intervals the figures cover. */
const FRAMES = 240;
/** How far back long frames count, in milliseconds. */
const WINDOW_MS = 10_000;

interface Memory {
  readonly bytes: number;
  readonly scope: string;
}

/** One sample of the counters the overlay shows. */
interface Figures {
  readonly frameP50: number;
  readonly frameP95: number;
  readonly longFrames: number;
  readonly blockedMsPerSecond: number;
  readonly drawMsPerFrame: number | null;
  readonly latencyP50: number | null;
  readonly latencyP95: number | null;
}

/** The `p`th percentile, interpolating between the two nearest ranks. */
export function percentile(values: readonly number[], p: number): number {
  const sorted = [...values].sort((a, b) => a - b);
  const rank = (p / 100) * (sorted.length - 1);
  const below = sorted[Math.floor(rank)];
  const above = sorted[Math.ceil(rank)];
  if (below === undefined || above === undefined) {
    return 0;
  }
  return below + (above - below) * (rank - Math.floor(rank));
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null;
}

/** Chromium's JavaScript heap figure for the page, available at once. */
function heapMemory(): Memory | null {
  const heap: unknown = Reflect.get(performance, "memory");
  if (isRecord(heap) && typeof heap["usedJSHeapSize"] === "number") {
    return { bytes: heap["usedJSHeapSize"], scope: "JavaScript heap of the page, without workers or canvases" };
  }
  return null;
}

/**
 * The whole tab's memory, workers included, when the page is cross-origin isolated. The browser answers after
 * its next garbage collection, which can take many seconds; null if it cannot measure.
 */
async function tabMemory(): Promise<Memory | null> {
  const measure: unknown = Reflect.get(performance, "measureUserAgentSpecificMemory");
  if (!crossOriginIsolated || typeof measure !== "function") {
    return null;
  }
  const result: unknown = await Reflect.apply(measure, performance, []);
  return isRecord(result) && typeof result["bytes"] === "number" ? { bytes: result["bytes"], scope: "whole tab, workers included" } : null;
}

/**
 * The grid's drawing time per frame between two samples of its counters, or `last` if it drew nothing since.
 * A layout change can replace the grid, whose counters then start again, so the figure starts again too.
 */
function drawTime(stats: DrawStats | null, previous: DrawStats | null, last: number | null): number | null {
  if (stats === null || previous === null) {
    return stats === null ? null : last;
  }
  if (stats.frames < previous.frames) {
    return null;
  }
  const frames = stats.frames - previous.frames;
  return frames > 0 ? (stats.drawMs - previous.drawMs) / frames : last;
}

const MEGABYTE = 1024 * 1024;

function ms(value: number | null): string {
  return value === null ? "n/a" : `${value.toFixed(1)} ms`;
}

@Component({
  selector: "demo-performance",
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <h2>Performance</h2>
    @if (lines(); as rows) {
      <dl>
        @for (row of rows; track row.term) {
          <dt>{{ row.term }}</dt>
          <dd [attr.data-figure]="row.key">{{ row.value }}</dd>
        }
      </dl>
    }
    <button type="button" (click)="copyReport()">Copy report</button>
    <span role="status">{{ note() }}</span>
  `,
  styles: `
    :host {
      position: fixed;
      right: 12px;
      bottom: 40px;
      z-index: 20;
      width: 330px;
      padding: 10px 12px;
      font:
        12px system-ui,
        sans-serif;
      color: #e6e9ef;
      background: rgb(20 24 33 / 92%);
      border-radius: 6px;
      box-shadow: 0 4px 16px rgb(0 0 0 / 30%);
    }
    h2 {
      margin: 0 0 6px;
      font-size: 13px;
    }
    dl {
      display: grid;
      grid-template-columns: auto 1fr;
      gap: 2px 10px;
      margin: 0 0 8px;
    }
    dt {
      color: #a3abbb;
    }
    dd {
      margin: 0;
      font-variant-numeric: tabular-nums;
      text-align: right;
    }
    span {
      margin-left: 8px;
    }
  `
})
export class PerformanceComponent {
  /** Reads the visible grid's counters, or null while there is none. */
  public readonly stats = input.required<() => DrawStats | null>();
  /** What the page shows, for the report. */
  public readonly context = input<string>("");

  private readonly figures = signal<Figures | null>(null);
  private readonly memory = signal<Memory | null>(null);
  protected readonly note = signal("");

  protected readonly lines = computed(() => {
    const f = this.figures();
    if (f === null) {
      return null;
    }
    const memory = this.memory();
    return [
      { key: "frame", term: "Frame time", value: `${ms(f.frameP50)} median, ${ms(f.frameP95)} p95` },
      { key: "blocked", term: "Long frames", value: `${f.longFrames.toString()} in 10 s, ${ms(f.blockedMsPerSecond)} blocked per s` },
      { key: "draw", term: "Grid drawing", value: `${ms(f.drawMsPerFrame)} per frame` },
      { key: "latency", term: "Rows asked to drawn", value: f.latencyP50 === null ? "n/a" : `${ms(f.latencyP50)} median, ${ms(f.latencyP95)} p95` },
      { key: "memory", term: "Memory", value: memory === null ? "n/a" : `${(memory.bytes / MEGABYTE).toFixed(0)} MB` },
      { key: "memory-scope", term: "Memory covers", value: memory?.scope ?? "not measurable in this browser" }
    ];
  });

  constructor() {
    const intervals: number[] = [];
    const longFrames: { readonly at: number; readonly blocked: number }[] = [];
    // A hidden tab draws nothing, so the interval spanning its absence is not a frame.
    let last: number | null = null;
    const onVisibility = (): void => {
      last = null;
    };
    document.addEventListener("visibilitychange", onVisibility);
    let handle = requestAnimationFrame(function tick(now: number): void {
      if (last !== null && !document.hidden) {
        intervals.push(now - last);
        if (intervals.length > FRAMES) {
          intervals.shift();
        }
      }
      last = now;
      handle = requestAnimationFrame(tick);
    });
    let observer: PerformanceObserver | null = null;
    if (PerformanceObserver.supportedEntryTypes.includes("long-animation-frame")) {
      observer = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          // The time long tasks in the frame ran past 50 ms each, as the API reports it.
          const blocking: unknown = Reflect.get(entry, "blockingDuration");
          longFrames.push({ at: entry.startTime, blocked: typeof blocking === "number" ? blocking : 0 });
        }
      });
      observer.observe({ type: "long-animation-frame", buffered: false });
    }
    let previous: DrawStats | null = null;
    // An idle grid draws nothing, so the figure keeps the last second that drew.
    let drawMsPerFrame: number | null = null;
    const sample = (): void => {
      const now = performance.now();
      while (longFrames.length > 0 && (longFrames[0]?.at ?? now) < now - WINDOW_MS) {
        longFrames.shift();
      }
      const stats = this.stats()();
      drawMsPerFrame = drawTime(stats, previous, drawMsPerFrame);
      previous = stats;
      const latencies = stats?.latencyMs ?? [];
      this.figures.set({
        frameP50: percentile(intervals, 50),
        frameP95: percentile(intervals, 95),
        longFrames: longFrames.length,
        blockedMsPerSecond: longFrames.reduce((sum, f) => sum + f.blocked, 0) / (WINDOW_MS / 1000),
        drawMsPerFrame,
        latencyP50: latencies.length === 0 ? null : percentile(latencies, 50),
        latencyP95: latencies.length === 0 ? null : percentile(latencies, 95)
      });
    };
    // The heap figure shows at once; the whole tab's replaces it whenever the browser answers.
    let measuring = false;
    let precise = false;
    const measure = (): void => {
      if (!precise) {
        this.memory.set(heapMemory());
      }
      if (measuring) {
        return;
      }
      measuring = true;
      tabMemory().then(
        (memory) => {
          measuring = false;
          if (memory !== null) {
            precise = true;
            this.memory.set(memory);
          }
        },
        () => {
          measuring = false;
        }
      );
    };
    measure();
    const sampler = setInterval(sample, 1000);
    const memoryTimer = setInterval(measure, 10_000);
    inject(DestroyRef).onDestroy(() => {
      cancelAnimationFrame(handle);
      document.removeEventListener("visibilitychange", onVisibility);
      observer?.disconnect();
      clearInterval(sampler);
      clearInterval(memoryTimer);
    });
  }

  /** The figures and the machine they were taken on, as plain text. */
  protected report(): string {
    const lines = this.lines() ?? [];
    return [
      "parallax performance report",
      `Taken: ${new Date().toISOString()}`,
      `Page: ${location.href}`,
      `Showing: ${this.context()}`,
      `Browser: ${navigator.userAgent}`,
      `Logical processors: ${navigator.hardwareConcurrency.toString()}`,
      `Window: ${innerWidth.toString()} x ${innerHeight.toString()} at ${devicePixelRatio.toString()}x`,
      `Cross-origin isolated: ${crossOriginIsolated ? "yes" : "no"}`,
      ...lines.map((l) => `${l.term}: ${l.value}`)
    ].join("\n");
  }

  protected copyReport(): void {
    navigator.clipboard.writeText(this.report()).then(
      () => {
        this.note.set("Copied");
      },
      (failure: unknown) => {
        this.note.set(`Could not copy: ${failure instanceof Error ? failure.message : String(failure)}`);
      }
    );
  }
}
