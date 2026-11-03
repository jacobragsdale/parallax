/** The pivot's settings: row and column groups, values, filters and options. */

import { ChangeDetectionStrategy, Component, computed, input, output, signal } from "@angular/core";

import type { Layout, LayoutFilter } from "../layout";
import { PivotAxisComponent } from "./axis.component";
import { addFilter, addMeasure, filter, measure, operandOf, removeFilter, updateFilter } from "./edit";
import { PivotFilterComponent } from "./filter.component";
import { NUMERIC_KINDS } from "./labels";
import type { PivotField } from "./labels";
import { PivotMeasureComponent } from "./measure.component";
import { PivotOptionsComponent } from "./options.component";
import type { ValueSource } from "./value-picker.component";

/** Whether a filter says what to match: an empty list or a missing value would hide every row. */
function complete(f: LayoutFilter): boolean {
  const operand = operandOf(f.op);
  return operand === "none" || (operand === "one" ? f.value !== null : f.values.length > 0);
}

@Component({
  selector: "px-pivot-panel",
  changeDetection: ChangeDetectionStrategy.OnPush,
  imports: [PivotAxisComponent, PivotMeasureComponent, PivotFilterComponent, PivotOptionsComponent],
  template: `
    <px-pivot-axis title="Rows" axis="group_by" [layout]="layout()" [fields]="groupable()" (changed)="changed.emit($event)" />
    <px-pivot-axis title="Columns" axis="split_by" [layout]="layout()" [fields]="groupable()" (changed)="changed.emit($event)" />
    <section aria-label="Values">
      <h3>Values</h3>
      @for (m of layout().measures; track $index) {
        <px-pivot-measure class="px-measure" [layout]="layout()" [index]="$index" [fields]="fields()" (changed)="changed.emit($event)" />
      }
      <button type="button" class="px-add" (click)="addValue()">Add value</button>
    </section>
    <section aria-label="Filters">
      <h3>Filters</h3>
      @for (f of layout().filters; track $index) {
        <px-pivot-filter
          class="px-filter"
          [filter]="f"
          [startPicking]="$index === picking()"
          [fields]="fields()"
          [source]="source()"
          (changed)="editFilter($index, $event)"
          (removed)="changed.emit(removeFilter(layout(), $index))"
        />
      }
      @if (draft(); as d) {
        <px-pivot-filter class="px-filter" [filter]="d" [startPicking]="true" [fields]="fields()" [source]="source()" (changed)="editDraft($event)" (removed)="draft.set(null)" />
      }
      <button type="button" class="px-add" [disabled]="fields().length === 0 || draft() !== null" (click)="addOneFilter()">Add filter</button>
    </section>
    <section aria-label="Options">
      <h3>Options</h3>
      <px-pivot-options [layout]="layout()" (changed)="changed.emit($event)" />
    </section>
  `
})
export class PivotPanelComponent {
  /** The layout being edited. */
  public readonly layout = input.required<Layout>();
  /** The dataset's columns. */
  public readonly fields = input.required<readonly PivotField[]>();
  /** Where filter pick-list values come from. */
  public readonly source = input.required<ValueSource>();
  /** Fires with each edited layout. */
  public readonly changed = output<Layout>();

  /** A filter being set up, kept out of the layout until it has a value: an empty "is one of" would hide every row. */
  protected readonly draft = signal<LayoutFilter | null>(null);
  protected readonly removeFilter = removeFilter;
  /** The filter whose value list stays open after its draft was added. */
  protected readonly picking = signal<number | null>(null);

  /** Columns worth grouping by: anything but doubles, whose values rarely repeat. */
  protected readonly groupable = computed(() => this.fields().filter((f) => f.kind !== "float"));

  protected addValue(): void {
    const column = this.fields().find((f) => NUMERIC_KINDS.includes(f.kind))?.name ?? null;
    this.changed.emit(addMeasure(this.layout(), column === null ? measure("count_all", null) : measure("sum", column)));
  }

  /** Adds a filter on the outermost row group, or else the first text column, as a pick-list. */
  protected addOneFilter(): void {
    const fields = this.fields();
    const outer = this.layout().group_by[0];
    const first = fields.find((f) => f.name === outer) ?? fields.find((f) => f.kind === "text") ?? fields[0];
    if (first !== undefined) {
      this.draft.set(filter(first.name, first.kind === "text" ? "in" : "eq"));
    }
  }

  /** Edits a filter in the layout; one that no longer says what to match goes back to being a draft. */
  protected editFilter(index: number, edited: LayoutFilter): void {
    if (complete(edited)) {
      this.changed.emit(updateFilter(this.layout(), index, edited));
    } else {
      this.draft.set(edited);
      this.changed.emit(removeFilter(this.layout(), index));
    }
  }

  /** Adds the draft to the layout once it says what to match. */
  protected editDraft(edited: LayoutFilter): void {
    if (complete(edited)) {
      this.draft.set(null);
      this.picking.set(this.layout().filters.length);
      this.changed.emit(addFilter(this.layout(), edited));
    } else {
      this.draft.set(edited);
    }
  }
}
