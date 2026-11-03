/**
 * The data side shared by both grids: the open view, the latest frame, and the
 * mapping from scroll position to rows and columns. The two grids differ only in
 * how they draw.
 */

import { DestroyRef, effect, inject, signal, untracked } from "@angular/core";
import type { InputSignal, Signal } from "@angular/core";

import { ParallaxClient } from "../client";
import type { Viewport, ViewHandle } from "../connection";
import { pathValue } from "../format";
import type { FrameData } from "../frame";

/** Width of the tree column of a grouped view, in CSS pixels. */
export const TREE_WIDTH = 260;
/** Width of a data column, in CSS pixels. */
export const CELL_WIDTH = 140;

/** Browsers cap element heights near 33 million pixels; stay well below. */
const MAX_SCROLL_PX = 15_000_000;
/** Rows and columns fetched beyond the visible ones, so small scrolls need no request. */
const ROW_MARGIN = 40;
const COLUMN_MARGIN = 2;

/** A JSON value in a group path. */
export type PathValue = string | number | boolean | null;

/** The grid's view of a source: what to draw and how to ask for more. */
export interface GridSource {
  readonly frame: Signal<FrameData | null>;
  readonly error: Signal<string | null>;
  /** Makes rows and data columns visible, fetching only what is not already held. */
  readonly show: (visible: Viewport) => void;
  /** Expands or collapses the group in the frame's row `local`, if it is one. */
  readonly toggle: (frame: FrameData, local: number) => void;
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
export function groupPath(frame: FrameData, local: number): readonly PathValue[] {
  const depth = frame.tree?.depth(local) ?? 0;
  return frame.groups.slice(0, depth).map((key) => pathValue(key.value(local)));
}

/** Opens a view for a grid component and follows its inputs. Call in an injection context. */
export function gridSource(dataset: InputSignal<string>, layout: InputSignal<string>): GridSource {
  const client = inject(ParallaxClient);
  const frame = signal<FrameData | null>(null);
  const error = signal<string | null>(null);
  let current: { readonly dataset: string; readonly view: ViewHandle } | null = null;
  let generation = 0;
  let destroyed = false;
  let visible: Viewport | null = null;

  const fetch = (view: ViewHandle, wanted: Viewport): void => {
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

  const open = (name: string): void => {
    current?.view.close();
    current = null;
    generation += 1;
    const mine = generation;
    client.connection().then(
      (connection) => {
        // The inputs changed or the grid went away while connecting.
        if (destroyed || mine !== generation) {
          return;
        }
        const view = connection.open(name, untracked(layout), {
          frame: (f) => {
            error.set(null);
            frame.set(f);
          },
          error: (message) => {
            error.set(message);
          }
        });
        current = { dataset: name, view };
        if (visible !== null) {
          fetch(view, visible);
        }
      },
      (failure: unknown) => {
        error.set(failure instanceof Error ? failure.message : String(failure));
      }
    );
  };

  effect(() => {
    const name = dataset();
    const json = layout();
    untracked(() => {
      if (current !== null && current.dataset === name && !current.view.isClosed) {
        current.view.update(json);
      } else {
        open(name);
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
    show: (wanted) => {
      visible = wanted;
      if (current !== null) {
        fetch(current.view, wanted);
      }
    },
    toggle: (shown, local) => {
      const expanded = shown.tree?.expanded(local) ?? null;
      if (expanded !== null) {
        current?.view.setExpanded(groupPath(shown, local), !expanded);
      }
    }
  };
}

/** Maps a scroll offset to the first visible row, scaling when there are too many rows to scroll natively. */
export interface ScrollGeometry {
  readonly spacerPx: number;
  readonly firstRow: (scrollTop: number, viewportPx: number) => number;
  /** Where row `firstRow` starts relative to the viewport's top edge, in pixels (zero or negative). */
  readonly offset: (scrollTop: number, viewportPx: number) => number;
}

/** Computes scroll geometry for `rows` rows of `rowPx` pixels. */
export function scrollGeometry(rows: number, rowPx: number): ScrollGeometry {
  const natural = rows * rowPx;
  if (natural <= MAX_SCROLL_PX) {
    return { spacerPx: natural, firstRow: (top) => Math.floor(top / rowPx), offset: (top) => -(top % rowPx) };
  }
  return {
    spacerPx: MAX_SCROLL_PX,
    firstRow: (top, viewport) => {
      const fraction = top / Math.max(1, MAX_SCROLL_PX - viewport);
      return Math.min(rows - 1, Math.floor(fraction * Math.max(0, rows - viewport / rowPx)));
    },
    offset: () => 0
  };
}

/** The visible rows and data columns of a scrolled grid. */
export function visibleWindow(geometry: ScrollGeometry, element: HTMLElement, rowPx: number, treePx: number): Viewport {
  const dataLeft = Math.max(0, element.scrollLeft);
  return {
    firstRow: geometry.firstRow(element.scrollTop, element.clientHeight),
    rowCount: Math.ceil(element.clientHeight / rowPx) + 1,
    firstColumn: Math.floor(dataLeft / CELL_WIDTH),
    columnCount: Math.ceil(Math.max(0, element.clientWidth - treePx) / CELL_WIDTH) + 1
  };
}
