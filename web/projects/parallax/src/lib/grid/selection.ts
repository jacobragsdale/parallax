/**
 * Cell selection: an anchor where it started and a focus that moves. Column -1 is
 * the group column of a grouped view. Rows and columns are positions in the whole
 * view, not in the frame on screen, so a selection survives scrolling.
 */

/** A cell by view row and data column; column -1 is the group column. */
export interface CellRef {
  readonly row: number;
  readonly column: number;
}

/** A rectangular selection. */
export interface Selection {
  readonly anchor: CellRef;
  readonly focus: CellRef;
}

/** The inclusive bounds of a selection. */
export interface CellRange {
  readonly top: number;
  readonly bottom: number;
  readonly left: number;
  readonly right: number;
}

/** The view's extent: rows, data columns, and whether a group column exists. */
export interface Extent {
  readonly rows: number;
  readonly columns: number;
  readonly tree: boolean;
}

/** Returns the bounds of a selection. */
export function rangeOf(selection: Selection): CellRange {
  const { anchor, focus } = selection;
  return { top: Math.min(anchor.row, focus.row), bottom: Math.max(anchor.row, focus.row), left: Math.min(anchor.column, focus.column), right: Math.max(anchor.column, focus.column) };
}

/** Returns `true` if the cell is inside the range. */
export function contains(range: CellRange, row: number, column: number): boolean {
  return row >= range.top && row <= range.bottom && column >= range.left && column <= range.right;
}

/** Keeps a cell inside the view. */
export function clampCell(cell: CellRef, extent: Extent): CellRef {
  const minColumn = extent.tree ? -1 : 0;
  return { row: Math.min(Math.max(0, cell.row), Math.max(0, extent.rows - 1)), column: Math.min(Math.max(minColumn, cell.column), Math.max(minColumn, extent.columns - 1)) };
}

/** Moves the focus by whole rows and columns; `extend` keeps the anchor, as Shift does. */
export function moveFocus(selection: Selection, rows: number, columns: number, extend: boolean, extent: Extent): Selection {
  const focus = clampCell({ row: selection.focus.row + rows, column: selection.focus.column + columns }, extent);
  return { anchor: extend ? selection.anchor : focus, focus };
}

/** Selects every cell of the view. */
export function selectAll(extent: Extent): Selection {
  return { anchor: { row: 0, column: extent.tree ? -1 : 0 }, focus: clampCell({ row: extent.rows - 1, column: extent.columns - 1 }, extent) };
}

/** Quotes a cell for tab-separated text as Excel does, when it holds a tab, a line break or a quote. */
function quote(cell: string): string {
  return /[\t\n\r"]/.test(cell) ? `"${cell.replaceAll('"', '""')}"` : cell;
}

/** Joins cells into tab-separated text that pastes into Excel as cells. */
export function toTsv(rows: readonly (readonly string[])[]): string {
  return rows.map((row) => row.map(quote).join("\t")).join("\r\n");
}
