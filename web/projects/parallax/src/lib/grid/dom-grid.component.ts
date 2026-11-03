/**
 * A virtualized DOM grid: the same data plumbing as the canvas grid, drawn with
 * positioned elements. It exists for the measured comparison that decides which
 * grid parallax ships.
 */

import { ChangeDetectionStrategy, Component, DestroyRef, afterNextRender, computed, inject, input, signal, viewChild } from "@angular/core";
import type { ElementRef } from "@angular/core";

import { cellText, isNegative, pathValue } from "../format";
import type { FrameColumn, FrameData } from "../frame";
import { gridSource, scrollGeometry } from "./grid-source";

interface Cell {
  readonly text: string;
  readonly numeric: boolean;
  readonly negative: boolean;
}

interface Row {
  readonly index: number;
  readonly top: number;
  readonly label: string | null;
  readonly indent: number;
  readonly total: boolean;
  readonly expandable: boolean;
  readonly cells: readonly Cell[];
}

const TREE_WIDTH = 260;
const CELL_WIDTH = 140;

@Component({
  selector: "px-dom-grid",
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <div class="px-scroll" #scroll (scroll)="onScroll()" role="grid" [attr.aria-rowcount]="totalRows()">
      <div class="px-spacer" [style.height.px]="spacerHeight()" [style.width.px]="contentWidth()"></div>
      <div class="px-rows" [style.transform]="offset()">
        <div class="px-row px-header" [style.height.px]="rowHeight()">
          <span class="px-cell px-tree" [style.display]="tree() ? 'inline-flex' : 'none'" [style.width.px]="treeWidth">Group</span>
          @for (title of titles(); track $index) {
            <span class="px-cell" [style.width.px]="cellWidth">{{ title }}</span>
          }
        </div>
        @for (row of rows(); track row.index) {
          <div class="px-row" [class.px-stripe]="row.index % 2 === 1" [style.height.px]="rowHeight()" [style.top.px]="row.top">
            <button
              type="button"
              class="px-cell px-tree"
              [style.display]="row.label === null ? 'none' : 'inline-flex'"
              [class.px-total]="row.total"
              [style.width.px]="treeWidth"
              [style.padding-left.px]="row.indent"
              [disabled]="!row.expandable"
              (click)="toggle(row.index)"
            >
              {{ row.label }}
            </button>
            @for (cell of row.cells; track $index) {
              <span class="px-cell" [class.px-number]="cell.numeric" [class.px-negative]="cell.negative" [style.width.px]="cellWidth">{{ cell.text }}</span>
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
      left: 0;
      height: 0;
    }
    .px-row {
      position: absolute;
      left: 0;
      display: flex;
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
  private readonly viewportPx = signal(600);

  protected readonly totalRows = computed(() => this.source.frame()?.totalRows ?? 0);
  protected readonly tree = computed(() => (this.source.frame()?.tree ?? null) !== null);
  private readonly geometry = computed(() => scrollGeometry(this.totalRows() + 1, this.rowHeight()));
  protected readonly spacerHeight = computed(() => this.geometry().spacerPx);
  private readonly shown = computed((): readonly FrameColumn[] => {
    const frame = this.source.frame();
    const groupBy = this.source.groupBy();
    return frame === null ? [] : frame.columns.filter((c) => !groupBy.includes(c.name));
  });
  protected readonly titles = computed(() => this.shown().map((c) => c.name));
  protected readonly contentWidth = computed(() => (this.tree() ? TREE_WIDTH : 0) + this.shown().length * CELL_WIDTH);
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
    this.viewportPx.set(element.clientHeight);
    const rows = Math.ceil(element.clientHeight / this.rowHeight()) + 1;
    this.source.request(this.geometry().firstRow(element.scrollTop, element.clientHeight), rows);
  }

  protected toggle(row: number): void {
    const frame = this.source.frame();
    const tree = frame?.tree ?? null;
    if (frame === null || tree === null) {
      return;
    }
    const local = row - frame.firstRow;
    const expanded = tree.expanded(local);
    if (expanded === null) {
      return;
    }
    const path = this.source
      .groupBy()
      .slice(0, tree.depth(local))
      .map((name) => {
        const column = frame.columns.find((c) => c.name === name);
        return column === undefined ? null : pathValue(column.value(local));
      });
    this.source.expand(path, !expanded);
  }

  private buildRow(frame: FrameData, index: number, local: number, top: number): Row {
    const tree = frame.tree;
    const expanded = tree?.expanded(local) ?? null;
    const marker = expanded === null ? "" : expanded ? "▾ " : "▸ ";
    return {
      index,
      top,
      label: tree === null ? null : `${marker}${tree.label(local)}`,
      indent: 6 + (tree?.depth(local) ?? 0) * 14,
      total: tree?.kind(local) === "total",
      expandable: expanded !== null,
      cells: this.shown().map((c) => {
        const value = c.value(local);
        return { text: cellText(value, c.flag(local)), numeric: c.numeric, negative: isNegative(value) };
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
