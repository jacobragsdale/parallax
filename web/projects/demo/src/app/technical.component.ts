import { ChangeDetectionStrategy, Component, computed, input, signal } from "@angular/core";
import { ChartComponent, GridComponent } from "parallax";
import type { Density, LayoutFilter, LayoutIndicator, LayoutInput, ThemeName } from "parallax";

const TICKERS = ["QYE", "ZREZ", "NV"] as const;
type Ticker = (typeof TICKERS)[number];

const KINDS = ["candlestick", "ohlc", "line", "area"] as const;
type Kind = (typeof KINDS)[number];

const SINCE = ["2016-01-01", "2020-01-01", "2024-01-01", "2025-06-01"] as const;
type Since = (typeof SINCE)[number];

/** An indicator the page offers: what it is, and whether it needs candles or reads prices. */
interface Option {
  readonly key: string;
  readonly label: string;
  readonly indicator: LayoutIndicator;
  /** It reads the high, low or volume, which only candles have. */
  readonly candles: boolean;
  /** It is in prices, which a comparison's percentages would not match. */
  readonly priced: boolean;
}

const OPTIONS: readonly Option[] = [
  { key: "sma20", label: "SMA 20", indicator: { kind: "sma", period: 20 }, candles: false, priced: true },
  { key: "sma50", label: "SMA 50", indicator: { kind: "sma", period: 50 }, candles: false, priced: true },
  { key: "sma200", label: "SMA 200", indicator: { kind: "sma", period: 200 }, candles: false, priced: true },
  { key: "ema20", label: "EMA 20", indicator: { kind: "ema", period: 20 }, candles: false, priced: true },
  { key: "wma20", label: "WMA 20", indicator: { kind: "wma", period: 20 }, candles: false, priced: true },
  { key: "bollinger", label: "Bollinger 20 2", indicator: { kind: "bollinger", period: 20, deviations: 2 }, candles: false, priced: true },
  { key: "vwap", label: "VWAP", indicator: { kind: "vwap" }, candles: true, priced: true },
  { key: "rsi", label: "RSI 14", indicator: { kind: "rsi", period: 14 }, candles: false, priced: false },
  { key: "macd", label: "MACD 12 26 9", indicator: { kind: "macd", fast: 12, slow: 26, signal: 9 }, candles: false, priced: false },
  { key: "atr", label: "ATR 14", indicator: { kind: "atr", period: 14 }, candles: true, priced: false },
  { key: "stochastic", label: "Stochastic 14 3 3", indicator: { kind: "stochastic", k: 14, k_smooth: 3, d: 3 }, candles: true, priced: false },
  { key: "volume", label: "Volume", indicator: { kind: "volume" }, candles: true, priced: false }
];

/** The page's settings. */
interface Settings {
  readonly ticker: Ticker;
  readonly kind: Kind;
  readonly since: Since;
  readonly on: ReadonlySet<string>;
  readonly compare: boolean;
  readonly events: boolean;
  readonly log: boolean;
}

function parse<T extends string>(values: readonly T[], value: string): T | null {
  return values.find((v) => v === value) ?? null;
}

/** Whether an option fits the chart the settings make. */
function fits(option: Option, s: Settings): boolean {
  const candles = !s.compare && (s.kind === "candlestick" || s.kind === "ohlc");
  return (candles || !option.candles) && !(s.compare && option.priced);
}

/** What a chart keeps from the one it replaces: its drawings, zoom and hidden series. */
type Kept = Pick<NonNullable<LayoutInput["chart"]>, "drawings" | "range" | "hidden">;

/** The chart the settings make, keeping what the chart before it holds. */
function chartLayout(s: Settings, kept: Kept): LayoutInput {
  const since: LayoutFilter = { column: "date", op: "ge", value: s.since, values: [] };
  const indicators = OPTIONS.filter((o) => s.on.has(o.key) && fits(o, s)).map((o) => o.indicator);
  if (s.compare) {
    return {
      dataset: "prices",
      filters: [{ column: "ticker", op: "in", value: null, values: [...TICKERS] }, since],
      chart: { ...kept, kind: "line", x: "date", y: ["close"], series: "ticker", axis: "trading", compare: true, indicators, events: s.events ? "event" : null }
    };
  }
  const candles = s.kind === "candlestick" || s.kind === "ohlc";
  return {
    dataset: "prices",
    filters: [{ column: "ticker", op: "eq", value: s.ticker, values: [] }, since],
    chart: {
      ...kept,
      kind: s.kind,
      x: "date",
      ...(candles ? { ohlc: { open: "open", high: "high", low: "low", close: "close" } } : { y: ["close"] }),
      axis: "trading",
      log_y: s.log,
      volume: "volume",
      indicators,
      events: s.events ? "event" : null
    }
  };
}

/** The ticker's days, narrowed to a brushed range. */
function rowsLayout(s: Settings, brush: readonly LayoutFilter[]): LayoutInput {
  const ticker: LayoutFilter = s.compare ? { column: "ticker", op: "in", value: null, values: [...TICKERS] } : { column: "ticker", op: "eq", value: s.ticker, values: [] };
  return {
    dataset: "prices",
    filters: [ticker, { column: "date", op: "ge", value: s.since, values: [] }, ...brush],
    sort: [{ column: "date", descending: true }, { column: "ticker" }],
    columns: ["date", "ticker", "open", "high", "low", "close", "volume", "event"]
  };
}

/** One instrument's technical analysis: a chart whose indicators, kind and range the controls set, and its days below. */
@Component({
  selector: "demo-technical",
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [ChartComponent, GridComponent],
  template: `
    <form class="controls" aria-label="Chart settings" (submit)="$event.preventDefault()">
      <label
        >Ticker
        <select aria-label="Ticker" [value]="settings().ticker" [disabled]="settings().compare" (change)="pick('ticker', $event)">
          @for (t of tickers; track t) {
            <option [value]="t" [selected]="t === settings().ticker">{{ t }}</option>
          }
        </select>
      </label>
      <label
        >Chart
        <select aria-label="Chart kind" [value]="settings().kind" [disabled]="settings().compare" (change)="pick('kind', $event)">
          @for (k of kinds; track k) {
            <option [value]="k" [selected]="k === settings().kind">{{ k }}</option>
          }
        </select>
      </label>
      <label
        >Since
        <select aria-label="Since" [value]="settings().since" (change)="pick('since', $event)">
          @for (s of since; track s) {
            <option [value]="s" [selected]="s === settings().since">{{ s.slice(0, 7) }}</option>
          }
        </select>
      </label>
      <label><input type="checkbox" [checked]="settings().compare" (change)="flag('compare')" /> Compare three tickers</label>
      <label><input type="checkbox" [checked]="settings().events" (change)="flag('events')" /> Events</label>
      <label><input type="checkbox" [checked]="settings().log" [disabled]="settings().compare" (change)="flag('log')" /> Log scale</label>
      <fieldset>
        <legend>Indicators</legend>
        @for (o of options(); track o.key) {
          <label><input type="checkbox" [checked]="o.on" [disabled]="!o.fits" (change)="toggle(o.key)" /> {{ o.label }}</label>
        }
      </fieldset>
    </form>
    <section class="chart"><px-chart [(layout)]="chart" [theme]="theme()" (brushed)="brush($event)" /></section>
    <section class="rows">
      <h2>{{ rowsTitle() }}</h2>
      <px-grid [(layout)]="rows" [theme]="theme()" [density]="density()" />
    </section>
  `,
  styles: `
    :host {
      display: grid;
      grid-template-rows: auto minmax(0, 3fr) minmax(0, 1.4fr);
      gap: 8px;
      height: 100%;
      padding: 8px 12px 12px;
      box-sizing: border-box;
    }
    .controls {
      display: flex;
      flex-wrap: wrap;
      align-items: center;
      gap: 6px 16px;
      font-size: 13px;
    }
    fieldset {
      display: flex;
      flex-wrap: wrap;
      gap: 4px 12px;
      margin: 0;
      padding: 2px 8px 4px;
      border: 1px solid #d0d5dd;
      border-radius: 6px;
    }
    :host-context(.dark) fieldset {
      border-color: #2c3240;
    }
    legend {
      font-size: 12px;
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
      padding: 6px 10px 0;
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
export class TechnicalComponent {
  public readonly theme = input<ThemeName>("light");
  public readonly density = input<Density>("compact");

  protected readonly tickers = TICKERS;
  protected readonly kinds = KINDS;
  protected readonly since = SINCE;
  protected readonly settings = signal<Settings>({
    ticker: "QYE",
    kind: "candlestick",
    since: "2024-01-01",
    on: new Set(["sma50", "bollinger", "rsi", "macd", "volume"]),
    compare: false,
    events: true,
    log: false
  });
  protected readonly options = computed(() => OPTIONS.map((o) => ({ key: o.key, label: o.label, on: this.settings().on.has(o.key), fits: fits(o, this.settings()) })));
  protected readonly chart = signal<LayoutInput>(chartLayout(this.settings(), {}));
  private readonly brushed = signal<readonly LayoutFilter[]>([]);
  protected readonly rows = signal<LayoutInput>(rowsLayout(this.settings(), []));
  protected readonly rowsTitle = computed(() => (this.brushed().length === 0 ? "Every day shown, newest first: Shift-drag on the chart to narrow them" : "The days brushed on the chart"));

  /**
   * New settings remake the chart. Another ticker, start date or comparison draws other rows: the zoom goes, the grid
   * lists every day again, and the chart drops its brush itself. Another ticker or a comparison also puts prices on
   * another scale, so drawings anchored to the old one go too. Indicators, events and the log scale change neither.
   */
  private apply(next: Settings, change: { readonly rows: boolean; readonly units: boolean }): void {
    this.settings.set(next);
    this.chart.update((current) => {
      const c = current.chart;
      return chartLayout(next, { drawings: change.units ? [] : c?.drawings, range: change.rows ? null : c?.range, hidden: c?.hidden });
    });
    if (change.rows) {
      this.brushed.set([]);
      this.rows.set(rowsLayout(next, []));
    }
  }

  protected pick(field: "ticker" | "kind" | "since", event: Event): void {
    const value = event.target instanceof HTMLSelectElement ? event.target.value : "";
    const s = this.settings();
    const ticker = field === "ticker" ? parse(TICKERS, value) : s.ticker;
    const kind = field === "kind" ? parse(KINDS, value) : s.kind;
    const since = field === "since" ? parse(SINCE, value) : s.since;
    if (ticker !== null && kind !== null && since !== null) {
      this.apply({ ...s, ticker, kind, since }, { rows: field !== "kind", units: field === "ticker" });
    }
  }

  protected flag(field: "compare" | "events" | "log"): void {
    const s = this.settings();
    const compare = field === "compare";
    this.apply({ ...s, [field]: !s[field] }, { rows: compare, units: compare });
  }

  protected toggle(key: string): void {
    const s = this.settings();
    const on = new Set(s.on);
    if (!on.delete(key)) {
      on.add(key);
    }
    this.apply({ ...s, on }, { rows: false, units: false });
  }

  protected brush(filters: readonly LayoutFilter[] | null): void {
    this.brushed.set(filters ?? []);
    this.rows.set(rowsLayout(this.settings(), filters ?? []));
  }
}
