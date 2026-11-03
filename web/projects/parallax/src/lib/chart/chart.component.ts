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
  signal,
  viewChild
} from "@angular/core";
import type { ElementRef } from "@angular/core";

import { StatusBarComponent } from "../grid/status-bar.component";
import { checkLayout } from "../layout";
import type { Layout, LayoutInput } from "../layout";
import { CHART_COLORS, THEMES, installThemeStyles, readChartColors, readTheme } from "../theme";
import type { ChartColors, Theme, ThemeName } from "../theme";
import { chartSource } from "./chart-source";
import type { PlotSize } from "./chart-source";
import { chartModel } from "./model";
import type { ChartModel } from "./model";
import { paintChart } from "./paint";

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

/** Words for what a chart shows, for its accessible name. */
const KIND_NAMES: Readonly<Record<NonNullable<Layout["chart"]>["kind"], string>> = {
  line: "Line chart",
  area: "Area chart",
  bar: "Bar chart",
  scatter: "Density of points",
  heatmap: "Heatmap",
  histogram: "Histogram",
  candlestick: "Candlestick chart",
  ohlc: "OHLC chart"
};

@Component({
  selector: "px-chart",
  changeDetection: ChangeDetectionStrategy.OnPush,
  encapsulation: ViewEncapsulation.None,
  imports: [StatusBarComponent],
  host: { "[attr.data-px-theme]": "theme()" },
  template: `
    @if (legend().length > 0) {
      <div class="px-chart-legend" role="group" aria-label="Series">
        @for (entry of legend(); track entry.label) {
          <button type="button" class="px-legend-entry" [attr.aria-pressed]="!entry.hidden" (click)="toggle(entry.label)">
            <span class="px-legend-key" [class.px-legend-line]="entry.shape === 'line'" [style.background]="entry.color"></span>
            <span>{{ entry.label }}</span>
          </button>
        }
      </div>
    }
    <div class="px-chart-plot" #plot>
      <canvas class="px-chart-canvas" #canvas role="img" [attr.aria-label]="description()"></canvas>
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
    const chart = layout?.chart;
    if (chart === undefined || chart === null) {
      return "Chart";
    }
    const columns = [...chart.y, ...chart.y_right].join(", ");
    const of = chart.kind === "bar" || chart.kind === "heatmap" ? layout?.group_by.join(", ") : columns === "" ? chart.x : columns;
    return `${KIND_NAMES[chart.kind]} of ${of ?? ""}${chart.series === null ? "" : ` by ${chart.series}`} from ${layout?.dataset ?? ""}`;
  });

  protected readonly summary = computed(() => {
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
