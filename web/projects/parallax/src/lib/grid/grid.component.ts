/**
 * `<px-grid>`: a data grid and pivot table drawn on a Canvas 2D surface, for
 * desktops without a GPU.
 *
 * The grid takes one layout and reports every change to it through
 * `layoutChange`, so `[(layout)]` keeps a host's copy current: sorting, column
 * widths, and expanded or collapsed groups all live in the layout. Only the
 * visible rows and columns are drawn, and when a frame arrives for rows already on
 * screen only the cells whose text changed are redrawn, which keeps both the CPU
 * and a remote-display protocol such as Citrix's as idle as possible.
 *
 * The text in each cell is the engine's, made by the formatter the Excel export
 * uses. Numbers too wide for their cell show `###`, as Excel does, rather than
 * losing digits; the full text shows on hover. Copying puts tab-separated text
 * with every digit on the clipboard, which pastes into Excel as cells.
 */

import {
  ChangeDetectionStrategy,
  Component,
  DOCUMENT,
  DestroyRef,
  ViewEncapsulation,
  afterNextRender,
  afterRenderEffect,
  computed,
  effect,
  inject,
  input,
  model,
  output,
  signal,
  viewChild
} from "@angular/core";
import type { ElementRef } from "@angular/core";
import { z } from "zod";

import type { DistinctValues, Viewport } from "../connection";
import { isNegativeText } from "../format";
import { CellFlag } from "../frame";
import type { FrameData, TreeRows } from "../frame";
import { TREE_WIDTH_KEY, layoutSchema } from "../layout";
import type { Layout, LayoutInput, LayoutScalar } from "../layout";
import { ParallaxClient } from "../client";
import { installThemeStyles, readTheme } from "../theme";
import type { Theme, ThemeName } from "../theme";
import { DEFAULT_TREE_WIDTH, DEFAULT_WIDTH, MIN_WIDTH, flatGeometry, groupedGeometry, rowGeometry } from "./geometry";
import type { ColumnGeometry } from "./geometry";
import { groupPath, gridSource, withExpanded } from "./grid-source";
import { contains, moveFocus, rangeOf, selectAll, toTsv } from "./selection";
import type { CellRange, CellRef, Extent, Selection } from "./selection";
import { StatusBarComponent } from "./status-bar.component";

/** Counters for measuring the grid. */
export interface DrawStats {
  readonly frames: number;
  readonly fullDraws: number;
  readonly cellDraws: number;
  readonly drawMs: number;
}

/** A request for the rows behind one group. */
export interface DrillRequest {
  /** The group's values from the outermost level; empty for the grand total. */
  readonly path: readonly LayoutScalar[];
  /** The pivot values of the cell's column, or null for a row total or a flat column. */
  readonly split: readonly LayoutScalar[] | null;
  /** The group's label as shown. */
  readonly label: string;
}

/** The message a refused export posts from its hidden frame. */
const exportRefusal = z.strictObject({ source: z.literal("parallax-export"), token: z.string(), error: z.string() });

/** Row heights by density. */
const ROW_HEIGHTS = { compact: 24, comfortable: 30 } as const;

/** How tightly rows are packed. */
export type Density = keyof typeof ROW_HEIGHTS;

const PADDING = 6;
const INDENT = 14;
/** The width of the expand marker, which a click toggles. */
const MARKER_WIDTH = 14;
/** How close to a header edge, in pixels, the pointer starts a resize. */
const EDGE = 4;
const TOOLTIP_DELAY_MS = 350;

const FLAG_NOTES: Readonly<Record<CellFlag, string | null>> = {
  [CellFlag.Ok]: null,
  [CellFlag.Overflow]: "The exact result needs more than 38 digits, so it is not shown.",
  [CellFlag.Mixed]: "This group combines values in different units, so they are not added.",
  [CellFlag.Undefined]: "Undefined, such as a share of a zero total."
};

type Parsed = { readonly ok: true; readonly layout: Layout } | { readonly ok: false; readonly error: string };

/** Where the pointer is. */
interface Hit {
  readonly header: boolean;
  /** The view row, for the body. */
  readonly row: number | null;
  /** -1 for the group column, a data column, or null past the last column. */
  readonly column: number | null;
  /** The column whose right edge is under the pointer in the header, -1 for the group column. */
  readonly edge: number | null;
}

/** The scrolled area a pass draws. */
interface Area {
  readonly first: number;
  readonly scrollLeft: number;
  readonly width: number;
  readonly height: number;
}

/** One pass of drawing: the context, tokens and geometry it uses. */
interface Pass {
  readonly context: CanvasRenderingContext2D;
  readonly theme: Theme;
  readonly rowPx: number;
  readonly full: boolean;
  readonly geometry: ColumnGeometry;
  readonly scrollLeft: number;
  readonly width: number;
  readonly range: CellRange | null;
  readonly focus: CellRef | null;
  /** Rows to redraw even if their text has not changed, because the selection moved over them. */
  readonly stale: ReadonlySet<number>;
}

function isGrouped(layout: Layout): boolean {
  return layout.group_by.length > 0 || layout.split_by.length > 0 || layout.measures.length > 0;
}

function parse(input: LayoutInput): Parsed {
  const result = layoutSchema.safeParse(input);
  return result.success ? { ok: true, layout: result.data } : { ok: false, error: `Invalid layout: ${z.prettifyError(result.error)}` };
}

const GRID_STYLES = `
  px-grid {
    display: flex;
    flex-direction: column;
    position: relative;
    height: 100%;
    min-height: 0;
    overflow: hidden;
    font: var(--px-font);
    color: var(--px-grid-fg);
    background: var(--px-grid-bg);
  }
  px-grid .px-body {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  px-grid .px-scroll {
    position: absolute;
    inset: 0;
    overflow: auto;
    outline: none;
  }
  px-grid .px-scroll:focus-visible {
    box-shadow: inset 0 0 0 2px var(--px-selection-border);
  }
  px-grid .px-spacer {
    position: absolute;
    top: 0;
    left: 0;
  }
  px-grid .px-canvas {
    position: sticky;
    top: 0;
    left: 0;
    display: block;
  }
  px-grid .px-error {
    position: absolute;
    inset: auto 0 0 0;
    padding: 6px 10px;
    background: var(--px-danger);
    color: #fff;
  }
  px-grid .px-tooltip {
    position: absolute;
    z-index: 2;
    max-width: 480px;
    padding: 4px 8px;
    border: 1px solid var(--px-grid-line);
    border-radius: 3px;
    background: var(--px-header-bg);
    color: var(--px-header-fg);
    white-space: pre-wrap;
    word-break: break-word;
    pointer-events: none;
    box-shadow: 0 2px 6px rgba(0, 0, 0, 0.2);
  }
  px-status-bar {
    position: relative;
    display: flex;
    align-items: center;
    gap: 12px;
    min-height: 24px;
    padding: 0 8px;
    border-top: 1px solid var(--px-grid-line);
    background: var(--px-status-bg);
    color: var(--px-status-fg);
    font-size: 12px;
    white-space: nowrap;
  }
  px-status-bar .px-status-message {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  px-status-bar .px-badge {
    padding: 1px 6px;
    border-radius: 8px;
    color: #fff;
  }
  px-status-bar .px-badge-warning {
    background: var(--px-warning);
  }
  px-status-bar .px-badge-danger {
    background: var(--px-danger);
  }
  px-status-bar .px-status-info {
    width: 18px;
    height: 18px;
    padding: 0;
    border: 1px solid currentColor;
    border-radius: 50%;
    background: transparent;
    color: inherit;
    font: italic 600 11px serif;
    cursor: pointer;
  }
  px-status-bar .px-info {
    position: absolute;
    right: 8px;
    bottom: 28px;
    z-index: 3;
    display: grid;
    grid-template-columns: auto auto;
    gap: 4px 12px;
    margin: 0;
    padding: 10px 12px;
    border: 1px solid var(--px-grid-line);
    border-radius: 4px;
    background: var(--px-grid-bg);
    color: var(--px-grid-fg);
    white-space: normal;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.25);
  }
  px-status-bar .px-info dt {
    color: var(--px-muted-fg);
  }
  px-status-bar .px-info dd {
    margin: 0;
  }
`;

@Component({
  selector: "px-grid",
  changeDetection: ChangeDetectionStrategy.OnPush,
  encapsulation: ViewEncapsulation.None,
  imports: [StatusBarComponent],
  host: { "[attr.data-px-theme]": "theme()" },
  template: `
    <div class="px-body">
      <div
        class="px-scroll"
        #scroll
        tabindex="0"
        role="grid"
        aria-multiselectable="true"
        [attr.aria-rowcount]="totalRows()"
        [attr.aria-colcount]="geometry().count"
        (scroll)="onScroll()"
        (keydown)="onKey($event)"
        (copy)="onCopy($event)"
      >
        <div class="px-spacer" [style.height.px]="spacerHeight()" [style.width.px]="geometry().contentWidth"></div>
        <canvas
          class="px-canvas"
          #canvas
          [style.cursor]="cursor()"
          (pointerdown)="onPointerDown($event)"
          (pointermove)="onPointerMove($event)"
          (pointerup)="onPointerUp($event)"
          (pointerleave)="onPointerLeave()"
          (pointercancel)="endPointer()"
          (lostpointercapture)="endPointer()"
          (dblclick)="onDoubleClick($event)"
        ></canvas>
      </div>
      @if (tooltip(); as tip) {
        <div class="px-tooltip" role="tooltip" [style.left.px]="tip.x" [style.top.px]="tip.y">{{ tip.text }}</div>
      }
      @if (error(); as message) {
        <div class="px-error" role="alert">{{ message }}</div>
      }
    </div>
    <px-status-bar
      [dataset]="current()?.dataset ?? ''"
      [parameters]="current()?.parameters ?? {}"
      [viewRows]="source.frame()?.totalRows ?? null"
      [sourceAsOf]="source.frame()?.sourceAsOfMicros ?? null"
      [status]="source.status()"
      [message]="message()"
      (exported)="export($event)"
    />
  `,
  styles: GRID_STYLES
})
export class GridComponent {
  /** The view: dataset, grouping, sorting, formats and widths. Bind it two ways with `[(layout)]`. */
  public readonly layout = model.required<LayoutInput>();
  /** The theme's tokens; a host can override any of them on the element. */
  public readonly theme = input<ThemeName>("light");
  /** How tightly rows are packed. */
  public readonly density = input<Density>("compact");
  /** Fires when the user double-clicks a value in a grouped view, asking for the rows behind its group. */
  public readonly drill = output<DrillRequest>();

  private readonly parsed = computed(() => parse(this.layout()));
  protected readonly current = computed(() => {
    const parsed = this.parsed();
    return parsed.ok ? parsed.layout : null;
  });
  protected readonly source = gridSource(this.current);
  /** The dataset's columns, once the view has opened. */
  public readonly fields = this.source.fields;
  protected readonly error = computed(() => {
    const parsed = this.parsed();
    return parsed.ok ? this.source.error() : parsed.error;
  });

  private readonly scroll = viewChild.required<ElementRef<HTMLDivElement>>("scroll");
  private readonly canvas = viewChild.required<ElementRef<HTMLCanvasElement>>("canvas");
  private readonly scrollTop = signal(0);
  private readonly scrollLeft = signal(0);
  private readonly viewportPx = signal(600);
  private readonly widthPx = signal(800);
  private readonly selection = signal<Selection | null>(null);
  /** A width being dragged, before it is committed to the layout. */
  private readonly liveWidth = signal<{ readonly key: string; readonly width: number } | null>(null);
  protected readonly cursor = signal("default");
  protected readonly tooltip = signal<{ readonly x: number; readonly y: number; readonly text: string } | null>(null);
  protected readonly message = signal("");

  protected readonly rowHeight = computed(() => ROW_HEIGHTS[this.density()]);
  protected readonly totalRows = computed(() => this.source.frame()?.totalRows ?? 0);
  private readonly rows = computed(() => rowGeometry(this.totalRows() + 1, this.rowHeight()));
  protected readonly spacerHeight = computed(() => this.rows().spacerPx);
  protected readonly geometry = computed((): ColumnGeometry => {
    const layout = this.current();
    const count = this.source.frame()?.totalColumns ?? 0;
    const live = this.liveWidth();
    const widths = { ...(layout?.column_widths ?? {}), ...(live === null ? {} : { [live.key]: live.width }) };
    if (layout !== null && isGrouped(layout)) {
      return groupedGeometry(count, layout.measures.length, widths);
    }
    const names = layout !== null && layout.columns.length > 0 ? layout.columns : this.source.fields().map((f) => f.name);
    return flatGeometry(count, names, widths);
  });
  private readonly extent = computed((): Extent => ({ rows: this.totalRows(), columns: this.geometry().count, tree: this.geometry().treePx > 0 }));

  private stats: DrawStats = { frames: 0, fullDraws: 0, cellDraws: 0, drawMs: 0 };
  private drawn = new Map<string, string>();
  private drawnRows = new Set<number>();
  /** Full text of cells drawn cut short, and notes for flagged cells, by cell key. */
  private notes = new Map<string, string>();
  private shape = "";
  private tokens: Theme | null = null;
  private readonly textWidths = new Map<string, number>();
  private pressed: { readonly hit: Hit; readonly x: number; readonly startWidth: number; readonly key: string } | null = null;
  private selecting = false;
  /** The selection as last drawn, to find the rows a new selection changes. */
  private drawnRange: CellRange | null = null;
  private drawnFocus: CellRef | null = null;
  /** Rows fetched for a copy that could not reach the clipboard at once, kept for the next Ctrl+C. */
  private prepared: { readonly key: string; readonly text: string } | null = null;
  private tooltipTimer: ReturnType<typeof setTimeout> | null = null;

  private readonly document = inject(DOCUMENT);
  private readonly client = inject(ParallaxClient);

  constructor() {
    installThemeStyles(this.document);
    const destroyRef = inject(DestroyRef);
    // Measure once rendered and whenever the grid resizes; that also asks for the first rows.
    afterNextRender(() => {
      const observer = new ResizeObserver(() => {
        this.onScroll();
      });
      observer.observe(this.scroll().nativeElement);
      destroyRef.onDestroy(() => {
        observer.disconnect();
        this.clearTooltip();
      });
      this.onScroll();
    });
    // Ask for what is in view whenever scrolling, resizing or a new column count changes it.
    effect(() => {
      this.source.show(this.visible());
    });
    afterRenderEffect(() => {
      // Reading these tracks them; any change redraws.
      this.theme();
      this.geometry();
      this.selection();
      this.draw(this.source.frame(), this.scrollTop(), this.scrollLeft(), this.viewportPx(), this.widthPx());
    });
  }

  /** Drawing counters, for measurement. */
  public drawStats(): DrawStats {
    return this.stats;
  }

  /**
   * Downloads the view as an Excel workbook or a CSV file. The browser posts the
   * layout in a form and streams the answer to disk, so a large export never sits
   * in the page's memory. A refusal, such as more rows than Excel holds, comes
   * back as text, which the status bar shows.
   */
  public export(format: "xlsx" | "csv"): void {
    const layout = this.current();
    if (layout === null) {
      return;
    }
    const token = `${String(Date.now())}-${String(Math.random()).slice(2)}`;
    const target = `px-export-${token}`;
    const frame = this.document.createElement("iframe");
    frame.name = target;
    frame.hidden = true;
    // Downloads never reach the page; a refusal posts its reason here, which works
    // even when the server is on another origin and the frame cannot be read.
    const window = this.document.defaultView;
    const listen = (event: MessageEvent<unknown>): void => {
      const posted = exportRefusal.safeParse(event.data);
      if (posted.success && posted.data.token === token) {
        this.message.set(`Export failed: ${posted.data.error}`);
      }
    };
    window?.addEventListener("message", listen);
    this.document.body.append(frame);
    setTimeout(() => {
      window?.removeEventListener("message", listen);
      frame.remove();
    }, 600_000);
    const form = this.document.createElement("form");
    form.method = "post";
    form.action = this.client.exportUrl();
    form.target = target;
    for (const [name, value] of [
      ["layout", JSON.stringify(layout)],
      ["format", format],
      ["token", token]
    ] as const) {
      const field = this.document.createElement("input");
      field.type = "hidden";
      field.name = name;
      field.value = value;
      form.append(field);
    }
    this.document.body.append(form);
    form.submit();
    form.remove();
    this.message.set(format === "xlsx" ? "Exporting to Excel; your browser will save the file." : "Exporting to CSV; your browser will save the file.");
  }

  /** Lists a column's values under the view's other filters, for filter pick-lists. */
  public distinctValues(column: string, search: string, limit: number): Promise<DistinctValues> {
    return this.source.distinct(column, search, limit);
  }

  // ---- Scrolling ---------------------------------------------------------

  protected onScroll(): void {
    const element = this.scroll().nativeElement;
    this.scrollTop.set(element.scrollTop);
    this.scrollLeft.set(element.scrollLeft);
    this.viewportPx.set(element.clientHeight);
    this.widthPx.set(element.clientWidth);
  }

  /** The rows and data columns in view. Before the first frame, when the column count is unknown, it covers the width at default widths. */
  private visible(): Viewport {
    const geometry = this.geometry();
    const rowPx = this.rowHeight();
    const scrollLeft = this.scrollLeft();
    const width = this.widthPx();
    const firstRow = this.rows().firstRow(this.scrollTop(), this.viewportPx());
    const rowCount = Math.ceil(this.viewportPx() / rowPx) + 1;
    if (geometry.count === 0) {
      return { firstRow, rowCount, firstColumn: 0, columnCount: Math.ceil(width / DEFAULT_WIDTH) + 1 };
    }
    const left = geometry.indexAt(geometry.treePx + scrollLeft);
    const right = geometry.indexAt(scrollLeft + width);
    return { firstRow, rowCount, firstColumn: left, columnCount: right - left + 1 };
  }

  private firstVisibleRow(): number {
    return this.rows().firstRow(this.scrollTop(), this.viewportPx());
  }

  /** Scrolls so that `cell` is in view. */
  private reveal(cell: CellRef): void {
    const element = this.scroll().nativeElement;
    const rowPx = this.rowHeight();
    const visibleRows = Math.max(1, Math.floor(element.clientHeight / rowPx) - 1);
    const first = this.firstVisibleRow();
    if (cell.row < first) {
      element.scrollTop = this.rows().scrollTopFor(cell.row, element.clientHeight);
    } else if (cell.row >= first + visibleRows) {
      element.scrollTop = this.rows().scrollTopFor(cell.row - visibleRows + 1, element.clientHeight);
    }
    if (cell.column >= 0) {
      const geometry = this.geometry();
      const x = geometry.x(cell.column) - element.scrollLeft;
      if (x < geometry.treePx) {
        element.scrollLeft = geometry.x(cell.column) - geometry.treePx;
      } else if (x + geometry.width(cell.column) > element.clientWidth) {
        element.scrollLeft = geometry.x(cell.column) + geometry.width(cell.column) - element.clientWidth;
      }
    }
  }

  // ---- Pointer -----------------------------------------------------------

  private hit(x: number, y: number): Hit {
    const rowPx = this.rowHeight();
    const geometry = this.geometry();
    const header = y < rowPx;
    const row = header ? null : this.firstVisibleRow() + Math.floor((y - rowPx) / rowPx);
    let column: number | null;
    let edge: number | null = null;
    if (geometry.treePx > 0 && x < geometry.treePx) {
      column = -1;
      if (header && geometry.treePx - x <= EDGE) {
        edge = -1;
      }
    } else {
      const content = x + this.scrollLeft();
      column = content < geometry.contentWidth && geometry.count > 0 ? geometry.indexAt(content) : null;
      if (header && column !== null) {
        const right = geometry.x(column) + geometry.width(column) - this.scrollLeft();
        const left = geometry.x(column) - this.scrollLeft();
        if (right - x <= EDGE) {
          edge = column;
        } else if (x - left <= EDGE && column > 0) {
          edge = column - 1;
        }
      }
    }
    return { header, row: row !== null && row < this.totalRows() ? row : null, column, edge };
  }

  private widthKeyOf(column: number): string {
    return column === -1 ? TREE_WIDTH_KEY : this.geometry().widthKey(column);
  }

  private widthOf(column: number): number {
    const geometry = this.geometry();
    return column === -1 ? geometry.treePx : geometry.width(column);
  }

  protected onPointerDown(event: PointerEvent): void {
    if (event.button !== 0) {
      return;
    }
    this.clearTooltip();
    const hit = this.hit(event.offsetX, event.offsetY);
    const canvas = this.canvas().nativeElement;
    if (hit.header) {
      const key = hit.edge === null ? "" : this.widthKeyOf(hit.edge);
      this.pressed = { hit, x: event.clientX, startWidth: hit.edge === null ? 0 : this.widthOf(hit.edge), key };
      if (hit.edge !== null) {
        canvas.setPointerCapture(event.pointerId);
      }
      return;
    }
    if (hit.row === null || hit.column === null) {
      return;
    }
    this.scroll().nativeElement.focus({ preventScroll: true });
    const cell = { row: hit.row, column: hit.column };
    const previous = this.selection();
    this.selection.set(event.shiftKey && previous !== null ? { anchor: previous.anchor, focus: cell } : { anchor: cell, focus: cell });
    this.selecting = true;
    canvas.setPointerCapture(event.pointerId);
    if (hit.column === -1 && !event.shiftKey && this.onMarker(hit.row, event.offsetX)) {
      this.toggleRow(hit.row);
    }
  }

  /** Whether `x` falls on the expand marker of row `row`'s group cell. */
  private onMarker(row: number, x: number): boolean {
    const frame = this.source.frame();
    const tree = frame?.tree ?? null;
    const local = frame === null ? -1 : row - frame.firstRow;
    if (frame === null || tree === null || local < 0 || local >= frame.rowCount || tree.expanded(local) === null) {
      return false;
    }
    const left = PADDING + tree.depth(local) * INDENT;
    return x >= left - 4 && x <= left + MARKER_WIDTH;
  }

  /** Ends a drag that the browser cancelled, as when the window loses focus. */
  protected endPointer(): void {
    this.pressed = null;
    this.selecting = false;
    if (this.liveWidth() !== null) {
      this.liveWidth.set(null);
    }
  }

  protected onPointerMove(event: PointerEvent): void {
    const pressed = this.pressed;
    if (pressed !== null && pressed.hit.edge !== null) {
      this.liveWidth.set({ key: pressed.key, width: Math.max(MIN_WIDTH, Math.round(pressed.startWidth + event.clientX - pressed.x)) });
      return;
    }
    const hit = this.hit(event.offsetX, event.offsetY);
    if (this.selecting && hit.row !== null && hit.column !== null) {
      const selection = this.selection();
      if (selection !== null) {
        this.selection.set({ anchor: selection.anchor, focus: { row: hit.row, column: hit.column } });
      }
      return;
    }
    this.cursor.set(hit.edge !== null ? "col-resize" : hit.header && hit.column !== null ? "pointer" : "default");
    this.scheduleTooltip(hit, event.offsetX, event.offsetY);
  }

  protected onPointerUp(event: PointerEvent): void {
    const pressed = this.pressed;
    this.pressed = null;
    this.selecting = false;
    if (pressed === null) {
      return;
    }
    if (pressed.hit.edge !== null) {
      const live = this.liveWidth();
      this.liveWidth.set(null);
      if (live !== null) {
        this.setWidth(live.key, live.width);
      }
      return;
    }
    const hit = this.hit(event.offsetX, event.offsetY);
    if (hit.header && hit.column !== null && hit.column === pressed.hit.column) {
      this.sortBy(hit.column, event.shiftKey);
    }
  }

  protected onPointerLeave(): void {
    this.clearTooltip();
    if (this.pressed === null) {
      this.cursor.set("default");
    }
  }

  protected onDoubleClick(event: MouseEvent): void {
    const hit = this.hit(event.offsetX, event.offsetY);
    if (hit.header && hit.edge !== null) {
      this.setWidth(this.widthKeyOf(hit.edge), this.fitWidth(hit.edge));
    } else if (!hit.header && hit.column === -1 && hit.row !== null && !this.onMarker(hit.row, event.offsetX)) {
      // The marker toggles on a single click, so only the label toggles here.
      this.toggleRow(hit.row);
    } else if (!hit.header && hit.column !== null && hit.column >= 0 && hit.row !== null) {
      this.requestDrill(hit.row, hit.column);
    }
  }

  /** Asks for the rows behind the cell at view row `row` and data column `column`, if its row is one group rather than an Other row. */
  private requestDrill(row: number, column: number): void {
    const frame = this.source.frame();
    const tree = frame?.tree ?? null;
    const local = frame === null ? -1 : row - frame.firstRow;
    if (frame === null || tree === null || local < 0 || local >= frame.rowCount || tree.kind(local) === "other") {
      return;
    }
    const data = frame.columns[column - frame.firstColumn];
    const split = data?.split ?? null;
    const label = split === null || data === undefined ? tree.label(local) : `${tree.label(local)}, ${data.title.slice(0, data.title.lastIndexOf(" | "))}`;
    this.drill.emit({ path: groupPath(frame, local), split, label });
  }

  // ---- Layout changes ----------------------------------------------------

  private change(update: (layout: Layout) => Layout): void {
    const layout = this.current();
    if (layout !== null) {
      this.layout.set(update(layout));
    }
  }

  private setWidth(key: string, width: number): void {
    this.change((layout) => ({ ...layout, column_widths: { ...layout.column_widths, [key]: Math.max(MIN_WIDTH, Math.round(width)) } }));
  }

  /** The width that shows the header and every loaded value of a column. */
  private fitWidth(column: number): number {
    const frame = this.source.frame();
    const context = this.canvas().nativeElement.getContext("2d");
    if (frame === null || context === null) {
      return this.widthOf(column);
    }
    context.font = (this.tokens ?? readTheme(this.canvas().nativeElement)).font;
    let widest = 0;
    if (column === -1) {
      const tree = frame.tree;
      for (let local = 0; local < frame.rowCount && tree !== null; local += 1) {
        widest = Math.max(widest, tree.depth(local) * INDENT + this.measure(context, `▾ ${tree.label(local)}`));
      }
      return Math.max(DEFAULT_TREE_WIDTH / 2, widest + 2 * PADDING);
    }
    const data = frame.columns[column - frame.firstColumn];
    if (data === undefined) {
      return this.widthOf(column);
    }
    widest = this.measure(context, `${data.title} ▲`);
    for (let local = 0; local < frame.rowCount; local += 1) {
      widest = Math.max(widest, this.measure(context, data.text(local)));
    }
    return widest + 2 * PADDING + 2;
  }

  /** Cycles a column's sort: ascending, descending, then off. Shift adds it to the sorts of a flat view. */
  private sortBy(column: number, additive: boolean): void {
    this.change((layout) => {
      if (isGrouped(layout)) {
        const by = layout.group_sort.by;
        if (column === -1) {
          return { ...layout, group_sort: { by: "key", descending: by === "key" ? !layout.group_sort.descending : false } };
        }
        const measure = column % Math.max(1, layout.measures.length);
        const same = by !== "key" && by.measure === measure;
        if (!same) {
          return { ...layout, group_sort: { by: { measure }, descending: false } };
        }
        return { ...layout, group_sort: layout.group_sort.descending ? { by: "key", descending: false } : { by: { measure }, descending: true } };
      }
      const name = this.geometry().widthKey(column);
      const existing = layout.sort.find((s) => s.column === name);
      const next = existing === undefined ? { column: name, descending: false } : existing.descending ? null : { column: name, descending: true };
      if (!additive) {
        return { ...layout, sort: next === null ? [] : [next] };
      }
      const others = layout.sort.filter((s) => s.column !== name);
      return { ...layout, sort: next === null ? others : existing === undefined ? [...others, next] : layout.sort.map((s) => (s.column === name ? next : s)) };
    });
  }

  private toggleRow(row: number): void {
    const frame = this.source.frame();
    if (frame === null) {
      return;
    }
    const local = row - frame.firstRow;
    if (local < 0 || local >= frame.rowCount) {
      return;
    }
    const entry = this.source.toggle(frame, local);
    if (entry !== null) {
      this.change((layout) => withExpanded(layout, entry));
    }
  }

  // ---- Keyboard and clipboard --------------------------------------------

  /** The selection a navigation key leads to, or null for keys that do not navigate. */
  private navigate(event: KeyboardEvent, selection: Selection, extent: Extent): Selection | null {
    const page = Math.max(1, Math.floor(this.viewportPx() / this.rowHeight()) - 2);
    const jump = event.ctrlKey || event.metaKey;
    const far = Number.MAX_SAFE_INTEGER;
    const steps: Readonly<Record<string, readonly [number, number]>> = {
      ArrowDown: [jump ? far : 1, 0],
      ArrowUp: [jump ? -far : -1, 0],
      ArrowRight: [0, jump ? far : 1],
      ArrowLeft: [0, jump ? -far : -1],
      PageDown: [page, 0],
      PageUp: [-page, 0],
      Home: [jump ? -far : 0, -far],
      End: [jump ? far : 0, far]
    };
    const step = steps[event.key];
    if (step !== undefined) {
      return moveFocus(selection, step[0], step[1], event.shiftKey, extent);
    }
    return event.key === "a" && jump ? selectAll(extent) : null;
  }

  protected onKey(event: KeyboardEvent): void {
    const extent = this.extent();
    if (extent.rows === 0) {
      return;
    }
    const start = { row: 0, column: extent.tree ? -1 : 0 };
    const selection = this.selection() ?? { anchor: start, focus: start };
    if (event.key === "Escape") {
      this.selection.set(null);
      event.preventDefault();
      return;
    }
    if ((event.key === "Enter" || event.key === " ") && selection.focus.column === -1) {
      this.toggleRow(selection.focus.row);
      event.preventDefault();
      return;
    }
    const next = this.navigate(event, selection, extent);
    if (next !== null) {
      event.preventDefault();
      this.selection.set(next);
      this.reveal(next.focus);
    }
  }

  /** The selected cells as text, if the frame holds all of them. */
  private rangeText(frame: FrameData, range: CellRange): string | null {
    const firstData = Math.max(0, range.left);
    const holdsRows = range.top >= frame.firstRow && range.bottom < frame.firstRow + frame.rowCount;
    const holdsColumns = range.right < 0 || (firstData >= frame.firstColumn && range.right < frame.firstColumn + frame.columns.length);
    if (!holdsRows || !holdsColumns || (range.left === -1 && frame.tree === null)) {
      return null;
    }
    const rows: string[][] = [];
    for (let row = range.top; row <= range.bottom; row += 1) {
      const local = row - frame.firstRow;
      const cells: string[] = [];
      for (let column = range.left; column <= range.right; column += 1) {
        cells.push(column === -1 ? (frame.tree?.label(local) ?? "") : (frame.columns[column - frame.firstColumn]?.copy(local) ?? ""));
      }
      rows.push(cells);
    }
    return toTsv(rows);
  }

  /** Identifies a copied range in one configuration and load of the view. */
  private copyKey(frame: FrameData, range: CellRange): string {
    return JSON.stringify([frame.configVersion.toString(), frame.loadVersion.toString(), range]);
  }

  protected onCopy(event: ClipboardEvent): void {
    const selection = this.selection();
    const frame = this.source.frame();
    if (selection === null || frame === null) {
      return;
    }
    event.preventDefault();
    const range = rangeOf(selection);
    const rows = range.bottom - range.top + 1;
    const count = `${rows.toLocaleString("en-US")} ${rows === 1 ? "row" : "rows"}`;
    const key = this.copyKey(frame, range);
    const ready = this.prepared?.key === key ? this.prepared.text : this.rangeText(frame, range);
    if (ready !== null) {
      event.clipboardData?.setData("text/plain", ready);
      this.prepared = null;
      this.message.set(`Copied ${count}`);
      return;
    }
    const firstColumn = Math.max(0, range.left);
    this.message.set(`Copying ${count}…`);
    const fetched = this.source.fetch({ firstRow: range.top, rowCount: rows, firstColumn, columnCount: Math.max(1, range.right - firstColumn + 1) }).then((f) => {
      const text = this.rangeText(f, range);
      if (text === null) {
        throw new Error("the rows changed while copying; copy again");
      }
      return { text, key: this.copyKey(f, range) };
    });
    const keep = (): void => {
      // Browsers only let a page write to the clipboard during the user's action,
      // and the rows arrived after it ended: keep them for the next Ctrl+C.
      fetched.then(
        (copy) => {
          this.prepared = copy;
          this.message.set(`${count} ready: press Ctrl+C again to copy them`);
        },
        (failure: unknown) => {
          this.message.set(`Copy failed: ${failure instanceof Error ? failure.message : String(failure)}`);
        }
      );
    };
    if (!window.isSecureContext || typeof ClipboardItem === "undefined") {
      keep();
      return;
    }
    // Started inside the copy event, the write still counts as the user's action;
    // the text follows when the rows arrive.
    const blob = fetched.then((copy) => new Blob([copy.text], { type: "text/plain" }));
    navigator.clipboard.write([new ClipboardItem({ "text/plain": blob })]).then(
      () => {
        this.message.set(`Copied ${count}`);
      },
      () => {
        keep();
      }
    );
  }

  // ---- Tooltips ----------------------------------------------------------

  private scheduleTooltip(hit: Hit, x: number, y: number): void {
    this.clearTooltip();
    if (hit.header || hit.row === null || hit.column === null) {
      return;
    }
    const text = this.notes.get(`${String(hit.row)}:${String(hit.column)}`);
    if (text === undefined) {
      return;
    }
    this.tooltipTimer = setTimeout(() => {
      this.tooltip.set({ x: x + 12, y: y + 16, text });
    }, TOOLTIP_DELAY_MS);
  }

  private clearTooltip(): void {
    if (this.tooltipTimer !== null) {
      clearTimeout(this.tooltipTimer);
      this.tooltipTimer = null;
    }
    if (this.tooltip() !== null) {
      this.tooltip.set(null);
    }
  }

  // ---- Drawing -----------------------------------------------------------

  private measure(context: CanvasRenderingContext2D, text: string): number {
    let width = this.textWidths.get(text);
    if (width === undefined) {
      width = context.measureText(text).width;
      if (this.textWidths.size > 50_000) {
        this.textWidths.clear();
      }
      this.textWidths.set(text, width);
    }
    return width;
  }

  private prepare(width: number, height: number): CanvasRenderingContext2D | null {
    const element = this.canvas().nativeElement;
    const context = element.getContext("2d", { alpha: false });
    if (context === null) {
      return null;
    }
    const ratio = window.devicePixelRatio;
    if (element.width !== Math.round(width * ratio) || element.height !== Math.round(height * ratio)) {
      element.width = Math.round(width * ratio);
      element.height = Math.round(height * ratio);
      element.style.width = `${String(width)}px`;
      element.style.height = `${String(height)}px`;
      this.shape = "";
    }
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    return context;
  }

  private draw(frame: FrameData | null, scrollTop: number, scrollLeft: number, viewportPx: number, widthPx: number): void {
    const started = performance.now();
    const height = Math.max(1, viewportPx);
    const width = Math.max(1, widthPx);
    const context = this.prepare(width, height);
    if (context === null) {
      return;
    }
    const first = this.rows().firstRow(scrollTop, height);
    const pass = this.begin(context, frame, { first, scrollLeft, width, height });
    if (frame !== null) {
      const cells = this.drawRows(pass, frame, first, height);
      this.stats = { frames: this.stats.frames + 1, fullDraws: this.stats.fullDraws, cellDraws: this.stats.cellDraws + cells, drawMs: this.stats.drawMs + performance.now() - started };
    }
  }

  /** Decides whether this pass redraws everything, and if so clears and draws the header. */
  /** Everything a full redraw depends on, as one comparable string. */
  private shapeOf(frame: FrameData | null, at: Area): string {
    const geometry = this.geometry();
    const layout = this.current();
    const versions = frame === null ? null : [frame.configVersion.toString(), frame.loadVersion.toString(), frame.totalRows, frame.firstColumn];
    const decor = layout === null ? null : [layout.column_widths, layout.sort, layout.group_sort];
    return JSON.stringify([at, versions, this.theme(), geometry.treePx, geometry.contentWidth, decor]);
  }

  /** Visible rows the selection covered or now covers, which must be redrawn even if their text is unchanged. */
  private selectionRows(range: CellRange | null, focus: CellRef | null, at: Area): ReadonlySet<number> {
    const rows = new Set<number>();
    const visible = Math.ceil(at.height / this.rowHeight());
    for (let row = at.first; row < at.first + visible; row += 1) {
      const covered = (r: CellRange | null): boolean => r !== null && row >= r.top && row <= r.bottom;
      if (covered(range) || covered(this.drawnRange) || row === focus?.row || row === this.drawnFocus?.row) {
        rows.add(row);
      }
    }
    return rows;
  }

  private begin(target: CanvasRenderingContext2D, frame: FrameData | null, at: Area): Pass {
    const geometry = this.geometry();
    const selection = this.selection();
    const shape = this.shapeOf(frame, at);
    const range = selection === null ? null : rangeOf(selection);
    const focus = selection?.focus ?? null;
    const moved = JSON.stringify([range, focus]) !== JSON.stringify([this.drawnRange, this.drawnFocus]);
    const stale = moved ? this.selectionRows(range, focus, at) : new Set<number>();
    this.drawnRange = range;
    this.drawnFocus = focus;
    const full = this.tokens === null || shape !== this.shape;
    const theme = full || this.tokens === null ? readTheme(this.canvas().nativeElement) : this.tokens;
    this.tokens = theme;
    this.shape = shape;
    const pass: Pass = { context: target, theme, rowPx: this.rowHeight(), full, geometry, scrollLeft: at.scrollLeft, width: at.width, range, focus, stale };
    const { context } = pass;
    context.font = theme.font;
    context.textBaseline = "middle";
    if (full) {
      this.drawn = new Map();
      this.drawnRows = new Set();
      this.notes = new Map();
      context.fillStyle = theme.background;
      context.fillRect(0, 0, at.width, at.height);
      this.drawHeader(pass, frame);
      this.stats = { ...this.stats, fullDraws: this.stats.fullDraws + 1 };
    }
    return pass;
  }

  private drawRows(pass: Pass, frame: FrameData, first: number, height: number): number {
    let cells = 0;
    const visible = Math.ceil(height / pass.rowPx);
    for (let i = 0; i < visible; i += 1) {
      const row = first + i;
      const local = row - frame.firstRow;
      const y = pass.rowPx * (i + 1);
      if (local >= 0 && local < frame.rowCount) {
        cells += this.drawRow(pass, frame, local, row, y);
        this.drawnRows.add(row);
      } else if (this.drawnRows.delete(row)) {
        // The row no longer has data, such as after a group collapsed: clear it.
        const { context, theme } = pass;
        context.fillStyle = theme.background;
        context.fillRect(0, y, pass.width, pass.rowPx);
      }
    }
    return cells;
  }

  /** Data columns that intersect the visible part of the data area. */
  private visibleColumns(pass: Pass): readonly number[] {
    const { geometry } = pass;
    if (geometry.count === 0) {
      return [];
    }
    const left = geometry.indexAt(geometry.treePx + pass.scrollLeft);
    const right = geometry.indexAt(pass.scrollLeft + pass.width);
    return Array.from({ length: right - left + 1 }, (_, i) => left + i);
  }

  /** Runs `paint` clipped to the data area, which scrolls under the pinned group column. */
  private inDataArea(pass: Pass, top: number, height: number, paint: () => void): void {
    const { context, geometry } = pass;
    context.save();
    context.beginPath();
    context.rect(geometry.treePx, top, Math.max(0, pass.width - geometry.treePx), height);
    context.clip();
    paint();
    context.restore();
  }

  private sortMark(column: number): string {
    const layout = this.current();
    if (layout === null) {
      return "";
    }
    if (isGrouped(layout)) {
      const by = layout.group_sort.by;
      const arrow = layout.group_sort.descending ? "▼" : "▲";
      if (column === -1) {
        return by === "key" && layout.group_sort.descending ? "▼" : "";
      }
      return by !== "key" && column % Math.max(1, layout.measures.length) === by.measure ? arrow : "";
    }
    const name = this.geometry().widthKey(column);
    const index = layout.sort.findIndex((s) => s.column === name);
    const sort = layout.sort[index];
    if (sort === undefined) {
      return "";
    }
    const arrow = sort.descending ? "▼" : "▲";
    return layout.sort.length > 1 ? `${arrow}${String(index + 1)}` : arrow;
  }

  private drawHeader(pass: Pass, frame: FrameData | null): void {
    const { context, theme, rowPx, geometry } = pass;
    context.fillStyle = theme.headerBackground;
    context.fillRect(0, 0, pass.width, rowPx);
    const layout = this.current();
    if (geometry.treePx > 0) {
      context.fillStyle = theme.headerForeground;
      const title = layout === null || layout.group_by.length === 0 ? "Group" : layout.group_by.join(" › ");
      this.headerText(pass, title, this.sortMark(-1), 0, geometry.treePx);
      context.fillStyle = theme.gridLine;
      context.fillRect(geometry.treePx - 1, 0, 1, rowPx);
    }
    this.inDataArea(pass, 0, rowPx, () => {
      for (const column of this.visibleColumns(pass)) {
        const x = geometry.x(column) - pass.scrollLeft;
        const width = geometry.width(column);
        const data = frame === null ? undefined : frame.columns[column - frame.firstColumn];
        context.fillStyle = theme.headerForeground;
        this.headerText(pass, data?.title ?? "", this.sortMark(column), x, width);
        context.fillStyle = theme.gridLine;
        context.fillRect(x + width - 1, 0, 1, rowPx);
      }
    });
    context.fillStyle = theme.gridLine;
    context.fillRect(0, rowPx - 1, pass.width, 1);
  }

  private headerText(pass: Pass, title: string, mark: string, x: number, width: number): void {
    const { context, rowPx } = pass;
    const markWidth = mark === "" ? 0 : this.measure(context, mark) + 4;
    this.clipText(context, title, x, width - markWidth, rowPx / 2, false);
    if (mark !== "") {
      context.fillText(mark, x + width - PADDING - markWidth + 4, rowPx / 2);
    }
  }

  private drawRow(pass: Pass, frame: FrameData, local: number, row: number, y: number): number {
    const stripe = row % 2 === 0 ? pass.theme.background : pass.theme.stripe;
    let cells = frame.tree === null ? 0 : this.drawTreeCell(pass, frame.tree, local, row, y, stripe);
    this.inDataArea(pass, y, pass.rowPx, () => {
      for (const column of this.visibleColumns(pass)) {
        if (this.drawCell(pass, frame, column, local, row, y, stripe)) {
          cells += 1;
        }
      }
    });
    return cells;
  }

  /** Fills a cell's background, marking it if it is selected. */
  private cellBackground(pass: Pass, row: number, column: number, x: number, y: number, width: number, stripe: string): void {
    const { context, theme, rowPx, range, focus } = pass;
    context.fillStyle = stripe;
    context.fillRect(x, y, width, rowPx);
    if (range !== null && contains(range, row, column)) {
      context.fillStyle = theme.selection;
      context.fillRect(x, y, width, rowPx);
    }
    if (focus !== null && focus.row === row && focus.column === column) {
      context.strokeStyle = theme.selectionBorder;
      context.lineWidth = 2;
      context.strokeRect(x + 1, y + 1, width - 2, rowPx - 2);
    }
  }

  private drawTreeCell(pass: Pass, tree: TreeRows, local: number, row: number, y: number, stripe: string): number {
    const depth = tree.depth(local);
    const expanded = tree.expanded(local);
    const marker = expanded === null ? "  " : expanded ? "▾ " : "▸ ";
    const text = `${marker}${tree.label(local)}`;
    const key = `${String(row)}:-1`;
    if (!pass.full && !pass.stale.has(row) && this.drawn.get(key) === text) {
      return 0;
    }
    const { context, theme, rowPx, geometry } = pass;
    this.cellBackground(pass, row, -1, 0, y, geometry.treePx, stripe);
    context.fillStyle = tree.kind(local) === "total" ? theme.totalForeground : theme.foreground;
    const left = PADDING + depth * INDENT;
    if (this.clipText(context, text, left - PADDING, geometry.treePx - left + PADDING, y + rowPx / 2, false)) {
      this.notes.set(key, tree.label(local));
    }
    context.fillStyle = theme.gridLine;
    context.fillRect(geometry.treePx - 1, y, 1, rowPx);
    this.drawn.set(key, text);
    return 1;
  }

  private drawCell(pass: Pass, frame: FrameData, column: number, local: number, row: number, y: number, stripe: string): boolean {
    const data = frame.columns[column - frame.firstColumn];
    if (data === undefined) {
      return false;
    }
    const text = data.text(local);
    const key = `${String(row)}:${String(column)}`;
    if (!pass.full && !pass.stale.has(row) && this.drawn.get(key) === text) {
      return false;
    }
    const { context, theme, rowPx, geometry } = pass;
    const x = geometry.x(column) - pass.scrollLeft;
    const width = geometry.width(column);
    this.cellBackground(pass, row, column, x, y, width, stripe);
    const flag = data.flag(local);
    context.fillStyle = flag !== CellFlag.Ok ? theme.muted : data.numeric && isNegativeText(text) ? theme.negative : theme.foreground;
    const clipped = this.clipText(context, text, x, width, y + rowPx / 2, data.numeric && flag === CellFlag.Ok);
    const note = FLAG_NOTES[flag];
    if (note !== null) {
      this.notes.set(key, note);
    } else if (clipped) {
      this.notes.set(key, text);
    } else {
      this.notes.delete(key);
    }
    context.fillStyle = theme.gridLine;
    context.fillRect(x + width - 1, y, 1, rowPx);
    this.drawn.set(key, text);
    return true;
  }

  /** Draws text within a cell; returns `true` if it did not fit and was cut or replaced by `###`. */
  private clipText(context: CanvasRenderingContext2D, text: string, x: number, width: number, y: number, numeric: boolean): boolean {
    const room = width - 2 * PADDING;
    const measured = this.measure(context, text);
    if (numeric) {
      const fits = measured <= room;
      const shown = fits ? text : "###";
      const w = fits ? measured : this.measure(context, shown);
      context.fillText(shown, x + width - PADDING - w, y);
      return !fits;
    }
    if (measured <= room) {
      context.fillText(text, x + PADDING, y);
      return false;
    }
    let cut = text.length;
    while (cut > 0 && this.measure(context, `${text.slice(0, cut)}…`) > room) {
      cut -= 1;
    }
    context.fillText(`${text.slice(0, cut)}…`, x + PADDING, y);
    return true;
  }
}
