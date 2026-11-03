/**
 * A grid drawn on a Canvas 2D surface, for GPU-less desktops.
 *
 * Only the visible rows and columns are drawn. When a frame arrives for rows already
 * on screen, only the cells whose text changed are redrawn, which keeps both the CPU
 * and a remote-display protocol such as Citrix's busy for as little as possible. The
 * tree column of a grouped view stays pinned while data columns scroll. Numbers too
 * wide for their cell show `###`, as Excel does, rather than losing digits.
 */

import { ChangeDetectionStrategy, Component, DestroyRef, afterNextRender, afterRenderEffect, computed, inject, input, signal, viewChild } from "@angular/core";
import type { ElementRef } from "@angular/core";

import { cellText, isNegative } from "../format";
import type { FrameColumn, FrameData, TreeRows } from "../frame";
import { readTheme } from "../theme";
import type { Theme } from "../theme";
import { CELL_WIDTH, TREE_WIDTH, gridSource, scrollGeometry, visibleWindow } from "./grid-source";

/** Counters the grid experiment reads. */
export interface DrawStats {
  readonly frames: number;
  readonly fullDraws: number;
  readonly cellDraws: number;
  readonly drawMs: number;
}

const PADDING = 6;
const INDENT = 14;

/** A data column placed in content coordinates. */
interface Column {
  readonly frameIndex: number;
  readonly title: string;
  readonly x: number;
  readonly numeric: boolean;
}

/** One pass of drawing: the context, tokens and geometry it uses. */
interface Pass {
  readonly context: CanvasRenderingContext2D;
  readonly theme: Theme;
  readonly rowPx: number;
  readonly full: boolean;
  readonly treePx: number;
  readonly scrollLeft: number;
  readonly width: number;
}

/** What a full redraw depends on; any change forces one. */
interface Shape {
  readonly first: number;
  readonly scrollLeft: number;
  readonly configVersion: bigint;
  readonly loadVersion: bigint;
  readonly totalRows: number;
  readonly firstColumn: number;
}

function sameShape(a: Shape | null, b: Shape): boolean {
  return (
    a !== null &&
    a.first === b.first &&
    a.scrollLeft === b.scrollLeft &&
    a.configVersion === b.configVersion &&
    a.loadVersion === b.loadVersion &&
    a.totalRows === b.totalRows &&
    a.firstColumn === b.firstColumn
  );
}

@Component({
  selector: "px-canvas-grid",
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <div class="px-scroll" #scroll (scroll)="onScroll()">
      <div class="px-spacer" [style.height.px]="spacerHeight()" [style.width.px]="contentWidth()"></div>
      <canvas class="px-canvas" #canvas (click)="onClick($event)" role="grid" [attr.aria-rowcount]="totalRows()"></canvas>
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
    .px-canvas {
      position: sticky;
      top: 0;
      left: 0;
      display: block;
    }
    .px-error {
      position: absolute;
      inset: auto 0 0 0;
      padding: 6px 10px;
      background: var(--px-error-bg, #5c1f24);
      color: var(--px-error-fg, #fff);
      font: var(--px-font, 13px system-ui);
    }
  `
})
export class CanvasGridComponent {
  /** The dataset to open. */
  public readonly dataset = input.required<string>();
  /** The layout document, as JSON. */
  public readonly layout = input.required<string>();
  /** Row height in CSS pixels. */
  public readonly rowHeight = input(24);

  protected readonly source = gridSource(this.dataset, this.layout);
  private readonly scroll = viewChild.required<ElementRef<HTMLDivElement>>("scroll");
  private readonly canvas = viewChild.required<ElementRef<HTMLCanvasElement>>("canvas");
  private readonly scrollTop = signal(0);
  private readonly scrollLeft = signal(0);
  private readonly viewportPx = signal(600);
  private readonly widthPx = signal(800);

  protected readonly totalRows = computed(() => this.source.frame()?.totalRows ?? 0);
  private readonly treePx = computed(() => ((this.source.frame()?.tree ?? null) === null ? 0 : TREE_WIDTH));
  private readonly geometry = computed(() => scrollGeometry(this.totalRows() + 1, this.rowHeight()));
  protected readonly spacerHeight = computed(() => this.geometry().spacerPx);
  protected readonly contentWidth = computed(() => this.treePx() + (this.source.frame()?.totalColumns ?? 0) * CELL_WIDTH);
  private readonly columns = computed(() => placeColumns(this.source.frame(), this.source.groupBy(), this.treePx()));

  private stats: DrawStats = { frames: 0, fullDraws: 0, cellDraws: 0, drawMs: 0 };
  private drawn = new Map<string, string>();
  private drawnRows = new Set<number>();
  private shape: Shape | null = null;
  private theme: Theme | null = null;
  private readonly textWidths = new Map<string, number>();

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
    afterRenderEffect(() => {
      this.draw(this.source.frame(), this.scrollTop(), this.scrollLeft(), this.viewportPx(), this.widthPx());
    });
  }

  /** Drawing counters, for measurement. */
  public drawStats(): DrawStats {
    return this.stats;
  }

  protected onScroll(): void {
    const element = this.scroll().nativeElement;
    this.scrollTop.set(element.scrollTop);
    this.scrollLeft.set(element.scrollLeft);
    this.viewportPx.set(element.clientHeight);
    this.widthPx.set(element.clientWidth);
    this.source.show(visibleWindow(this.geometry(), element, this.rowHeight(), this.treePx()));
  }

  protected onClick(event: MouseEvent): void {
    const frame = this.source.frame();
    const rowPx = this.rowHeight();
    if (frame === null) {
      return;
    }
    if (frame.tree === null || event.offsetX > TREE_WIDTH || event.offsetY < rowPx) {
      return;
    }
    const row = (this.shape?.first ?? 0) + Math.floor((event.offsetY - rowPx) / rowPx);
    const local = row - frame.firstRow;
    if (local >= 0 && local < frame.rowCount) {
      this.source.toggle(frame, local);
    }
  }

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
      this.shape = null;
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
    const first = this.geometry().firstRow(scrollTop, height);
    const pass = this.begin(context, frame, { first, scrollLeft, width, height });
    if (frame !== null) {
      const cells = this.drawRows(pass, frame, first, height);
      this.stats = { frames: this.stats.frames + 1, fullDraws: this.stats.fullDraws, cellDraws: this.stats.cellDraws + cells, drawMs: this.stats.drawMs + performance.now() - started };
    }
  }

  /** Decides whether this pass redraws everything, and if so clears and draws the header. */
  private begin(target: CanvasRenderingContext2D, frame: FrameData | null, at: { readonly first: number; readonly scrollLeft: number; readonly width: number; readonly height: number }): Pass {
    const shape: Shape = {
      first: at.first,
      scrollLeft: at.scrollLeft,
      configVersion: frame?.configVersion ?? -1n,
      loadVersion: frame?.loadVersion ?? -1n,
      totalRows: frame?.totalRows ?? -1,
      firstColumn: frame?.firstColumn ?? -1
    };
    const full = this.theme === null || !sameShape(this.shape, shape);
    const theme = full || this.theme === null ? readTheme(this.canvas().nativeElement) : this.theme;
    this.theme = theme;
    this.shape = shape;
    const pass: Pass = { context: target, theme, rowPx: this.rowHeight(), full, treePx: this.treePx(), scrollLeft: at.scrollLeft, width: at.width };
    const { context } = pass;
    context.font = theme.font;
    context.textBaseline = "middle";
    if (full) {
      this.drawn = new Map();
      this.drawnRows = new Set();
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

  /** Columns that intersect the visible part of the data area. */
  private visibleColumns(pass: Pass): readonly Column[] {
    const left = pass.treePx + pass.scrollLeft;
    const right = pass.scrollLeft + pass.width;
    return this.columns().filter((c) => c.x + CELL_WIDTH > left && c.x < right);
  }

  /** Runs `paint` clipped to the data area, which scrolls under the pinned tree column. */
  private inDataArea(pass: Pass, top: number, height: number, paint: () => void): void {
    const { context } = pass;
    context.save();
    context.beginPath();
    context.rect(pass.treePx, top, Math.max(0, pass.width - pass.treePx), height);
    context.clip();
    paint();
    context.restore();
  }

  private drawHeader(pass: Pass, frame: FrameData | null): void {
    const { context, theme, rowPx } = pass;
    context.fillStyle = theme.headerBackground;
    context.fillRect(0, 0, pass.width, rowPx);
    context.fillStyle = theme.headerForeground;
    if ((frame?.tree ?? null) !== null) {
      context.fillText("Group", PADDING, rowPx / 2);
    }
    this.inDataArea(pass, 0, rowPx, () => {
      for (const column of this.visibleColumns(pass)) {
        this.clipText(context, column.title, column.x - pass.scrollLeft, CELL_WIDTH, rowPx / 2, false);
      }
    });
  }

  private drawRow(pass: Pass, frame: FrameData, local: number, row: number, y: number): number {
    const stripe = row % 2 === 0 ? pass.theme.background : pass.theme.stripe;
    let cells = frame.tree === null ? 0 : this.drawTreeCell(pass, frame.tree, local, row, y, stripe);
    this.inDataArea(pass, y, pass.rowPx, () => {
      for (const column of this.visibleColumns(pass)) {
        const data = frame.columns[column.frameIndex];
        if (data !== undefined && this.drawCell(pass, column, data, local, row, y, stripe)) {
          cells += 1;
        }
      }
    });
    return cells;
  }

  private drawTreeCell(pass: Pass, tree: TreeRows, local: number, row: number, y: number, stripe: string): number {
    const depth = tree.depth(local);
    const expanded = tree.expanded(local);
    const marker = expanded === null ? "  " : expanded ? "▾ " : "▸ ";
    const text = `${marker}${tree.label(local)}`;
    const key = `${String(row)}:tree`;
    if (!pass.full && this.drawn.get(key) === text) {
      return 0;
    }
    const { context, theme, rowPx } = pass;
    context.fillStyle = stripe;
    context.fillRect(0, y, TREE_WIDTH, rowPx);
    context.fillStyle = tree.kind(local) === "total" ? theme.totalForeground : theme.foreground;
    this.clipText(context, text, depth * INDENT, TREE_WIDTH - depth * INDENT, y + rowPx / 2, false);
    this.drawn.set(key, text);
    return 1;
  }

  private drawCell(pass: Pass, column: Column, data: FrameColumn, local: number, row: number, y: number, stripe: string): boolean {
    const value = data.value(local);
    const text = cellText(value, data.flag(local));
    const key = `${String(row)}:${String(column.x)}`;
    if (!pass.full && this.drawn.get(key) === text) {
      return false;
    }
    const { context, theme, rowPx } = pass;
    const x = column.x - pass.scrollLeft;
    context.fillStyle = stripe;
    context.fillRect(x, y, CELL_WIDTH, rowPx);
    context.fillStyle = isNegative(value) ? theme.negative : theme.foreground;
    this.clipText(context, text, x, CELL_WIDTH, y + rowPx / 2, column.numeric);
    this.drawn.set(key, text);
    return true;
  }

  private clipText(context: CanvasRenderingContext2D, text: string, x: number, width: number, y: number, numeric: boolean): void {
    const room = width - 2 * PADDING;
    const measured = this.measure(context, text);
    if (numeric) {
      const shown = measured > room ? "###" : text;
      const w = measured > room ? this.measure(context, shown) : measured;
      context.fillText(shown, x + width - PADDING - w, y);
      return;
    }
    if (measured <= room) {
      context.fillText(text, x + PADDING, y);
      return;
    }
    let cut = text.length;
    while (cut > 0 && this.measure(context, `${text.slice(0, cut)}…`) > room) {
      cut -= 1;
    }
    context.fillText(`${text.slice(0, cut)}…`, x + PADDING, y);
  }
}

/** Places a frame's data columns at their absolute positions in the content. */
function placeColumns(frame: FrameData | null, groupBy: readonly string[], treePx: number): readonly Column[] {
  if (frame === null) {
    return [];
  }
  const columns: Column[] = [];
  let index = frame.firstColumn;
  frame.columns.forEach((column, frameIndex) => {
    if (!groupBy.includes(column.name)) {
      columns.push({ frameIndex, title: column.name, x: treePx + index * CELL_WIDTH, numeric: column.numeric });
      index += 1;
    }
  });
  return columns;
}
