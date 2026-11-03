import { ChangeDetectionStrategy, Component, input, signal } from "@angular/core";
import { ChartComponent } from "parallax";
import type { LayoutInput, ThemeName } from "parallax";

/** A chart the page shows, with what it says. */
interface Card {
  readonly title: string;
  readonly layout: ReturnType<typeof signal<LayoutInput>>;
}

const THREE = ["QYE", "ZREZ", "NV"];

function card(title: string, layout: LayoutInput): Card {
  return { title, layout: signal(layout) };
}

/** Every chart kind over the generated data: prices held in the browser, positions on the server. */
@Component({
  selector: "demo-charts",
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [ChartComponent],
  template: `
    @for (c of cards; track c.title) {
      <section>
        <h2>{{ c.title }}</h2>
        <px-chart [(layout)]="c.layout" [theme]="theme()" />
      </section>
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
    px-chart {
      flex: 1;
      min-height: 0;
    }
  `
})
export class ChartsComponent {
  public readonly theme = input<ThemeName>("light");

  protected readonly cards: readonly Card[] = [
    card("Closing prices on trading days, log scale", {
      dataset: "prices",
      filters: [{ column: "ticker", op: "in", values: THREE }],
      chart: { kind: "line", x: "date", y: ["close"], series: "ticker", axis: "trading", log_y: true }
    }),
    card("QYE candles, bucketed to fit", {
      dataset: "prices",
      filters: [{ column: "ticker", op: "eq", value: "QYE" }],
      chart: { kind: "candlestick", x: "date", axis: "trading", ohlc: { open: "open", high: "high", low: "low", close: "close" } }
    }),
    card("Market value by sector and asset class", {
      dataset: "positions",
      group_by: ["sector"],
      split_by: ["asset_class"],
      measures: [{ column: "market_value_usd", aggregate: "sum" }],
      formats: { market_value_usd: { abbreviate: "millions" } },
      chart: { kind: "bar", stacked: true }
    }),
    card("Beta of long and short positions", { dataset: "positions", formats: { beta: { decimals: 2 } }, chart: { kind: "histogram", x: "beta", series: "long_short" } }),
    card("Daily P&L against market value", { dataset: "positions", chart: { kind: "scatter", x: "market_value_usd", y: ["daily_pnl_usd"] } }),
    card("Daily P&L by sector and asset class", {
      dataset: "positions",
      group_by: ["sector"],
      split_by: ["asset_class"],
      measures: [{ column: "daily_pnl_usd", aggregate: "sum" }],
      formats: { daily_pnl_usd: { abbreviate: "thousands", negative: "parentheses" } },
      chart: { kind: "heatmap" }
    })
  ];
}
