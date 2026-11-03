/**
 * Where columns sit. Data columns have their own widths, chosen by the user and
 * kept in the layout, so positions are prefix sums. A grouped view repeats its
 * measures once per column-pivot value and then for the row total, so its widths
 * repeat with a period of one set of measures, and a column far to the right is
 * placed without knowing the columns before it.
 */

import { TREE_WIDTH_KEY, measureWidthKey } from "../layout";

/** Width of a data column the user has not resized, in CSS pixels. */
export const DEFAULT_WIDTH = 140;
/** Width of the group column the user has not resized. */
export const DEFAULT_TREE_WIDTH = 260;
/** The narrowest a column can be dragged. */
export const MIN_WIDTH = 40;

/** Positions of a view's columns in content coordinates, which start at the left edge of the group column. */
export interface ColumnGeometry {
  /** Data columns in the whole view. */
  readonly count: number;
  /** Width of the pinned group column, or 0 for a flat view. */
  readonly treePx: number;
  /** Total width of the content. */
  readonly contentWidth: number;
  /** The left edge of data column `column`. */
  readonly x: (column: number) => number;
  /** The width of data column `column`. */
  readonly width: (column: number) => number;
  /** The data column at content offset `x`, clamped to the view's columns. */
  readonly indexAt: (x: number) => number;
  /** The `column_widths` key that data column `column` reads its width from. */
  readonly widthKey: (column: number) => string;
}

type Widths = Readonly<Record<string, number>>;

/** A stored width, kept at or above the minimum so that every column can be found and grabbed. */
function widthOr(widths: Widths, key: string, fallback: number): number {
  return Math.max(MIN_WIDTH, widths[key] ?? fallback);
}

function prefixSums(widths: readonly number[]): readonly number[] {
  const sums = [0];
  for (const w of widths) {
    sums.push((sums.at(-1) ?? 0) + w);
  }
  return sums;
}

/** The last index whose prefix sum is at most `x`. */
function search(sums: readonly number[], x: number): number {
  let lo = 0;
  let hi = sums.length - 2;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if ((sums[mid] ?? 0) <= x) {
      lo = mid;
    } else {
      hi = mid - 1;
    }
  }
  return Math.max(0, lo);
}

/** Geometry of a flat view whose columns are `names`, in order. Columns past the names known so far get the default width. */
export function flatGeometry(count: number, names: readonly string[], widths: Widths): ColumnGeometry {
  const key = (column: number): string => names[column] ?? `#${String(column)}`;
  const sums = prefixSums(Array.from({ length: count }, (_, j) => widthOr(widths, key(j), DEFAULT_WIDTH)));
  const total = sums.at(-1) ?? 0;
  return {
    count,
    treePx: 0,
    contentWidth: total,
    x: (column) => sums[column] ?? total,
    width: (column) => (sums[column + 1] ?? total) - (sums[column] ?? total),
    indexAt: (x) => (count === 0 ? 0 : search(sums, x)),
    widthKey: key
  };
}

/** Geometry of a grouped view with `count` data columns built from `measures` measures. */
export function groupedGeometry(count: number, measures: number, widths: Widths): ColumnGeometry {
  const period = Math.max(1, measures);
  const treePx = widthOr(widths, TREE_WIDTH_KEY, DEFAULT_TREE_WIDTH);
  const sums = prefixSums(Array.from({ length: period }, (_, m) => widthOr(widths, measureWidthKey(m), DEFAULT_WIDTH)));
  const span = sums.at(-1) ?? DEFAULT_WIDTH;
  const x = (column: number): number => treePx + Math.floor(column / period) * span + (sums[column % period] ?? 0);
  return {
    count,
    treePx,
    contentWidth: x(count),
    x,
    width: (column) => (sums[(column % period) + 1] ?? 0) - (sums[column % period] ?? 0),
    indexAt: (at) => {
      if (count === 0) {
        return 0;
      }
      const offset = Math.max(0, at - treePx);
      const set = Math.floor(offset / span);
      const column = set * period + search(sums, offset - set * span);
      return Math.min(count - 1, column);
    },
    widthKey: (column) => measureWidthKey(column % period)
  };
}

/** Browsers cap element heights near 33 million pixels; stay well below. */
const MAX_SCROLL_PX = 15_000_000;

/** Maps a scroll offset to rows, scaling when there are too many rows to scroll natively. */
export interface RowGeometry {
  readonly spacerPx: number;
  /** The first row visible at `scrollTop`. */
  readonly firstRow: (scrollTop: number, viewportPx: number) => number;
  /** Where row `firstRow` starts relative to the viewport's top edge, in pixels (zero or negative). */
  readonly offset: (scrollTop: number, viewportPx: number) => number;
  /** The scroll offset that shows `row` first. */
  readonly scrollTopFor: (row: number, viewportPx: number) => number;
}

/** Computes scroll geometry for `rows` rows of `rowPx` pixels. */
export function rowGeometry(rows: number, rowPx: number): RowGeometry {
  const natural = rows * rowPx;
  if (natural <= MAX_SCROLL_PX) {
    return { spacerPx: natural, firstRow: (top) => Math.floor(top / rowPx), offset: (top) => -(top % rowPx), scrollTopFor: (row) => row * rowPx };
  }
  const span = (viewport: number): number => Math.max(0, rows - viewport / rowPx);
  return {
    spacerPx: MAX_SCROLL_PX,
    firstRow: (top, viewport) => {
      const fraction = top / Math.max(1, MAX_SCROLL_PX - viewport);
      return Math.min(rows - 1, Math.floor(fraction * span(viewport)));
    },
    offset: () => 0,
    scrollTopFor: (row, viewport) => Math.ceil((row / Math.max(1, span(viewport))) * Math.max(1, MAX_SCROLL_PX - viewport))
  };
}
