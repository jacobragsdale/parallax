/** One filter of a pivot: column, operator, and the values it compares with. It edits the filter alone; the panel puts it in the layout. */

import { ChangeDetectionStrategy, Component, computed, input, linkedSignal, output } from "@angular/core";

import { FILTER_OPS } from "../layout";
import type { LayoutFilter, LayoutScalar } from "../layout";
import { filterValue, operandOf } from "./edit";
import { choices, fieldChoices, operandText } from "./labels";
import type { PivotField } from "./labels";
import { SelectComponent } from "./select.component";
import { ValuePickerComponent } from "./value-picker.component";
import type { ValueSource } from "./value-picker.component";

@Component({
  selector: "px-pivot-filter",
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [SelectComponent, ValuePickerComponent],
  template: `
    <px-select label="Filter column" [choices]="columns()" [value]="filter().column" (picked)="patch({ column: $event, value: null, values: [] })" />
    <px-select label="Operator" [choices]="ops()" [value]="filter().op" (picked)="setOp($event)" />
    @if (operand() === "one") {
      <input aria-label="Value" [value]="valueText()" (change)="setValue($event)" />
    } @else if (operand() === "many") {
      <button type="button" class="px-values" [attr.aria-expanded]="picking()" (click)="picking.set(!picking())">{{ summary() }}</button>
    }
    <button type="button" aria-label="Remove filter" (click)="removed.emit()">×</button>
    @if (picking() && operand() === "many") {
      <px-value-picker class="px-popover" [column]="filter().column" [source]="source()" [values]="filter().values" [searchable]="kind() === 'text'" (changed)="setValues($event)" />
    }
  `
})
export class PivotFilterComponent {
  /** The filter being edited. */
  public readonly filter = input.required<LayoutFilter>();
  /** Opens the value list at once, for a filter just added. */
  public readonly startPicking = input(false);
  /** The dataset's columns. */
  public readonly fields = input.required<readonly PivotField[]>();
  /** Where pick-list values come from. */
  public readonly source = input.required<ValueSource>();
  /** Fires with each edit. */
  public readonly changed = output<LayoutFilter>();
  /** Fires when the user removes the filter. */
  public readonly removed = output();

  private readonly kind = computed(() => this.fields().find((f) => f.name === this.filter().column)?.kind ?? "text");
  /** Text matching applies to text columns only. */
  protected readonly ops = computed(() => choices(this.kind() === "text" ? FILTER_OPS : FILTER_OPS.filter((op) => op !== "contains" && op !== "starts_with")));
  protected readonly picking = linkedSignal(() => this.startPicking());
  protected readonly columns = computed(() => fieldChoices(this.fields()));
  protected readonly operand = computed(() => operandOf(this.filter().op));
  protected readonly valueText = computed(() => operandText(this.filter().value));
  protected readonly summary = computed(() => {
    const values = this.filter().values;
    if (values.length === 0) {
      return "Choose values…";
    }
    const shown = values.slice(0, 3).map((v: LayoutScalar) => (v === null ? "(blank)" : String(v)));
    return values.length > 3 ? `${shown.join(", ")} +${String(values.length - 3)}` : shown.join(", ");
  });

  protected patch(patch: Partial<LayoutFilter>): void {
    this.changed.emit({ ...this.filter(), ...patch });
  }

  protected setValues(values: readonly LayoutScalar[]): void {
    this.patch({ values: [...values] });
  }

  protected setOp(value: string): void {
    const op = FILTER_OPS.find((o) => o === value);
    if (op !== undefined) {
      // A list operator opens its value list, since it matches nothing until values are picked.
      this.picking.set(operandOf(op) === "many");
      this.patch({ op });
    }
  }

  protected setValue(event: Event): void {
    const text = event.target instanceof HTMLInputElement ? event.target.value : "";
    this.patch({ value: filterValue(this.kind(), text) });
  }
}
