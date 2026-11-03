import { ChangeDetectionStrategy, Component, input, signal } from "@angular/core";
import { ChartComponent, GridComponent } from "parallax";
import type { LayoutFilter, LayoutInput, ThemeName } from "parallax";

/** A chart the page shows, with what it says. */
interface Card {
  /** A name that stays when the title is reworded, such as for the chart's screenshot. */
  readonly id: string;
  readonly title: string;
  readonly layout: ReturnType<typeof signal<LayoutInput>>;
  /** Whether a brush on it narrows the grid beside it. */
  readonly linked: boolean;
}

const THREE = ["QYE", "ZREZ", "NV"];

const QYE: LayoutFilter = { column: "ticker", op: "eq", value: "QYE", values: [] };

const ALL_DAYS = "QYE's days, all of them until a range of candles is selected";

/** The grid beside the candles: QYE's days, narrowed to a brushed range. */
function rowsLayout(brush: readonly LayoutFilter[]): LayoutInput {
  return { dataset: "prices", filters: [QYE, ...brush], sort: [{ column: "date", descending: true }], columns: ["date", "open", "high", "low", "close", "volume"] };
}

function card(id: string, title: string, layout: LayoutInput, linked = false): Card {
  return { id, title, layout: signal(layout), linked };
}

/** Every chart kind over the generated data: prices held in the browser, positions on the server. */
@Component({
  selector: "demo-charts",
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [ChartComponent, GridComponent],
  template: `
    @for (c of cards; track c.id) {
      <section [attr.data-card]="c.id">
        <h2>{{ c.title }}</h2>
        <px-chart [(layout)]="c.layout" [theme]="theme()" (brushed)="brush(c, $event)" />
      </section>
      @if (c.linked) {
        <section>
          <h2>{{ rowsTitle() }}</h2>
          <px-grid [(layout)]="rows" [theme]="theme()" />
        </section>
      }
    }
  `,
  styles: `
    :host {
      display: grid;
      grid-template-columns: repeat(auto-fill, minmax(520px, 1fr));
      grid-auto-rows: 340px;
      gap: 12px;
      padding: 12px;
      overflow: auto;
      height: 100%;
      box-sizing: border-box;
    }
    section {
      display: flex;
      flex-direction: column;
      min-height: 0;
      border: 1px solid #d0d5dd;
      border-radius: 6px;
      overflow: hidden;
    }
    :host-context(.dark) section {
      border-color: #2c3240;
    }
    h2 {
      margin: 0;
      padding: 8px 10px 0;
      font-size: 13px;
      font-weight: 600;
    }
    px-chart,
    px-grid {
      flex: 1;
      min-height: 0;
    }
  `
})
export class ChartsComponent {
  public readonly theme = input<ThemeName>("light");

  /** The grid of QYE's days, narrowed by a brush on the candles. */
  protected readonly rows = signal<LayoutInput>(rowsLayout([]));
  protected readonly rowsTitle = signal(ALL_DAYS);

  /** A brushed range on a linked chart narrows the grid to the days it covers; clearing it shows them all. */
  protected brush(c: Card, filters: readonly LayoutFilter[] | null): void {
    if (!c.linked) {
      return;
    }
    this.rows.set(rowsLayout(filters ?? []));
    this.rowsTitle.set(filters === null ? ALL_DAYS : "QYE's days in the selected candles");
  }

  protected readonly cards: readonly Card[] = [
    card("closes-log", "Closing prices on trading days, log scale", {
      dataset: "prices",
      filters: [{ column: "ticker", op: "in", values: THREE }],
      chart: { kind: "line", x: "date", y: ["close"], series: "ticker", axis: "trading", log_y: true }
    }),
    card(
      "candles",
      "QYE candles, bucketed to fit: drag to pan, wheel to zoom, Shift-drag to select days",
      { dataset: "prices", filters: [QYE], chart: { kind: "candlestick", x: "date", axis: "trading", ohlc: { open: "open", high: "high", low: "low", close: "close" } } },
      true
    ),
    card("close-and-volume", "NV close since 2025, with its volume on a second axis", {
      dataset: "prices",
      filters: [
        { column: "ticker", op: "eq", value: "NV" },
        { column: "date", op: "ge", value: "2025-01-01" }
      ],
      chart: { kind: "line", x: "date", y: ["close"], y_right: ["volume"], axis: "trading" }
    }),
    card("ohlc", "NV OHLC bars since 2024", {
      dataset: "prices",
      filters: [
        { column: "ticker", op: "eq", value: "NV" },
        { column: "date", op: "ge", value: "2024-01-01" }
      ],
      chart: { kind: "ohlc", x: "date", axis: "trading", ohlc: { open: "open", high: "high", low: "low", close: "close" } }
    }),
    card("area", "QYE close since 2025, as an area", {
      dataset: "prices",
      filters: [
        { column: "ticker", op: "eq", value: "QYE" },
        { column: "date", op: "ge", value: "2025-01-01" }
      ],
      chart: { kind: "area", x: "date", y: ["close"], axis: "trading" }
    }),
    card("stacked-bars", "Market value by sector and asset class", {
      dataset: "positions",
      group_by: ["sector"],
      split_by: ["asset_class"],
      measures: [{ column: "market_value_usd", aggregate: "sum" }],
      formats: { market_value_usd: { abbreviate: "millions" } },
      chart: { kind: "bar", stacked: true }
    }),
    card("horizontal-bars", "Market value by sector, long and short side by side", {
      dataset: "positions",
      group_by: ["sector"],
      split_by: ["long_short"],
      measures: [{ column: "market_value_usd", aggregate: "sum" }],
      formats: { market_value_usd: { abbreviate: "millions" } },
      chart: { kind: "bar", horizontal: true }
    }),
    card("histogram", "Beta of long and short positions", { dataset: "positions", formats: { beta: { decimals: 2 } }, chart: { kind: "histogram", x: "beta", series: "long_short" } }),
    card("density", "Daily P&L against market value", { dataset: "positions", chart: { kind: "scatter", x: "market_value_usd", y: ["daily_pnl_usd"] } }),
    card("heatmap", "Daily P&L by sector and asset class", {
      dataset: "positions",
      group_by: ["sector"],
      split_by: ["asset_class"],
      measures: [{ column: "daily_pnl_usd", aggregate: "sum" }],
      formats: { daily_pnl_usd: { abbreviate: "thousands", negative: "parentheses" } },
      chart: { kind: "heatmap" }
    })
  ];
}
