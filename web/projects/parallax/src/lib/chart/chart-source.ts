/**
 * The data side of a chart: the open view, its latest chart frame or grouped
 * frame, and what to ask for when the layout or the plot's size changes.
 *
 * Kinds the engine reduces ask for a chart window: the x range shown at the
 * plot's size, again on every resize and zoom. Bar charts and heatmaps draw the
 * layout's grouped view, so they ask for its first rows like a pivot. Changing
 * the dataset or parameters opens a new view; changing what the engine computes
 * updates it; changing only how the chart is drawn, such as hidden series or the
 * scale, sends nothing.
 */

import { DestroyRef, effect, inject, signal, untracked } from "@angular/core";
import type { Signal } from "@angular/core";

import type { ChartFrameData } from "../chart-frame";
import { ParallaxClient } from "../client";
import type { ViewHandle } from "../connection";
import type { CellValue, FrameData } from "../frame";
import type { ViewPlacement } from "../grid/grid-source";
import type { Layout } from "../layout";
import type { TableStatus } from "../proto/parallax_pb";
import type { CategoryData } from "./model";

/** The most groups a bar chart or heatmap draws, and the most columns it reads. */
const MAX_GROUPS = 500;
const MAX_COLUMNS = 400;

/** The plot's size in CSS pixels. */
export interface PlotSize {
  readonly width: number;
  readonly height: number;
}

/** A chart's view of its source. */
export interface ChartSource {
  readonly frame: Signal<ChartFrameData | null>;
  readonly categories: Signal<CategoryData | null>;
  readonly status: Signal<TableStatus | null>;
  readonly error: Signal<string | null>;
  readonly placement: Signal<ViewPlacement | null>;
  /** Asks for the chart at a plot size, over an x range or everything. */
  readonly show: (size: PlotSize, range: readonly [number, number] | null) => void;
}

/** Whether a layout's chart draws its grouped view rather than a reduction. */
export function drawsGroups(layout: Layout): boolean {
  return layout.chart?.kind === "bar" || layout.chart?.kind === "heatmap";
}

/** The part of a layout the engine computes from: not the zoom, hidden series or scales, which only change drawing. */
export function computedPart(layout: Layout): string {
  const chart = layout.chart === null ? null : { ...layout.chart, range: null, hidden: [], log_y: false, stacked: false, horizontal: false };
  return JSON.stringify({ ...layout, column_widths: {}, chart });
}

function toNumber(value: CellValue): number {
  switch (value.kind) {
    case "number":
      return value.value;
    case "integer":
      return Number(value.value);
    case "decimal":
      // For drawing only: the text shows every digit.
      return Number(value.unscaled) / 10 ** value.scale;
    case "null":
    case "text":
    case "boolean":
    case "date":
    case "timestamp":
      return Number.NaN;
  }
}

/** A grouped frame's top-level groups and data columns, as a bar chart or heatmap draws them. */
export function categoriesOf(frame: FrameData): CategoryData {
  const tree = frame.tree;
  const rows: number[] = [];
  for (let row = 0; row < frame.rowCount; row += 1) {
    if (tree !== null && tree.depth(row) === 1 && tree.kind(row) !== "total") {
      rows.push(row);
    }
  }
  // Under column pivots, one series per pivot value and measure; otherwise one per measure.
  const pivoted = frame.columns.some((c) => c.split !== null);
  const columns = frame.columns.filter((c) => !pivoted || c.split !== null);
  // Under column pivots with one measure, every title ends with the same measure; the pivot values tell the series apart.
  const suffixes = new Set(columns.map((c) => c.title.slice(c.title.lastIndexOf(" | "))));
  const strip = pivoted && suffixes.size === 1 && columns.every((c) => c.title.includes(" | "));
  return {
    categories: rows.map((row) => tree?.label(row) ?? ""),
    series: columns.map((c) => ({ label: strip ? c.title.slice(0, c.title.lastIndexOf(" | ")) : c.title, values: rows.map((row) => toNumber(c.value(row))), texts: rows.map((row) => c.text(row)) }))
  };
}

interface Current {
  readonly view: ViewHandle;
  readonly source: string;
  computed: string;
  shape: string;
}

/** What a frame's data looks like: the chart's kind and the columns it plots. */
function shapeOf(layout: Layout): string {
  const c = layout.chart;
  return JSON.stringify(c === null ? null : [c.kind, c.x, c.y, c.y_right, c.series, c.ohlc, layout.group_by, layout.split_by, layout.measures]);
}

/** Opens a view for a chart and follows its layout. Call in an injection context. */
export function chartSource(layout: Signal<Layout | null>): ChartSource {
  const client = inject(ParallaxClient);
  const frame = signal<ChartFrameData | null>(null);
  const categories = signal<CategoryData | null>(null);
  const status = signal<TableStatus | null>(null);
  const error = signal<string | null>(null);
  const placement = signal<ViewPlacement | null>(null);
  let current: Current | null = null;
  let generation = 0;
  let destroyed = false;
  let wanted: { readonly size: PlotSize; readonly range: readonly [number, number] | null } | null = null;

  const request = (view: ViewHandle, groups: boolean): void => {
    if (wanted === null) {
      return;
    }
    if (groups) {
      view.setViewport({ firstRow: 0, rowCount: MAX_GROUPS, firstColumn: 0, columnCount: MAX_COLUMNS });
    } else {
      view.setChartWindow({ ...(wanted.range === null ? {} : { x: wanted.range }), width: wanted.size.width, height: wanted.size.height });
    }
  };

  const open = (next: Layout): void => {
    current?.view.close();
    current = null;
    frame.set(null);
    categories.set(null);
    status.set(null);
    placement.set(null);
    error.set(null);
    generation += 1;
    const mine = generation;
    client.connectionFor(next.dataset, next.parameters).then(
      ({ connection, fallback }) => {
        if (destroyed || mine !== generation) {
          return;
        }
        placement.set({ location: connection.location, fallback });
        const view = connection.open(
          next.dataset,
          JSON.stringify(next),
          {
            frame: (f) => {
              error.set(null);
              categories.set(categoriesOf(f));
            },
            chart: (f) => {
              error.set(null);
              frame.set(f);
            },
            opened: (opened) => {
              status.set(opened.status ?? null);
            },
            status: (s) => {
              status.set(s);
            },
            error: (message) => {
              error.set(message);
            }
          },
          next.parameters
        );
        current = { view, source: JSON.stringify([next.dataset, next.parameters]), computed: computedPart(next), shape: shapeOf(next) };
        request(view, drawsGroups(next));
      },
      (failure: unknown) => {
        // A failure for a layout since replaced, or a chart since gone, says nothing.
        if (!destroyed && mine === generation) {
          error.set(failure instanceof Error ? failure.message : String(failure));
        }
      }
    );
  };

  effect(() => {
    const next = layout();
    untracked(() => {
      if (next === null) {
        return;
      }
      const source = JSON.stringify([next.dataset, next.parameters]);
      if (current?.source !== source) {
        open(next);
        return;
      }
      const computed = computedPart(next);
      if (computed !== current.computed) {
        // Data of another shape must not be drawn as the new chart while the answer is on its way; the same shape
        // keeps its last frame until then.
        if (current.shape !== shapeOf(next)) {
          frame.set(null);
          categories.set(null);
        }
        current.shape = shapeOf(next);
        current.computed = computed;
        current.view.update(JSON.stringify(next));
        // A grouped view needs its rows asked for again after an update.
        request(current.view, drawsGroups(next));
      }
    });
  });

  inject(DestroyRef).onDestroy(() => {
    destroyed = true;
    current?.view.close();
    current = null;
  });

  return {
    frame,
    categories,
    status,
    error,
    placement,
    show: (size, range) => {
      wanted = { size, range };
      const next = layout();
      if (current !== null && next !== null) {
        request(current.view, drawsGroups(next));
      }
    }
  };
}
