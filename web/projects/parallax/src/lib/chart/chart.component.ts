/**
 * `<px-chart>`: a chart of a parallax view, drawn on a Canvas 2D surface.
 *
 * The layout's `chart` section says what to draw. The engine reduces line, area,
 * scatter, histogram, candlestick and OHLC charts to the plot's pixels; bar
 * charts and heatmaps draw the layout's grouped view. The legend's buttons hide
 * and show series, which the layout records. Under the chart, the status bar
 * says where the numbers come from and how old they are.
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

import { StatusBarComponent } from "../grid/status-bar.component";
import { checkLayout } from "../layout";
import type { Layout, LayoutFilter, LayoutInput } from "../layout";
import { CHART_COLORS, THEMES, installThemeStyles, readChartColors, readTheme } from "../theme";
import type { ChartColors, Theme, ThemeName } from "../theme";
import { ChartLegendComponent, ChartTipComponent } from "./chart-parts.component";
import type { PlacedReading } from "./chart-parts.component";
import { chartSource } from "./chart-source";
import type { PlotSize } from "./chart-source";
import { brushFilters, markAt, panRange, readSeries, zoomRange } from "./interaction";
import type { Range, Reading } from "./interaction";
import { chartModel, describeChart } from "./model";
import type { ChartModel } from "./model";
import { paintChart } from "./paint";
import { ChartSync } from "./sync";

const CHART_STYLES = `
  px-chart {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 120px;
    background: var(--px-grid-bg);
    color: var(--px-grid-fg);
    font: var(--px-font);
  }
  px-chart .px-chart-legend {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 12px;
    padding: 6px 8px 0;
  }
  px-chart .px-legend-entry {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 2px 4px;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--px-grid-fg);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  px-chart .px-legend-entry[aria-pressed="false"] {
    color: var(--px-muted-fg);
    text-decoration: line-through;
  }
  px-chart .px-legend-entry:hover {
    background: var(--px-grid-stripe);
  }
  px-chart .px-legend-key {
    display: inline-block;
    width: 10px;
    height: 10px;
    border-radius: 2px;
  }
  px-chart .px-legend-key.px-legend-line {
    height: 2px;
    width: 14px;
    border-radius: 1px;
  }
  px-chart .px-legend-entry[aria-pressed="false"] .px-legend-key {
    opacity: 0.35;
  }
  px-chart .px-chart-plot {
    position: relative;
    flex: 1;
    min-height: 0;
  }
  px-chart .px-chart-plot {
    cursor: crosshair;
    outline: none;
    touch-action: none;
  }
  px-chart .px-chart-plot:focus-visible {
    box-shadow: inset 0 0 0 2px var(--px-selection-border);
  }
  px-chart .px-chart-plot.px-chart-panning {
    cursor: grabbing;
  }
  px-chart .px-chart-crosshair {
    position: absolute;
    width: 1px;
    background: var(--px-muted-fg);
    pointer-events: none;
  }
  px-chart .px-chart-brush {
    position: absolute;
    background: var(--px-selection-bg);
    border-left: 1px solid var(--px-selection-border);
    border-right: 1px solid var(--px-selection-border);
    pointer-events: none;
  }
  px-chart .px-chart-tip {
    position: absolute;
    z-index: 2;
    max-width: 280px;
    padding: 6px 8px;
    border: 1px solid var(--px-grid-line);
    border-radius: 4px;
    background: var(--px-header-bg);
    color: var(--px-header-fg);
    font-size: 12px;
    box-shadow: 0 2px 6px rgba(0, 0, 0, 0.2);
    pointer-events: none;
    white-space: nowrap;
  }
  px-chart .px-tip-title {
    margin-bottom: 2px;
    color: var(--px-muted-fg);
  }
  px-chart .px-tip-row {
    display: flex;
    align-items: center;
    gap: 6px;
    font-variant-numeric: tabular-nums;
  }
  px-chart .px-tip-key {
    display: inline-block;
    width: 10px;
    height: 2px;
  }
  px-chart .px-tip-row span:last-child {
    color: var(--px-muted-fg);
  }
  px-chart .px-chart-canvas {
    position: absolute;
    inset: 0;
    display: block;
  }
  px-chart .px-error {
    position: absolute;
    left: 8px;
    right: 8px;
    bottom: 8px;
    padding: 6px 8px;
    border-radius: 4px;
    background: var(--px-danger);
    color: #fff;
    font-size: 12px;
  }
`;

const COUNT = new Intl.NumberFormat("en-US");

/** About how wide a tooltip is, to keep it on the plot's right edge. */
const TIP_WIDTH = 200;

/** How far a step of the wheel or a key zooms. */
const ZOOM_IN = 0.8;
const ZOOM_OUT = 1.25;

/** What each key does to the plot. */
const KEYS: Readonly<Partial<Record<string, "in" | "out" | "left" | "right" | "all" | "clear">>> = {
  "+": "in",
  "=": "in",
  "-": "out",
  ArrowLeft: "left",
  ArrowRight: "right",
  "0": "all",
  Escape: "clear"
};

/** Words for what a chart shows, for its accessible name. */

@Component({
  selector: "px-chart",
  changeDetection: ChangeDetectionStrategy.OnPush,
  encapsulation: ViewEncapsulation.None,
  imports: [ChartLegendComponent, ChartTipComponent, StatusBarComponent],
  host: { "[attr.data-px-theme]": "theme()" },
  template: `
    @if (legend().length > 0) {
      <px-chart-legend [entries]="legend()" (toggled)="toggle($event)" />
    }
    <div
      class="px-chart-plot"
      #plot
      tabindex="0"
      [class.px-chart-panning]="panning()"
      (pointerdown)="onPointerDown($event)"
      (pointermove)="onPointerMove($event)"
      (pointerup)="onPointerUp($event)"
      (pointercancel)="onPointerCancel($event)"
      (pointerleave)="onPointerLeave()"
      (wheel)="onWheel($event)"
      (dblclick)="reset()"
      (keydown)="onKeydown($event)"
    >
      <canvas class="px-chart-canvas" #canvas role="img" [attr.aria-label]="description()"></canvas>
      @if (band(); as b) {
        <div class="px-chart-brush" [style.left.px]="b.left" [style.top.px]="b.top" [style.width.px]="b.width" [style.height.px]="b.height"></div>
      }
      @if (reading(); as r) {
        <px-chart-tip [reading]="r" />
      }
      @if (problem(); as message) {
        <div class="px-error" role="alert">{{ message }}</div>
      }
    </div>
    <px-status-bar
      [dataset]="current()?.dataset ?? ''"
      [parameters]="current()?.parameters ?? {}"
      [summary]="summary()"
      [exports]="false"
      [sourceAsOf]="source.frame()?.sourceAsOfMicros ?? null"
      [status]="source.status()"
      [placement]="source.placement()"
    />
  `,
  styles: CHART_STYLES
})
export class ChartComponent {
  /** The view and its chart. Bind it two ways with `[(layout)]`. */
  public readonly layout = model.required<LayoutInput>();
  /** The light or dark theme. */
  public readonly theme = input<ThemeName>("light");
  /**
   * The group whose crosshairs move together. By default, charts of the same dataset, x column and axis; `null`
   * keeps the crosshair to this chart.
   */
  public readonly sync = input<string | null | undefined>(undefined);
  /** The filters selecting a range brushed with Shift and drag, for linked views; null when the brush is cleared. */
  public readonly brushed = output<readonly LayoutFilter[] | null>();

  private readonly checked = computed(() => checkLayout(this.layout()));
  protected readonly current = computed((): Layout | null => {
    const checked = this.checked();
    return checked.ok && checked.layout.chart !== null ? checked.layout : null;
  });
  protected readonly source = chartSource(this.current);
  private readonly size = signal<PlotSize>({ width: 0, height: 0 });
  private readonly colors = signal<ChartColors>(CHART_COLORS.light);
  private readonly tokens = signal<Theme>(THEMES.light);
  private readonly plot = viewChild.required<ElementRef<HTMLDivElement>>("plot");
  private readonly canvas = viewChild.required<ElementRef<HTMLCanvasElement>>("canvas");
  private readonly document = inject(DOCUMENT);
  private readonly measurer = this.document.createElement("canvas").getContext("2d");
  private readonly widths = new Map<string, number>();

  /** The draw model of the latest data at the plot's size. */
  protected readonly model = computed((): ChartModel | null => {
    const layout = this.current();
    const chart = layout?.chart ?? null;
    if (chart === null) {
      return null;
    }
    const { width, height } = this.size();
    const tokens = this.tokens();
    return chartModel({
      chart,
      width,
      height,
      colors: this.colors(),
      muted: tokens.muted,
      measure: (text) => this.measure(text, tokens.font),
      frame: this.source.frame(),
      categories: this.source.categories()
    });
  });

  protected readonly legend = computed(() => this.model()?.legend ?? []);

  protected readonly problem = computed(() => {
    const checked = this.checked();
    if (!checked.ok) {
      return `Invalid layout: ${checked.error}`;
    }
    if (checked.layout.chart === null) {
      return "The layout has no chart section.";
    }
    return this.source.error();
  });

  protected readonly description = computed(() => {
    const layout = this.current();
    return layout === null ? "Chart" : describeChart(layout);
  });

  /** What the chart draws, and whether it is zoomed. */
  protected readonly summary = computed(() => {
    const drawn = this.drawn();
    const range = this.current()?.chart?.range ?? null;
    return range === null || drawn === "loading…" ? drawn : `${drawn}, zoomed in`;
  });

  private readonly drawn = computed(() => {
    const frame = this.source.frame();
    const categories = this.source.categories();
    if (categories !== null && (this.current()?.chart?.kind === "bar" || this.current()?.chart?.kind === "heatmap")) {
      return `${COUNT.format(categories.categories.length)} groups`;
    }
    if (frame === null) {
      return "loading…";
    }
    const content = frame.content;
    const count = COUNT;
    switch (content.kind) {
      case "series":
        return `${count.format(content.series.reduce((n, s) => n + s.x.length, 0))} points drawn`;
      case "histogram":
        return `${count.format(content.series[0]?.counts.length ?? 0)} bins`;
      case "density":
        return `${count.format(content.counts.reduce((n, c) => n + c, 0))} points`;
      case "candles":
        return `${count.format(content.candles.from.length)} ${content.candles.interval} candles`;
    }
  });

  private readonly syncing = inject(ChartSync);
  /** The pointer over the plot, in pixels. */
  private readonly hover = signal<{ readonly x: number; readonly y: number } | null>(null);
  protected readonly panning = signal(false);
  private drag: { readonly start: number; readonly brush: boolean; readonly range: Range; readonly perPixel: number } | null = null;
  /** The brushed x range in axis positions, while dragging and after. */
  private readonly brush = signal<Range | null>(null);
  /** A zoom or pan waiting for the next animation frame, so that a drag or a burst of wheel events moves the chart once a frame. */
  private pending: { readonly range: Range | null } | null = null;
  private frameRequest: number | null = null;

  /** The group whose crosshairs move with this one's, or null. */
  private readonly group = computed((): string | null => {
    const sync = this.sync();
    if (sync !== undefined) {
      return sync;
    }
    const layout = this.current();
    const chart = layout?.chart;
    if (layout === null || chart === undefined || chart === null || chart.x === "" || !["line", "area", "candlestick", "ohlc"].includes(chart.kind)) {
      return null;
    }
    return JSON.stringify([layout.dataset, chart.x, chart.axis]);
  });

  /** The x in axis positions the pointer over this chart is at, or another chart of its group points at. */
  private pointedX(model: ChartModel): number | null {
    if (model.x === null) {
      return null;
    }
    const hover = this.hover();
    if (hover !== null) {
      return model.x.invert(hover.x);
    }
    // A chart in no group never reads the shared pointer, so other charts' pointers never recompute it.
    const group = this.group();
    if (group === null) {
      return null;
    }
    const pointer = this.syncing.pointer();
    return pointer === null || pointer.source === this || pointer.group !== group ? null : pointer.x;
  }

  /** What the pointer over this chart reads, or another chart of its group points at. */
  private readingNow(model: ChartModel): Reading | null {
    const x = this.pointedX(model);
    if (x === null) {
      const hover = this.hover();
      return hover === null ? null : markAt(model, hover.x, hover.y);
    }
    const frame = this.source.frame();
    if (frame?.content.kind === "series") {
      return readSeries(frame, model, this.current()?.chart?.hidden ?? [], x);
    }
    return model.x === null ? null : markAt(model, model.x.map(x), this.hover()?.y ?? model.plot.y + model.plot.h / 2);
  }

  /** What the crosshair or the pointer reads, and where its tooltip goes. */
  protected readonly reading = computed((): PlacedReading | null => {
    const model = this.model();
    if (model === null || this.panning()) {
      return null;
    }
    const reading = this.readingNow(model);
    if (reading === null) {
      return null;
    }
    // Beside the crosshair or pointer, flipped near the right edge.
    const anchor = reading.crosshair ?? this.hover()?.x ?? 0;
    const left = anchor + 12 + TIP_WIDTH > model.width ? Math.max(0, anchor - 12 - TIP_WIDTH) : anchor + 12;
    return { ...reading, top: model.plot.y, height: model.plot.h, left, tipTop: model.plot.y + 4 };
  });

  /** The brushed range on screen. */
  protected readonly band = computed(() => {
    const brush = this.brush();
    const model = this.model();
    const x = model?.x ?? null;
    if (brush === null || model === null || x === null) {
      return null;
    }
    const [a, b] = [x.map(brush[0]), x.map(brush[1])];
    const left = Math.max(model.plot.x, Math.min(a, b));
    const right = Math.min(model.plot.x + model.plot.w, Math.max(a, b));
    return { left, width: Math.max(0, right - left), top: model.plot.y, height: model.plot.h };
  });

  constructor() {
    installThemeStyles(this.document);
    const destroyRef = inject(DestroyRef);
    afterNextRender(() => {
      const element = this.plot().nativeElement;
      const observer = new ResizeObserver(() => {
        this.size.set({ width: element.clientWidth, height: element.clientHeight });
      });
      observer.observe(element);
      destroyRef.onDestroy(() => {
        observer.disconnect();
      });
      this.size.set({ width: element.clientWidth, height: element.clientHeight });
    });
    destroyRef.onDestroy(() => {
      if (this.frameRequest !== null) {
        cancelAnimationFrame(this.frameRequest);
      }
      // Other charts of the group must not keep following a pointer over a chart that is gone.
      if (this.syncing.pointer()?.source === this) {
        this.syncing.pointer.set(null);
      }
    });
    // The theme's tokens come from the page's styles, so they are read once rendered.
    afterRenderEffect(() => {
      this.theme();
      const element = this.canvas().nativeElement;
      this.colors.set(readChartColors(element));
      this.tokens.set(readTheme(element));
    });
    // Ask for the chart at the plot's size, over the zoomed range if any.
    effect(() => {
      const size = this.size();
      const range = this.current()?.chart?.range ?? null;
      if (size.width > 0 && size.height > 0) {
        this.source.show(size, range === null ? null : [range.from, range.to]);
      }
    });
    afterRenderEffect(() => {
      const model = this.model();
      if (model !== null) {
        this.draw(model);
      }
    });
  }

  /** The data's whole x range, which the engine sends with each frame, and zooming and panning stay within. */
  private extent(): Range | null {
    const frame = this.source.frame();
    return frame === null ? null : [frame.extentFrom, frame.extentTo];
  }

  /** The x range shown, or about to be: a pending zoom or pan, the zoom in the layout, or everything. */
  private shownRange(): Range | null {
    if (this.pending !== null) {
      return this.pending.range ?? this.extent();
    }
    const range = this.current()?.chart?.range;
    return range === null || range === undefined ? this.extent() : [range.from, range.to];
  }

  /** Zooms or pans to `range`, or shows everything for null, at the next animation frame. */
  private schedule(range: Range | null): void {
    this.pending = { range };
    this.frameRequest ??= requestAnimationFrame(() => {
      this.frameRequest = null;
      this.flush();
    });
  }

  private flush(): void {
    const pending = this.pending;
    this.pending = null;
    if (pending !== null) {
      this.setRange(pending.range);
    }
  }

  /** Records a zoomed range in the layout, or none for everything. */
  private setRange(range: Range | null): void {
    const layout = this.current();
    const chart = layout?.chart;
    if (layout === null || chart === undefined || chart === null) {
      return;
    }
    const next = range === null ? null : { from: range[0], to: range[1] };
    if (next?.from !== chart.range?.from || next?.to !== chart.range?.to) {
      this.layout.set({ ...layout, chart: { ...chart, range: next } });
    }
  }

  private local(event: PointerEvent | WheelEvent): { readonly x: number; readonly y: number } {
    const rect = this.plot().nativeElement.getBoundingClientRect();
    return { x: event.clientX - rect.left, y: event.clientY - rect.top };
  }

  /** Starts panning, or with Shift a brush. */
  protected onPointerDown(event: PointerEvent): void {
    const model = this.model();
    const range = this.shownRange();
    if (event.button !== 0 || model?.x === null || model === null || range === null) {
      return;
    }
    this.plot().nativeElement.setPointerCapture(event.pointerId);
    const at = this.local(event);
    this.drag = { start: at.x, brush: event.shiftKey, range, perPixel: (range[1] - range[0]) / model.plot.w };
    if (event.shiftKey) {
      this.brush.set(null);
    } else {
      this.panning.set(true);
    }
  }

  protected onPointerMove(event: PointerEvent): void {
    const at = this.local(event);
    this.hover.set(at);
    const x = this.model()?.x ?? null;
    const group = this.group();
    if (x !== null && group !== null) {
      this.syncing.pointer.set({ group, x: x.invert(at.x), source: this });
    }
    const drag = this.drag;
    if (drag === null || x === null) {
      return;
    }
    if (drag.brush) {
      this.brush.set([x.invert(drag.start), x.invert(at.x)]);
      return;
    }
    this.schedule(panRange(drag.range, (drag.start - at.x) * drag.perPixel, this.extent()));
  }

  /** Ends a drag, returning it. */
  private endDrag(event: PointerEvent): { readonly brush: boolean } | null {
    const drag = this.drag;
    this.drag = null;
    this.panning.set(false);
    this.flush();
    if (this.plot().nativeElement.hasPointerCapture(event.pointerId)) {
      this.plot().nativeElement.releasePointerCapture(event.pointerId);
    }
    return drag;
  }

  /** Ends a pan, or a brush, which then snaps to what it selects and names the filters selecting it. */
  protected onPointerUp(event: PointerEvent): void {
    if (this.endDrag(event)?.brush !== true) {
      return;
    }
    const brush = this.brush();
    const frame = this.source.frame();
    const chart = this.current()?.chart;
    const brushed = brush === null || frame === null || chart === undefined || chart === null ? null : brushFilters(frame, chart.x, chart.hidden, brush);
    this.brush.set(brushed?.range ?? null);
    this.brushed.emit(brushed?.filters ?? null);
  }

  /** A drag the browser took over or the system interrupted ends without a pointerup; a brush it was making is dropped. */
  protected onPointerCancel(event: PointerEvent): void {
    if (this.endDrag(event)?.brush === true) {
      this.clearBrush();
    }
  }

  private clearBrush(): void {
    this.brush.set(null);
    this.brushed.emit(null);
  }

  protected onPointerLeave(): void {
    this.hover.set(null);
    if (this.syncing.pointer()?.source === this) {
      this.syncing.pointer.set(null);
    }
  }

  /** Zooms around the pointer: up to zoom in, down to zoom out. */
  protected onWheel(event: WheelEvent): void {
    const x = this.model()?.x ?? null;
    const range = this.shownRange();
    if (x === null || range === null || event.deltaY === 0) {
      return;
    }
    event.preventDefault();
    this.schedule(zoomRange(range, x.invert(this.local(event).x), event.deltaY < 0 ? ZOOM_IN : ZOOM_OUT, this.extent()));
  }

  /** Shows everything again. */
  protected reset(): void {
    this.pending = { range: null };
    this.flush();
  }

  /** Plus and minus zoom, the arrows pan, 0 shows everything and Escape clears the brush. */
  protected onKeydown(event: KeyboardEvent): void {
    const key = KEYS[event.key];
    const range = this.shownRange();
    if (key === undefined || range === null) {
      return;
    }
    event.preventDefault();
    const [middle, step, extent] = [(range[0] + range[1]) / 2, (range[1] - range[0]) / 10, this.extent()];
    switch (key) {
      case "in":
      case "out":
        this.schedule(zoomRange(range, middle, key === "in" ? ZOOM_IN : ZOOM_OUT, extent));
        return;
      case "left":
      case "right":
        this.schedule(panRange(range, key === "left" ? -step : step, extent));
        return;
      case "all":
        this.reset();
        return;
      case "clear":
        this.clearBrush();
        return;
    }
  }

  /** Hides a series, or shows it again. */
  protected toggle(label: string): void {
    const layout = this.current();
    const chart = layout?.chart;
    if (layout === null || chart === undefined || chart === null) {
      return;
    }
    const hidden = chart.hidden.includes(label) ? chart.hidden.filter((h) => h !== label) : [...chart.hidden, label];
    this.layout.set({ ...layout, chart: { ...chart, hidden } });
  }

  private measure(text: string, font: string): number {
    const key = `${font}\u0000${text}`;
    let width = this.widths.get(key);
    if (width === undefined) {
      if (this.measurer === null) {
        return text.length * 7;
      }
      this.measurer.font = font;
      width = this.measurer.measureText(text).width;
      if (this.widths.size > 10_000) {
        this.widths.clear();
      }
      this.widths.set(key, width);
    }
    return width;
  }

  private draw(model: ChartModel): void {
    const element = this.canvas().nativeElement;
    const context = element.getContext("2d", { alpha: false });
    if (context === null) {
      return;
    }
    const ratio = window.devicePixelRatio;
    const [width, height] = [Math.max(1, Math.round(model.width * ratio)), Math.max(1, Math.round(model.height * ratio))];
    if (element.width !== width || element.height !== height) {
      element.width = width;
      element.height = height;
      element.style.width = `${String(model.width)}px`;
      element.style.height = `${String(model.height)}px`;
    }
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    paintChart(context, model, this.tokens());
  }
}
