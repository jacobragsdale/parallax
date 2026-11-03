/**
 * A virtualized DOM grid: the same data plumbing as the canvas grid, drawn with
 * positioned elements. It exists for the measured comparison that decided which
 * grid parallax ships; see docs/grid-experiment.md.
 */

import { ChangeDetectionStrategy, Component, DestroyRef, afterNextRender, computed, inject, input, signal, viewChild } from "@angular/core";
import type { ElementRef } from "@angular/core";

import { isNegativeText } from "../format";
import type { FrameData } from "../frame";
import { CELL_WIDTH, TREE_WIDTH, gridSource, scrollGeometry, visibleWindow } from "./grid-source";

interface Cell {
  readonly x: number;
  readonly text: string;
  readonly numeric: boolean;
  readonly negative: boolean;
}

interface Title {
  readonly x: number;
  readonly text: string;
}

interface Row {
  readonly index: number;
  readonly local: number;
  readonly top: number;
  readonly label: string | null;
  readonly indent: number;
  readonly total: boolean;
  readonly expandable: boolean;
  readonly cells: readonly Cell[];
}

@Component({
  selector: "px-dom-grid",
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <div class="px-scroll" #scroll (scroll)="onScroll()" role="grid" [attr.aria-rowcount]="totalRows()">
      <div class="px-spacer" [style.height.px]="spacerHeight()" [style.width.px]="contentWidth()"></div>
      <div class="px-rows" [style.transform]="offset()" [style.width.px]="contentWidth()">
        <div class="px-row px-header" [style.height.px]="rowHeight()" [style.width.px]="contentWidth()">
          <span class="px-cell px-tree" [style.display]="treePx() > 0 ? 'inline-flex' : 'none'" [style.width.px]="treeWidth">Group</span>
          @for (title of titles(); track title.x) {
            <span class="px-cell" [style.left.px]="title.x" [style.width.px]="cellWidth">{{ title.text }}</span>
          }
        </div>
        @for (row of rows(); track row.index) {
          <div class="px-row" [class.px-stripe]="row.index % 2 === 1" [style.height.px]="rowHeight()" [style.top.px]="row.top" [style.width.px]="contentWidth()">
            <button
              type="button"
              class="px-cell px-tree"
              [style.display]="row.label === null ? 'none' : 'inline-flex'"
              [class.px-total]="row.total"
              [style.width.px]="treeWidth"
              [style.padding-left.px]="row.indent"
              [disabled]="!row.expandable"
              (click)="toggle(row.local)"
            >
              {{ row.label }}
            </button>
            @for (cell of row.cells; track cell.x) {
              <span class="px-cell" [class.px-number]="cell.numeric" [class.px-negative]="cell.negative" [style.left.px]="cell.x" [style.width.px]="cellWidth">{{ cell.text }}</span>
            }
          </div>
        }
      </div>
    </div>
    @if (source.error(); as message) {
      <div class="px-error" role="alert">{{ message }}</div>
    }
  `,
  styles: `
    :host {
      display: block;
      position: relative;
      height: 100%;
      overflow: hidden;
      font: var(--px-font, 13px system-ui, sans-serif);
      color: var(--px-grid-fg, #1d2330);
      background: var(--px-grid-bg, #ffffff);
    }
    .px-scroll {
      position: absolute;
      inset: 0;
      overflow: auto;
    }
    .px-spacer {
      position: absolute;
      top: 0;
      left: 0;
    }
    .px-rows {
      position: sticky;
      top: 0;
      height: 0;
    }
    .px-row {
      position: absolute;
      left: 0;
      white-space: nowrap;
    }
    .px-header {
      position: sticky;
      top: 0;
      z-index: 1;
      background: var(--px-header-bg, #e8ebf0);
    }
    .px-stripe {
      background: var(--px-grid-stripe, #f6f7f9);
    }
    .px-cell {
      position: absolute;
      top: 0;
      bottom: 0;
      box-sizing: border-box;
      display: inline-flex;
      align-items: center;
      padding: 0 6px;
      overflow: hidden;
      text-overflow: ellipsis;
      font: inherit;
      color: inherit;
      background: transparent;
      border: 0;
    }
    .px-tree {
      left: 0;
    }
    .px-number {
      justify-content: flex-end;
    }
    .px-negative {
      color: var(--px-negative, #b42318);
    }
    .px-total {
      color: var(--px-total-fg, #0b4ea2);
    }
    .px-error {
      position: absolute;
      inset: auto 0 0 0;
      padding: 6px 10px;
      background: var(--px-error-bg, #5c1f24);
      color: var(--px-error-fg, #fff);
    }
  `
})
export class DomGridComponent {
  /** The dataset to open. */
  public readonly dataset = input.required<string>();
  /** The layout document, as JSON. */
  public readonly layout = input.required<string>();
  /** Row height in CSS pixels. */
  public readonly rowHeight = input(24);

  protected readonly treeWidth = TREE_WIDTH;
  protected readonly cellWidth = CELL_WIDTH;
  protected readonly source = gridSource(this.dataset, this.layout);
  private readonly scroll = viewChild.required<ElementRef<HTMLDivElement>>("scroll");
  private readonly scrollTop = signal(0);
  private readonly scrollLeft = signal(0);
  private readonly viewportPx = signal(600);
  private readonly widthPx = signal(800);

  protected readonly totalRows = computed(() => this.source.frame()?.totalRows ?? 0);
  protected readonly treePx = computed(() => ((this.source.frame()?.tree ?? null) === null ? 0 : TREE_WIDTH));
  private readonly geometry = computed(() => scrollGeometry(this.totalRows() + 1, this.rowHeight()));
  protected readonly spacerHeight = computed(() => this.geometry().spacerPx);
  protected readonly contentWidth = computed(() => this.treePx() + (this.source.frame()?.totalColumns ?? 0) * CELL_WIDTH);
  /** Data columns with their absolute positions, limited to the visible ones. */
  private readonly placed = computed(() => {
    const frame = this.source.frame();
    if (frame === null) {
      return [];
    }
    const left = this.scrollLeft();
    const right = left + this.widthPx();
    const out: { readonly x: number; readonly column: FrameData["columns"][number] }[] = [];
    frame.columns.forEach((column, i) => {
      const x = this.treePx() + (frame.firstColumn + i) * CELL_WIDTH;
      if (x + CELL_WIDTH > left && x < right) {
        out.push({ x, column });
      }
    });
    return out;
  });
  protected readonly titles = computed((): readonly Title[] => this.placed().map((p) => ({ x: p.x, text: p.column.title })));
  private readonly first = computed(() => this.geometry().firstRow(this.scrollTop(), this.viewportPx()));
  protected readonly offset = computed(() => `translateY(${String(this.geometry().offset(this.scrollTop(), this.viewportPx()))}px)`);
  protected readonly rows = computed(() => this.buildRows(this.source.frame(), this.first(), this.viewportPx()));

  constructor() {
    // Measure once rendered and whenever the grid resizes; that also asks for the first rows.
    const destroyRef = inject(DestroyRef);
    afterNextRender(() => {
      const observer = new ResizeObserver(() => {
        this.onScroll();
      });
      observer.observe(this.scroll().nativeElement);
      destroyRef.onDestroy(() => {
        observer.disconnect();
      });
      this.onScroll();
    });
  }

  protected onScroll(): void {
    const element = this.scroll().nativeElement;
    this.scrollTop.set(element.scrollTop);
    this.scrollLeft.set(element.scrollLeft);
    this.viewportPx.set(element.clientHeight);
    this.widthPx.set(element.clientWidth);
    this.source.show(visibleWindow(this.geometry(), element, this.rowHeight(), this.treePx()));
  }

  protected toggle(local: number): void {
    const frame = this.source.frame();
    if (frame !== null) {
      this.source.toggle(frame, local);
    }
  }

  private buildRow(frame: FrameData, index: number, local: number, top: number): Row {
    const tree = frame.tree;
    const expanded = tree?.expanded(local) ?? null;
    const marker = expanded === null ? "" : expanded ? "▾ " : "▸ ";
    return {
      index,
      local,
      top,
      label: tree === null ? null : `${marker}${tree.label(local)}`,
      indent: 6 + (tree?.depth(local) ?? 0) * 14,
      total: tree?.kind(local) === "total",
      expandable: expanded !== null,
      cells: this.placed().map(({ x, column }) => {
        const text = column.text(local);
        return { x, text, numeric: column.numeric, negative: column.numeric && isNegativeText(text) };
      })
    };
  }

  private buildRows(frame: FrameData | null, first: number, viewportPx: number): readonly Row[] {
    if (frame === null) {
      return [];
    }
    const rowPx = this.rowHeight();
    const visible = Math.ceil(viewportPx / rowPx);
    const rows: Row[] = [];
    for (let i = 0; i < visible; i += 1) {
      const index = first + i;
      const local = index - frame.firstRow;
      if (local >= 0 && local < frame.rowCount) {
        rows.push(this.buildRow(frame, index, local, rowPx * (i + 1)));
      }
    }
    return rows;
  }
}
