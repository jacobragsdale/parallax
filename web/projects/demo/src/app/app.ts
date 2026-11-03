import { ChangeDetectionStrategy, Component, computed, signal } from "@angular/core";
import { GridComponent, PivotComponent, decodeLayout, encodeLayout } from "parallax";
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

const searchSchema = z.strictObject({ preset: z.enum(["pivot", "trades", "sorted"]).default("pivot"), theme: z.enum(["light", "dark"]).default("light"), layout: z.string().optional() });

type Search = z.infer<typeof searchSchema>;

function readSearch(): Search {
  const parsed = searchSchema.safeParse(Object.fromEntries(new URLSearchParams(location.search)));
  return parsed.success ? parsed.data : searchSchema.parse({});
}

@Component({
  selector: "demo-root",
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [GridComponent, PivotComponent],
  template: `
    <header>
      <strong>parallax</strong>
      <span>{{ search.preset }}</span>
      <button type="button" (click)="share()">Copy link to this view</button>
      <span role="status">{{ note() }}</span>
      <details>
        <summary>Layout</summary>
        <pre data-testid="layout-json">{{ layoutJson() }}</pre>
      </details>
    </header>
    <main>
      @if (ready()) {
        @if (grouped()) {
          <px-pivot [(layout)]="layout" [theme]="search.theme" />
        } @else {
          <px-grid [(layout)]="layout" [theme]="search.theme" />
        }
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
      align-items: center;
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
  /** A linked layout opens as a pivot if it groups, and as a grid otherwise. */
  protected readonly grouped = computed(() => {
    const layout = this.layout();
    return (layout.group_by?.length ?? 0) > 0 || (layout.split_by?.length ?? 0) > 0 || (layout.measures?.length ?? 0) > 0;
  });
  protected readonly note = signal("");
  /** False while a linked layout is being read, so the preset never opens in its place. */
  protected readonly ready = signal(this.search.layout === undefined);

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

  protected share(): void {
    encodeLayout(this.layout()).then(
      (encoded) => {
        const url = new URL(location.href);
        url.searchParams.set("layout", encoded);
        history.replaceState(null, "", url);
        this.note.set("The address bar now links to this view.");
      },
      (failure: unknown) => {
        this.note.set(failure instanceof Error ? failure.message : String(failure));
      }
    );
  }
}
