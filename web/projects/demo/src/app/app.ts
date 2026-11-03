import { ChangeDetectionStrategy, Component, computed, signal, viewChild } from "@angular/core";
import { GridComponent, PivotComponent, decodeLayout, encodeLayout } from "parallax";
import type { Density, DrawStats, LayoutInput, ThemeName } from "parallax";
import { z } from "zod";

import { PerformanceComponent } from "./performance.component";

/** A view the demo opens, and what it shows off. */
interface Preset {
  readonly title: string;
  readonly about: string;
  readonly layout: LayoutInput;
}

const REFERENCE_PIVOT = {
  group_by: ["fund", "strategy", "sector", "ticker"],
  split_by: ["asset_class"],
  measures: [{ column: "market_value_usd", aggregate: "sum" }, { column: "daily_pnl_usd", aggregate: "sum" }, { aggregate: "count_all" }],
  expand_depth: 3
} as const satisfies LayoutInput;

/** The views the demo opens, in the order of their tabs. */
const PRESET_NAMES = ["pivot", "browser", "trades", "sorted", "prices"] as const;

type PresetName = (typeof PRESET_NAMES)[number];

/** The views the demo opens, and the end-to-end tests with them. */
const PRESETS: Readonly<Record<PresetName, Preset>> = {
  pivot: {
    title: "Positions pivot",
    about:
      "200,000 generated positions held on the server. The pivot groups fund, strategy, sector and ticker by asset class. Change it in the settings, double-click a value for the rows behind it, export it to Excel.",
    layout: { dataset: "positions", ...REFERENCE_PIVOT }
  },
  browser: {
    title: "Pivot in the browser",
    about:
      "The same positions, copied into this page and pivoted by the same engine compiled to WebAssembly in a worker. Every change is answered without the server; the cells match the server pivot exactly.",
    layout: { dataset: "positions_browser", ...REFERENCE_PIVOT }
  },
  trades: {
    title: "1,000,000 trades",
    about: "A million generated trades held on the server; the browser receives only the rows on screen. Sort by a header, select with the mouse or keyboard, copy with Ctrl+C.",
    layout: { dataset: "trades" }
  },
  sorted: {
    title: "Largest trades",
    about: "The trades sorted by gross amount, largest first, with amounts shown exactly.",
    layout: { dataset: "trades", sort: [{ column: "gross_amount_local", descending: true }] }
  },
  prices: {
    title: "Daily prices",
    about: "Daily prices for 50 tickers with fat-tailed returns, held in the browser. Charts and technical analysis come in milestones 2 and 3.",
    layout: { dataset: "prices", sort: [{ column: "date", descending: true }, { column: "ticker" }] }
  }
};

const themeSchema = z.enum(["light", "dark"]);
const densitySchema = z.enum(["compact", "comfortable"]);

const searchSchema = z.strictObject({
  preset: z.enum(PRESET_NAMES).default("pivot"),
  theme: themeSchema.default("light"),
  density: densitySchema.default("compact"),
  performance: z.enum(["on", "off"]).default("off"),
  layout: z.string().optional()
});

type Search = z.infer<typeof searchSchema>;

function readSearch(): Search {
  const parsed = searchSchema.safeParse(Object.fromEntries(new URLSearchParams(location.search)));
  return parsed.success ? parsed.data : searchSchema.parse({});
}

/** Sets one search parameter in the address bar without reloading. */
function remember(name: string, value: string): void {
  const url = new URL(location.href);
  url.searchParams.set(name, value);
  history.replaceState(null, "", url);
}

@Component({
  selector: "demo-root",
  changeDetection: ChangeDetectionStrategy.OnPush,
  host: { "[class.dark]": "theme() === 'dark'" },
  imports: [GridComponent, PivotComponent, PerformanceComponent],
  template: `
    <header>
      <strong>parallax</strong>
      <nav aria-label="Views">
        @for (tab of tabs(); track tab.name) {
          <a [href]="tab.href" [attr.aria-current]="tab.current ? 'page' : null">{{ tab.title }}</a>
        }
      </nav>
      <label
        >Theme
        <select [value]="theme()" (change)="setTheme($event)">
          <option value="light">Light</option>
          <option value="dark">Dark</option>
        </select>
      </label>
      <label
        >Row height
        <select [value]="density()" (change)="setDensity($event)">
          <option value="compact">Compact</option>
          <option value="comfortable">Comfortable</option>
        </select>
      </label>
      <button type="button" (click)="share()">Copy link to this view</button>
      <button type="button" [attr.aria-pressed]="performance()" (click)="togglePerformance()">Performance</button>
      <span role="status">{{ note() }}</span>
      <details>
        <summary>Layout</summary>
        <pre data-testid="layout-json">{{ layoutJson() }}</pre>
      </details>
    </header>
    <p class="about">{{ about() }}</p>
    <main>
      @if (ready()) {
        @if (grouped()) {
          <px-pivot [(layout)]="layout" [theme]="theme()" [density]="density()" />
        } @else {
          <px-grid [(layout)]="layout" [theme]="theme()" [density]="density()" />
        }
      }
    </main>
    @if (performance()) {
      <demo-performance [stats]="readStats" [context]="context()" />
    }
  `,
  styles: `
    :host {
      display: grid;
      grid-template-rows: auto auto 1fr;
      height: 100vh;
      font:
        14px system-ui,
        sans-serif;
      color: #1d2330;
      background: #ffffff;
    }
    :host(.dark) {
      color: #e6e9ef;
      background: #14171f;
    }
    :host(.dark) header {
      border-bottom-color: #2c3240;
    }
    :host(.dark) nav a[aria-current="page"] {
      background: #2c3240;
    }
    :host(.dark) .about {
      color: #a3abbb;
    }
    :host(.dark) pre {
      color: #e6e9ef;
      background: #1b1f29;
      border-color: #2c3240;
    }
    header {
      display: flex;
      flex-wrap: wrap;
      align-items: center;
      gap: 8px 14px;
      padding: 8px 12px;
      border-bottom: 1px solid #d0d5dd;
    }
    nav {
      display: flex;
      gap: 4px;
    }
    nav a {
      padding: 4px 8px;
      color: inherit;
      text-decoration: none;
      border-radius: 4px;
    }
    nav a[aria-current="page"] {
      background: #e4e9f2;
      font-weight: 600;
    }
    .about {
      margin: 0;
      padding: 6px 12px;
      color: #475467;
      font-size: 13px;
    }
    main {
      min-height: 0;
    }
    details {
      position: relative;
    }
    pre {
      position: absolute;
      right: 0;
      z-index: 5;
      max-height: 60vh;
      overflow: auto;
      margin: 4px 0 0;
      padding: 8px;
      background: #fff;
      border: 1px solid #d0d5dd;
      font-size: 12px;
    }
  `
})
export class App {
  protected readonly search = readSearch();
  protected readonly layout = signal<LayoutInput>(PRESETS[this.search.preset].layout);
  protected readonly theme = signal<ThemeName>(this.search.theme);
  protected readonly density = signal<Density>(this.search.density);
  protected readonly performance = signal(this.search.performance === "on");
  protected readonly layoutJson = computed(() => JSON.stringify(this.layout(), null, 2));
  /** A linked layout opens as a pivot if it groups, and as a grid otherwise. */
  protected readonly grouped = computed(() => {
    const layout = this.layout();
    return (layout.group_by?.length ?? 0) > 0 || (layout.split_by?.length ?? 0) > 0 || (layout.measures?.length ?? 0) > 0;
  });
  protected readonly note = signal("");
  /** False while a linked layout is being read, so the preset never opens in its place. */
  protected readonly ready = signal(this.search.layout === undefined);
  protected readonly about = computed(() => (this.search.layout === undefined ? PRESETS[this.search.preset].about : "A view opened from a shared link."));
  protected readonly tabs = computed(() =>
    PRESET_NAMES.map((name) => {
      const params = new URLSearchParams({ preset: name, theme: this.theme(), density: this.density(), performance: this.performance() ? "on" : "off" });
      return { name, title: PRESETS[name].title, href: `?${params.toString()}`, current: this.search.layout === undefined && name === this.search.preset };
    })
  );
  protected readonly context = computed(() => `${this.layout().dataset ?? "no dataset"}, ${this.grouped() ? "pivot" : "grid"}, ${this.theme()} theme, ${this.density()} rows`);

  private readonly grid = viewChild(GridComponent);
  private readonly pivot = viewChild(PivotComponent);
  /** The visible grid's drawing counters, for the performance overlay. */
  protected readonly readStats = (): DrawStats | null => this.grid()?.drawStats() ?? this.pivot()?.drawStats() ?? null;

  constructor() {
    const link = this.search.layout;
    if (link !== undefined) {
      decodeLayout(link).then(
        (layout) => {
          this.layout.set(layout);
          this.ready.set(true);
        },
        (failure: unknown) => {
          this.note.set(`The link's layout could not be read, so the preset is shown: ${failure instanceof Error ? failure.message : String(failure)}`);
          this.ready.set(true);
        }
      );
    }
  }

  protected setTheme(event: Event): void {
    const parsed = themeSchema.safeParse(event.target instanceof HTMLSelectElement ? event.target.value : null);
    if (parsed.success) {
      this.theme.set(parsed.data);
      remember("theme", parsed.data);
    }
  }

  protected setDensity(event: Event): void {
    const parsed = densitySchema.safeParse(event.target instanceof HTMLSelectElement ? event.target.value : null);
    if (parsed.success) {
      this.density.set(parsed.data);
      remember("density", parsed.data);
    }
  }

  protected togglePerformance(): void {
    this.performance.update((on) => !on);
    remember("performance", this.performance() ? "on" : "off");
  }

  protected share(): void {
    encodeLayout(this.layout()).then(
      (encoded) => {
        remember("layout", encoded);
        this.note.set("The address bar now links to this view.");
      },
      (failure: unknown) => {
        this.note.set(failure instanceof Error ? failure.message : String(failure));
      }
    );
  }
}
