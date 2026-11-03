/** Pivot options: how many levels open by default, and top-N. */

import { ChangeDetectionStrategy, Component, computed, input, output } from "@angular/core";

import type { Layout } from "../layout";
import { setExpandDepth, setTopN } from "./edit";
import { measureName } from "./labels";
import { SelectComponent } from "./select.component";

@Component({
  selector: "px-pivot-options",
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [SelectComponent],
  template: `
    <label class="px-option">
      Open levels
      <input type="number" min="0" [max]="layout().group_by.length" [value]="layout().expand_depth" (change)="setDepth($event)" />
    </label>
    <label class="px-option">
      <input type="checkbox" [checked]="layout().top_n !== null" [disabled]="layout().measures.length === 0" (change)="toggleTopN($event)" />
      Top
      <input type="number" min="1" aria-label="How many groups" [disabled]="layout().top_n === null" [value]="layout().top_n?.n ?? 10" (change)="setCount($event)" />
    </label>
    <px-select label="Top-N measure" [choices]="measures()" [value]="topMeasure()" [disabled]="layout().top_n === null" (picked)="setMeasure($event)" />
  `
})
export class PivotOptionsComponent {
  /** The layout being edited. */
  public readonly layout = input.required<Layout>();
  /** Fires with each edited layout. */
  public readonly changed = output<Layout>();

  protected readonly measures = computed(() => this.layout().measures.map((m, i) => ({ value: String(i), text: `by ${measureName(m)}` })));
  protected readonly topMeasure = computed(() => String(this.layout().top_n?.measure ?? 0));

  private number(event: Event): number {
    return event.target instanceof HTMLInputElement ? Number(event.target.value) : Number.NaN;
  }

  protected setDepth(event: Event): void {
    const depth = this.number(event);
    if (Number.isFinite(depth)) {
      this.changed.emit(setExpandDepth(this.layout(), depth));
    }
  }

  protected toggleTopN(event: Event): void {
    const on = event.target instanceof HTMLInputElement && event.target.checked;
    this.changed.emit(setTopN(this.layout(), on ? { n: 10, measure: 0, largest: true, other: true } : null));
  }

  protected setCount(event: Event): void {
    const top = this.layout().top_n;
    const n = Math.floor(this.number(event));
    if (top !== null && Number.isFinite(n) && n >= 1) {
      this.changed.emit(setTopN(this.layout(), { ...top, n }));
    }
  }

  protected setMeasure(value: string): void {
    const top = this.layout().top_n;
    const measure = Number(value);
    if (top !== null && Number.isInteger(measure)) {
      this.changed.emit(setTopN(this.layout(), { ...top, measure }));
    }
  }
}
