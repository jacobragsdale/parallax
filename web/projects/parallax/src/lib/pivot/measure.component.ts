/** One value of a pivot: aggregate, column, weight or percentile, and how it is shown. */

import { ChangeDetectionStrategy, Component, computed, input, output } from "@angular/core";

import { AGGREGATES, SHOW_AS } from "../layout";
import type { Layout, LayoutMeasure } from "../layout";
import { measure, moveMeasure, removeMeasure, updateMeasure } from "./edit";
import { NUMERIC_AGGREGATES, NUMERIC_KINDS, choices, fieldChoices } from "./labels";
import type { PivotField } from "./labels";
import { SelectComponent } from "./select.component";

@Component({
  selector: "px-pivot-measure",
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [SelectComponent],
  template: `
    <px-select label="Aggregate" [choices]="aggregates" [value]="measure().aggregate" (picked)="setAggregate($event)" />
    @if (measure().aggregate !== "count_all") {
      <px-select label="Column" [choices]="columns()" [value]="measure().column ?? ''" (picked)="patch({ column: $event })" />
    }
    @if (measure().aggregate === "weighted_mean") {
      <px-select label="Weight" [choices]="weights()" [value]="measure().weight ?? ''" (picked)="patch({ weight: $event === '' ? null : $event })" />
    }
    @if (measure().aggregate === "percentile") {
      <input aria-label="Percentile" type="number" min="0" max="1" step="0.05" [value]="measure().percentile ?? 0.5" (change)="setPercentile($event)" />
    }
    <px-select label="Show as" [choices]="showAs" [value]="measure().show_as" (picked)="setShowAs($event)" />
    <span class="px-row-buttons">
      <button type="button" aria-label="Move value up" [disabled]="index() === 0" (click)="changed.emit(move(-1))">↑</button>
      <button type="button" aria-label="Move value down" [disabled]="index() === layout().measures.length - 1" (click)="changed.emit(move(1))">↓</button>
      <button type="button" aria-label="Remove value" (click)="changed.emit(remove())">×</button>
    </span>
  `
})
export class PivotMeasureComponent {
  /** The layout being edited. */
  public readonly layout = input.required<Layout>();
  /** Which measure this row edits. */
  public readonly index = input.required<number>();
  /** The dataset's columns. */
  public readonly fields = input.required<readonly PivotField[]>();
  /** Fires with each edited layout. */
  public readonly changed = output<Layout>();

  protected readonly aggregates = choices(AGGREGATES);
  protected readonly showAs = choices(SHOW_AS);
  protected readonly measure = computed((): LayoutMeasure => this.layout().measures[this.index()] ?? measure("count_all", null));
  private readonly numeric = computed(() => this.fields().filter((f) => NUMERIC_KINDS.includes(f.kind)));
  protected readonly columns = computed(() => fieldChoices(NUMERIC_AGGREGATES.includes(this.measure().aggregate) ? this.numeric() : this.fields()));
  protected readonly weights = computed(() => [{ value: "", text: "weight…" }, ...fieldChoices(this.numeric())]);

  protected patch(patch: Partial<LayoutMeasure>): void {
    this.changed.emit(updateMeasure(this.layout(), this.index(), patch));
  }

  protected move(delta: number): Layout {
    return moveMeasure(this.layout(), this.index(), delta);
  }

  protected remove(): Layout {
    return removeMeasure(this.layout(), this.index());
  }

  protected setAggregate(value: string): void {
    const aggregate = AGGREGATES.find((a) => a === value);
    if (aggregate === undefined) {
      return;
    }
    const current = this.measure();
    const allowed = NUMERIC_AGGREGATES.includes(aggregate) ? this.numeric() : this.fields();
    const column = aggregate === "count_all" ? null : allowed.some((f) => f.name === current.column) ? current.column : (allowed[0]?.name ?? null);
    this.patch({ aggregate, column, percentile: aggregate === "percentile" ? (current.percentile ?? 0.5) : null, weight: aggregate === "weighted_mean" ? current.weight : null });
  }

  protected setShowAs(value: string): void {
    const showAs = SHOW_AS.find((s) => s === value);
    if (showAs !== undefined) {
      this.patch({ show_as: showAs });
    }
  }

  protected setPercentile(event: Event): void {
    const value = event.target instanceof HTMLInputElement ? Number(event.target.value) : Number.NaN;
    if (Number.isFinite(value) && value >= 0 && value <= 1) {
      this.patch({ percentile: value });
    }
  }
}
