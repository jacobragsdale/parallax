import { ChangeDetectionStrategy, Component, computed, signal } from "@angular/core";
import { CanvasGridComponent, DomGridComponent } from "parallax";
import { z } from "zod";

/** Layout presets for the grid experiment. */
const PRESETS = {
  pivot: {
    dataset: "positions",
    layout: {
      version: 1,
      group_by: ["fund", "strategy", "sector", "ticker"],
      split_by: ["asset_class"],
      measures: [{ column: "market_value_usd", aggregate: "sum" }, { column: "daily_pnl_usd", aggregate: "sum" }, { aggregate: "count_all" }],
      expand_depth: 3
    }
  },
  trades: { dataset: "trades", layout: { version: 1 } },
  sorted: { dataset: "trades", layout: { version: 1, sort: [{ column: "gross_amount_local", descending: true }] } }
} as const;

const searchSchema = z.strictObject({ grid: z.enum(["canvas", "dom"]).default("canvas"), preset: z.enum(["pivot", "trades", "sorted"]).default("pivot") });

type Search = z.infer<typeof searchSchema>;

function readSearch(): Search {
  const parsed = searchSchema.safeParse(Object.fromEntries(new URLSearchParams(location.search)));
  return parsed.success ? parsed.data : searchSchema.parse({});
}

@Component({
  selector: "demo-root",
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [CanvasGridComponent, DomGridComponent],
  template: `
    <header>
      <strong>parallax</strong>
      <span>{{ search().grid }} grid, {{ search().preset }}</span>
    </header>
    <main>
      @if (search().grid === "canvas") {
        <px-canvas-grid [dataset]="dataset()" [layout]="layout()" />
      } @else {
        <px-dom-grid [dataset]="dataset()" [layout]="layout()" />
      }
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
  `
})
export class App {
  protected readonly search = signal(readSearch());
  protected readonly dataset = computed(() => PRESETS[this.search().preset].dataset);
  protected readonly layout = computed(() => JSON.stringify(PRESETS[this.search().preset].layout));
}
