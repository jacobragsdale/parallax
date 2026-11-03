/**
 * A grid drawn on a Canvas 2D surface, for GPU-less desktops.
 *
 * Only the visible rows are drawn. When a frame arrives for rows already on screen,
 * only the cells whose text changed are redrawn, which keeps both the CPU and a
 * remote-display protocol such as Citrix's busy for as little as possible. Numbers
 * too wide for their cell show `###`, as Excel does, rather than losing digits.
 */

import { ChangeDetectionStrategy, Component, DestroyRef, afterNextRender, afterRenderEffect, computed, inject, input, signal, viewChild } from "@angular/core";
import type { ElementRef } from "@angular/core";

import { cellText, isNegative, pathValue } from "../format";
import type { FrameColumn, FrameData, TreeRows } from "../frame";
import { readTheme } from "../theme";
import type { Theme } from "../theme";
import { gridSource, scrollGeometry } from "./grid-source";

/** Counters the grid experiment reads. */
export interface DrawStats {
  readonly frames: number;
  readonly fullDraws: number;
  readonly cellDraws: number;
  readonly drawMs: number;
}

const TREE_WIDTH = 260;
const CELL_WIDTH = 140;
const PADDING = 6;
const INDENT = 14;

interface Column {
  readonly frameIndex: number;
  readonly title: string;
  readonly x: number;
  readonly width: number;
  readonly numeric: boolean;
}

/** One pass of drawing: the context, tokens and geometry it uses. */
interface Pass {
  readonly context: CanvasRenderingContext2D;
  readonly theme: Theme;
  readonly rowPx: number;
  readonly full: boolean;
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
  private readonly viewportPx = signal(600);
  private readonly widthPx = signal(800);

  protected readonly totalRows = computed(() => this.source.frame()?.totalRows ?? 0);
  private readonly geometry = computed(() => scrollGeometry(this.totalRows() + 1, this.rowHeight()));
  protected readonly spacerHeight = computed(() => this.geometry().spacerPx);
  private readonly columns = computed(() => layoutColumns(this.source.frame(), this.source.groupBy()));
  protected readonly contentWidth = computed(() => {
    const last = this.columns().at(-1);
    return last === undefined ? 0 : last.x + last.width;
  });

  private stats: DrawStats = { frames: 0, fullDraws: 0, cellDraws: 0, drawMs: 0 };
  private drawn = new Map<string, string>();
  private lastFrame: FrameData | null = null;
  private lastFirst = -1;
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
      this.draw(this.source.frame(), this.scrollTop(), this.viewportPx(), this.widthPx());
    });
  }

  /** Drawing counters, for measurement. */
  public drawStats(): DrawStats {
    return this.stats;
  }

  protected onScroll(): void {
    const element = this.scroll().nativeElement;
    this.scrollTop.set(element.scrollTop);
    this.viewportPx.set(element.clientHeight);
    this.widthPx.set(element.clientWidth);
    const rows = Math.ceil(element.clientHeight / this.rowHeight()) + 1;
    this.source.request(this.geometry().firstRow(element.scrollTop, element.clientHeight), rows);
  }

  protected onClick(event: MouseEvent): void {
    const frame = this.source.frame();
    const tree = frame?.tree ?? null;
    if (frame === null || tree === null || event.offsetX > TREE_WIDTH) {
      return;
    }
    const row = this.lastFirst + Math.floor((event.offsetY - this.rowHeight()) / this.rowHeight());
    const local = row - frame.firstRow;
    if (local < 0 || local >= frame.rowCount) {
      return;
    }
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
      this.lastFirst = -1;
    }
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    return context;
  }

  private draw(frame: FrameData | null, scrollTop: number, viewportPx: number, widthPx: number): void {
    const started = performance.now();
    const element = this.canvas().nativeElement;
    const height = Math.max(1, viewportPx);
    const width = Math.max(1, widthPx);
    const context = this.prepare(width, height);
    if (context === null) {
      return;
    }
    const theme = readTheme(element);
    context.font = theme.font;
    context.textBaseline = "middle";
    const rowPx = this.rowHeight();
    const first = this.geometry().firstRow(scrollTop, height);
    const full = first !== this.lastFirst || frame?.configVersion !== this.lastFrame?.configVersion || frame?.loadVersion !== this.lastFrame?.loadVersion;
    const pass: Pass = { context, theme, rowPx, full };
    if (full) {
      this.drawn = new Map();
      context.fillStyle = theme.background;
      context.fillRect(0, 0, width, height);
      this.drawHeader(pass, frame, width);
      this.stats = { ...this.stats, fullDraws: this.stats.fullDraws + 1 };
    }
    this.lastFirst = first;
    this.lastFrame = frame;
    if (frame === null) {
      return;
    }
    let cells = 0;
    const visible = Math.ceil(height / rowPx);
    for (let i = 0; i < visible; i += 1) {
      const row = first + i;
      const local = row - frame.firstRow;
      if (local >= 0 && local < frame.rowCount) {
        cells += this.drawRow(pass, frame, local, row, rowPx * (i + 1));
      }
    }
    this.stats = { frames: this.stats.frames + 1, fullDraws: this.stats.fullDraws, cellDraws: this.stats.cellDraws + cells, drawMs: this.stats.drawMs + performance.now() - started };
  }

  private drawHeader(pass: Pass, frame: FrameData | null, width: number): void {
    const { context, theme, rowPx } = pass;
    context.fillStyle = theme.headerBackground;
    context.fillRect(0, 0, width, rowPx);
    context.fillStyle = theme.headerForeground;
    if ((frame?.tree ?? null) !== null) {
      context.fillText("Group", PADDING, rowPx / 2);
    }
    for (const column of this.columns()) {
      this.clipText(context, column.title, column.x, column.width, rowPx / 2, false);
    }
  }

  private drawRow(pass: Pass, frame: FrameData, local: number, row: number, y: number): number {
    const stripe = row % 2 === 0 ? pass.theme.background : pass.theme.stripe;
    let cells = frame.tree === null ? 0 : this.drawTreeCell(pass, frame.tree, local, row, y, stripe);
    for (const column of this.columns()) {
      const data = frame.columns[column.frameIndex];
      if (data !== undefined && this.drawCell(pass, column, data, local, row, y, stripe)) {
        cells += 1;
      }
    }
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
    const key = `${String(row)}:${String(column.frameIndex)}`;
    if (!pass.full && this.drawn.get(key) === text) {
      return false;
    }
    const { context, theme, rowPx } = pass;
    context.fillStyle = stripe;
    context.fillRect(column.x, y, column.width, rowPx);
    context.fillStyle = isNegative(value) ? theme.negative : theme.foreground;
    this.clipText(context, text, column.x, column.width, y + rowPx / 2, column.numeric);
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

function layoutColumns(frame: FrameData | null, groupBy: readonly string[]): readonly Column[] {
  if (frame === null) {
    return [];
  }
  const columns: Column[] = [];
  let x = frame.tree === null ? 0 : TREE_WIDTH;
  frame.columns.forEach((column, frameIndex) => {
    if (!groupBy.includes(column.name)) {
      columns.push({ frameIndex, title: column.name, x, width: CELL_WIDTH, numeric: column.numeric });
      x += CELL_WIDTH;
    }
  });
  return columns;
}
