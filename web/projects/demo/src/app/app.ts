import { ChangeDetectionStrategy, Component, computed, signal } from "@angular/core";
import { GridComponent } from "parallax";
import type { LayoutInput } from "parallax";
import { z } from "zod";

/** Layout presets the demo and the end-to-end tests open. */
const PRESETS = {
  pivot: {
    dataset: "positions",
    group_by: ["fund", "strategy", "sector", "ticker"],
    split_by: ["asset_class"],
    measures: [{ column: "market_value_usd", aggregate: "sum" }, { column: "daily_pnl_usd", aggregate: "sum" }, { aggregate: "count_all" }],
    expand_depth: 3
  },
  trades: { dataset: "trades" },
  sorted: { dataset: "trades", sort: [{ column: "gross_amount_local", descending: true }] }
} as const satisfies Record<string, LayoutInput>;

const searchSchema = z.strictObject({ preset: z.enum(["pivot", "trades", "sorted"]).default("pivot"), theme: z.enum(["light", "dark"]).default("light") });

type Search = z.infer<typeof searchSchema>;

function readSearch(): Search {
  const parsed = searchSchema.safeParse(Object.fromEntries(new URLSearchParams(location.search)));
  return parsed.success ? parsed.data : searchSchema.parse({});
}

@Component({
  selector: "demo-root",
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [GridComponent],
  template: `
    <header>
      <strong>parallax</strong>
      <span>{{ search.preset }}</span>
      <details>
        <summary>Layout</summary>
        <pre data-testid="layout-json">{{ layoutJson() }}</pre>
      </details>
    </header>
    <main>
      <px-grid [(layout)]="layout" [theme]="search.theme" />
    </main>
  `,
  styles: `
    :host {
      display: grid;
      grid-template-rows: auto 1fr;
      height: 100vh;
    }
    header {
      display: flex;
      gap: 12px;
      padding: 8px 12px;
      font:
        14px system-ui,
        sans-serif;
      border-bottom: 1px solid #d0d5dd;
    }
    main {
      min-height: 0;
    }
    details {
      position: relative;
    }
    pre {
      position: absolute;
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
  protected readonly layout = signal<LayoutInput>(PRESETS[this.search.preset]);
  protected readonly layoutJson = computed(() => JSON.stringify(this.layout(), null, 2));
}
