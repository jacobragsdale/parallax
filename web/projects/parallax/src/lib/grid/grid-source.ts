/**
 * The data side of the grid: the open view, the latest frame and table status,
 * and what to send when the layout changes.
 *
 * A layout change that only resizes columns sends nothing. One that only expands
 * or collapses groups sends those groups. Anything else sends the whole layout,
 * and the engine decides whether it must recompute or only reformat. A change of
 * dataset or parameters opens a new view.
 */

import { DestroyRef, effect, inject, signal, untracked } from "@angular/core";
import type { Signal } from "@angular/core";
import { DataType, Table, tableFromIPC } from "apache-arrow";

import { ParallaxClient } from "../client";
import type { DistinctValues, EngineLocation, Viewport, ViewHandle } from "../connection";
import { pathValue } from "../format";
import { isNumericType } from "../frame";
import type { FrameData } from "../frame";
import type { Layout, LayoutScalar } from "../layout";
import type { TableStatus } from "../proto/parallax_pb";

/** Rows and columns fetched beyond the visible ones, so small scrolls need no request. */
const ROW_MARGIN = 40;
const COLUMN_MARGIN = 2;

/** The kind of value a column holds. */
export type FieldKind = "boolean" | "integer" | "float" | "decimal" | "text" | "date" | "timestamp";

/** A column of the dataset, from the schema the engine sends when a view opens. */
export interface SourceField {
  readonly name: string;
  readonly kind: FieldKind;
}

function kindOf(type: unknown): FieldKind {
  if (DataType.isBool(type)) {
    return "boolean";
  }
  if (isNumericType(type)) {
    return DataType.isInt(type) ? "integer" : DataType.isFloat(type) ? "float" : "decimal";
  }
  if (DataType.isDate(type)) {
    return "date";
  }
  if (DataType.isTimestamp(type)) {
    return "timestamp";
  }
  return "text";
}

/** A group the user expanded or collapsed, as the layout records it. */
export interface ExpandedEntry {
  readonly path: readonly LayoutScalar[];
  readonly expanded: boolean;
}

/** Where a view runs, and why it runs on the server if it could have run in the browser. */
export interface ViewPlacement {
  readonly location: EngineLocation;
  readonly fallback: string | null;
}

/** The grid's view of a source: what to draw and how to ask for more. */
export interface GridSource {
  readonly frame: Signal<FrameData | null>;
  readonly error: Signal<string | null>;
  readonly status: Signal<TableStatus | null>;
  readonly placement: Signal<ViewPlacement | null>;
  readonly fields: Signal<readonly SourceField[]>;
  /** Makes rows and data columns visible, fetching only what is not already held or asked for. */
  readonly show: (visible: Viewport) => void;
  /** Expands or collapses the group in the frame's row `local`, returning what the layout should record. */
  readonly toggle: (frame: FrameData, local: number) => ExpandedEntry | null;
  /** Fetches a window once, without moving the view, for copying. */
  readonly fetch: (viewport: Viewport) => Promise<FrameData>;
  /** Lists a column's values under the view's other filters, for filter pick-lists. */
  readonly distinct: (column: string, search: string, limit: number) => Promise<DistinctValues>;
}

function covers(outer: Viewport, inner: Viewport, totalRows: number, totalColumns: number): boolean {
  const rowEnd = Math.min(inner.firstRow + inner.rowCount, totalRows);
  const columnEnd = Math.min(inner.firstColumn + inner.columnCount, totalColumns);
  return outer.firstRow <= inner.firstRow && outer.firstRow + outer.rowCount >= rowEnd && outer.firstColumn <= inner.firstColumn && outer.firstColumn + outer.columnCount >= columnEnd;
}

/** The rows and data columns a frame holds. */
function held(frame: FrameData): Viewport {
  return { firstRow: frame.firstRow, rowCount: frame.rowCount, firstColumn: frame.firstColumn, columnCount: frame.columns.length };
}

/** Returns the JSON path of the group in the frame's row `local`. */
export function groupPath(frame: FrameData, local: number): readonly LayoutScalar[] {
  const depth = frame.tree?.depth(local) ?? 0;
  return frame.groups.slice(0, depth).map((key) => pathValue(key.value(local)));
}

/** What the engine computes from a layout; widths and expansions are left out. */
function computedPart(layout: Layout): string {
  return JSON.stringify({ ...layout, column_widths: {}, expanded: [] });
}

function pathKey(path: readonly LayoutScalar[]): string {
  return JSON.stringify(path);
}

/** Records a group as expanded or collapsed, replacing any earlier entry for the same path. */
export function withExpanded(layout: Layout, entry: ExpandedEntry): Layout {
  const key = pathKey(entry.path);
  return { ...layout, expanded: [...layout.expanded.filter((e) => pathKey(e.path) !== key), { path: [...entry.path], expanded: entry.expanded }] };
}

function readFields(schema: Uint8Array): readonly SourceField[] {
  const table = tableFromIPC(schema);
  if (!(table instanceof Table)) {
    return [];
  }
  return table.schema.fields.map((f) => ({ name: f.name, kind: kindOf(f.type) }));
}

interface Current {
  readonly view: ViewHandle;
  readonly dataset: string;
  readonly parameters: string;
  /** The layout the engine is running, as far as this client has told it. */
  sent: Layout;
}

/** Opens a view for a grid and follows its layout. Call in an injection context. */
export function gridSource(layout: Signal<Layout | null>): GridSource {
  const client = inject(ParallaxClient);
  const frame = signal<FrameData | null>(null);
  const error = signal<string | null>(null);
  const status = signal<TableStatus | null>(null);
  const placement = signal<ViewPlacement | null>(null);
  const fields = signal<readonly SourceField[]>([]);
  let current: Current | null = null;
  let generation = 0;
  let destroyed = false;
  let visible: Viewport | null = null;

  const request = (view: ViewHandle, wanted: Viewport): void => {
    const shown = frame();
    if (shown !== null && covers(held(shown), wanted, shown.totalRows, shown.totalColumns)) {
      return;
    }
    const asked = view.requested();
    if (asked !== null && covers(asked, wanted, shown?.totalRows ?? Number.MAX_SAFE_INTEGER, shown?.totalColumns ?? Number.MAX_SAFE_INTEGER)) {
      return;
    }
    const firstRow = Math.max(0, wanted.firstRow - ROW_MARGIN);
    const firstColumn = Math.max(0, wanted.firstColumn - COLUMN_MARGIN);
    view.setViewport({
      firstRow,
      rowCount: wanted.firstRow - firstRow + wanted.rowCount + ROW_MARGIN,
      firstColumn,
      columnCount: wanted.firstColumn - firstColumn + wanted.columnCount + COLUMN_MARGIN
    });
  };

  const open = (next: Layout): void => {
    current?.view.close();
    current = null;
    frame.set(null);
    status.set(null);
    placement.set(null);
    fields.set([]);
    generation += 1;
    const mine = generation;
    client.connectionFor(next.dataset, next.parameters).then(
      ({ connection, fallback }) => {
        // The layout changed or the grid went away while connecting.
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
              frame.set(f);
            },
            opened: (opened) => {
              fields.set(readFields(opened.tableSchema));
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
        current = { view, dataset: next.dataset, parameters: JSON.stringify(next.parameters), sent: next };
        if (visible !== null) {
          request(view, visible);
        }
      },
      (failure: unknown) => {
        error.set(failure instanceof Error ? failure.message : String(failure));
      }
    );
  };

  /** Sends what changed between the layout the engine runs, `sent`, and `next`. */
  const follow = (view: ViewHandle, sent: Layout, next: Layout): void => {
    if (computedPart(sent) !== computedPart(next)) {
      view.update(JSON.stringify(next));
      return;
    }
    const before = new Map(sent.expanded.map((e) => [pathKey(e.path), e.expanded]));
    const after = new Set(next.expanded.map((e) => pathKey(e.path)));
    if ([...before.keys()].some((key) => !after.has(key))) {
      // A group went back to its default state; only a full update can say so.
      view.update(JSON.stringify(next));
      return;
    }
    for (const e of next.expanded) {
      if (before.get(pathKey(e.path)) !== e.expanded) {
        view.setExpanded(e.path, e.expanded);
      }
    }
  };

  effect(() => {
    const next = layout();
    if (next === null) {
      return;
    }
    untracked(() => {
      if (current === null || current.view.isClosed || current.dataset !== next.dataset || current.parameters !== JSON.stringify(next.parameters)) {
        open(next);
      } else {
        const sent = current.sent;
        current.sent = next;
        follow(current.view, sent, next);
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
    error,
    status,
    placement,
    fields,
    show: (wanted) => {
      visible = wanted;
      if (current !== null) {
        request(current.view, wanted);
      }
    },
    toggle: (shown, local) => {
      const expanded = shown.tree?.expanded(local) ?? null;
      if (current === null || expanded === null) {
        return null;
      }
      const entry: ExpandedEntry = { path: groupPath(shown, local), expanded: !expanded };
      current.view.setExpanded(entry.path, entry.expanded);
      current.sent = withExpanded(current.sent, entry);
      return entry;
    },
    fetch: (viewport) => (current === null ? Promise.reject(new Error("the view is not open yet")) : current.view.fetch(viewport)),
    distinct: (column, search, limit) => (current === null ? Promise.reject(new Error("the view is not open yet")) : current.view.distinctValues(column, search, limit))
  };
}
