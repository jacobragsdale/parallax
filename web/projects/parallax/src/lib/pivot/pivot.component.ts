/**
 * `<px-pivot>`: a pivot table with its settings beside it. The settings edit the
 * same layout the grid shows, so `[(layout)]` keeps the host's copy current
 * whether the user changes a setting, sorts, resizes or expands a group. Double-
 * clicking a value opens the rows behind its group in a drawer below.
 */

import { ChangeDetectionStrategy, Component, DOCUMENT, ViewEncapsulation, computed, effect, inject, input, model, signal, untracked, viewChild } from "@angular/core";

import type { DistinctValues } from "../connection";
import { GridComponent } from "../grid/grid.component";
import type { Density, DrawStats, DrillRequest } from "../grid/grid.component";
import { layoutSchema } from "../layout";
import type { Layout, LayoutInput } from "../layout";
import { installThemeStyles } from "../theme";
import type { ThemeName } from "../theme";
import { drillLayout } from "./edit";
import { PivotPanelComponent } from "./pivot-panel.component";
import type { ValueSource } from "./value-picker.component";

/** What decides the rows behind a group: the dataset, filters and groups. */
function computedKey(layout: Layout): string {
  return JSON.stringify([layout.dataset, layout.parameters, layout.filters, layout.group_by, layout.split_by]);
}

const PIVOT_STYLES = `
  px-pivot {
    display: flex;
    height: 100%;
    min-height: 0;
    font: var(--px-font);
    color: var(--px-grid-fg);
    background: var(--px-grid-bg);
  }
  px-pivot .px-pivot-side {
    display: flex;
    flex-direction: column;
    width: 300px;
    min-width: 300px;
    border-right: 1px solid var(--px-grid-line);
    background: var(--px-status-bg);
  }
  px-pivot .px-pivot-side[data-open="false"] {
    width: 28px;
    min-width: 28px;
  }
  px-pivot .px-pivot-toggle {
    align-self: flex-end;
    margin: 4px;
    border: 0;
    background: transparent;
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  px-pivot px-pivot-panel {
    display: block;
    overflow: auto;
    padding: 0 10px 10px;
  }
  px-pivot px-pivot-panel h3 {
    margin: 12px 0 6px;
    color: var(--px-muted-fg);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  px-pivot .px-chips {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin: 0 0 6px;
    padding: 0;
    list-style: none;
  }
  px-pivot .px-chip {
    display: flex;
    align-items: center;
    gap: 2px;
    padding: 2px 4px 2px 8px;
    border: 1px solid var(--px-grid-line);
    border-radius: 3px;
    background: var(--px-grid-bg);
  }
  px-pivot .px-chip span {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  px-pivot button,
  px-pivot select,
  px-pivot input {
    font: inherit;
    color: inherit;
  }
  px-pivot select,
  px-pivot input:not([type="checkbox"]) {
    max-width: 100%;
    padding: 2px 4px;
    border: 1px solid var(--px-grid-line);
    border-radius: 3px;
    background: var(--px-grid-bg);
  }
  px-pivot .px-chip button,
  px-pivot .px-row-buttons button {
    padding: 0 4px;
    border: 0;
    background: transparent;
    cursor: pointer;
  }
  px-pivot button:disabled {
    opacity: 0.35;
    cursor: default;
  }
  px-pivot .px-measure,
  px-pivot .px-filter {
    position: relative;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    margin-bottom: 6px;
    padding: 6px;
    border: 1px solid var(--px-grid-line);
    border-radius: 3px;
    background: var(--px-grid-bg);
  }
  px-pivot .px-row-buttons {
    margin-left: auto;
  }
  px-pivot .px-add,
  px-pivot .px-values {
    padding: 2px 8px;
    border: 1px dashed var(--px-grid-line);
    border-radius: 3px;
    background: transparent;
    cursor: pointer;
  }
  px-pivot .px-option {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 6px;
  }
  px-pivot .px-option input[type="number"] {
    width: 56px;
  }
  px-pivot .px-popover {
    position: absolute;
    top: 100%;
    left: 0;
    z-index: 4;
    display: block;
    width: 260px;
    padding: 6px;
    border: 1px solid var(--px-grid-line);
    border-radius: 3px;
    background: var(--px-grid-bg);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.25);
  }
  px-pivot .px-popover ul {
    max-height: 240px;
    overflow: auto;
    margin: 6px 0 0;
    padding: 0;
    list-style: none;
  }
  px-pivot .px-popover label {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  px-pivot .px-popover label span:first-of-type {
    flex: 1;
  }
  px-pivot .px-muted {
    color: var(--px-muted-fg);
  }
  px-pivot .px-pivot-main {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-width: 0;
  }
  px-pivot .px-pivot-main > px-grid {
    flex: 1;
  }
  px-pivot .px-drill {
    display: flex;
    flex-direction: column;
    height: 40%;
    min-height: 160px;
    border-top: 2px solid var(--px-selection-border);
  }
  px-pivot .px-drill header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    background: var(--px-header-bg);
    color: var(--px-header-fg);
  }
  px-pivot .px-drill header span {
    flex: 1;
  }
  px-pivot .px-drill header button {
    border: 0;
    background: transparent;
    color: inherit;
    cursor: pointer;
  }
  px-pivot .px-drill px-grid {
    flex: 1;
  }
`;

@Component({
  selector: "px-pivot",
  changeDetection: ChangeDetectionStrategy.OnPush,
  encapsulation: ViewEncapsulation.None,
  imports: [GridComponent, PivotPanelComponent],
  host: { "[attr.data-px-theme]": "theme()" },
  template: `
    <aside class="px-pivot-side" [attr.data-open]="panelOpen()">
      <button type="button" class="px-pivot-toggle" [attr.aria-expanded]="panelOpen()" [attr.aria-label]="panelOpen() ? 'Hide settings' : 'Show settings'" (click)="panelOpen.set(!panelOpen())">
        {{ panelOpen() ? "«" : "»" }}
      </button>
      @if (panelOpen() && current(); as active) {
        <px-pivot-panel [layout]="active" [fields]="grid.fields()" [source]="values" (changed)="layout.set($event)" />
      }
    </aside>
    <div class="px-pivot-main">
      <px-grid #grid [(layout)]="layout" [theme]="theme()" [density]="density()" (drill)="openDrill($event)" />
      @if (drill(); as d) {
        <section class="px-drill" aria-label="Rows behind the group">
          <header>
            <span>Rows behind {{ d.label }}</span>
            <button type="button" aria-label="Close" (click)="drill.set(null)">×</button>
          </header>
          <px-grid [layout]="d.layout" [theme]="theme()" [density]="density()" (layoutChange)="updateDrill($event)" />
        </section>
      }
    </div>
  `,
  styles: PIVOT_STYLES
})
export class PivotComponent {
  /** The view. Bind it two ways with `[(layout)]`. */
  public readonly layout = model.required<LayoutInput>();
  /** The theme's tokens; a host can override any of them on the element. */
  public readonly theme = input<ThemeName>("light");
  /** How tightly rows are packed. */
  public readonly density = input<Density>("compact");

  private readonly gridRef = viewChild.required<GridComponent>("grid");

  /** The grid's drawing counters. */
  public drawStats(): DrawStats {
    return this.gridRef().drawStats();
  }
  protected readonly panelOpen = signal(true);
  protected readonly drill = signal<{ readonly label: string; readonly layout: Layout; readonly source: string } | null>(null);
  protected readonly current = computed((): Layout | null => {
    const parsed = layoutSchema.safeParse(this.layout());
    return parsed.success ? parsed.data : null;
  });
  /** Lists values for filter pick-lists from the grid's view. */
  protected readonly values: ValueSource = (column: string, search: string, limit: number): Promise<DistinctValues> => this.gridRef().distinctValues(column, search, limit);

  constructor() {
    installThemeStyles(inject(DOCUMENT));
    // The drawer's rows follow from the pivot's filters and groups; once those
    // change it would show rows that no longer add up to the cells above, so it closes.
    effect(() => {
      const layout = this.current();
      const open = untracked(() => this.drill());
      if (open !== null && (layout === null || computedKey(layout) !== open.source)) {
        this.drill.set(null);
      }
    });
  }

  protected updateDrill(layout: LayoutInput): void {
    const open = this.drill();
    const parsed = layoutSchema.safeParse(layout);
    if (open !== null && parsed.success) {
      this.drill.set({ ...open, layout: parsed.data });
    }
  }

  protected openDrill(request: DrillRequest): void {
    const layout = this.current();
    if (layout !== null) {
      this.drill.set({ label: request.label, layout: drillLayout(layout, request.path, request.split), source: computedKey(layout) });
    }
  }
}
