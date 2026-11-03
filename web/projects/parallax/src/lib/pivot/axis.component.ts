/** The row or column groups of a pivot: an ordered list with move and remove buttons, and a list to add from. */

import { ChangeDetectionStrategy, Component, computed, input, output, viewChild } from "@angular/core";
import type { ElementRef } from "@angular/core";

import type { Layout } from "../layout";
import { addGroup, moveGroup, removeGroup } from "./edit";
import type { Axis } from "./edit";
import type { PivotField } from "./labels";

@Component({
  selector: "px-pivot-axis",
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <h3>{{ title() }}</h3>
    <ol class="px-chips">
      @for (column of columns(); track column; let i = $index, last = $last) {
        <li class="px-chip">
          <span>{{ column }}</span>
          <button type="button" [attr.aria-label]="'Move ' + column + ' up'" [disabled]="i === 0" (click)="changed.emit(move(i, -1))">↑</button>
          <button type="button" [attr.aria-label]="'Move ' + column + ' down'" [disabled]="last" (click)="changed.emit(move(i, 1))">↓</button>
          <button type="button" [attr.aria-label]="'Remove ' + column" (click)="changed.emit(remove(i))">×</button>
        </li>
      }
    </ol>
    <select #adder [attr.aria-label]="'Add to ' + title()" (change)="add()">
      <option value="">Add…</option>
      @for (field of available(); track field.name) {
        <option [value]="field.name">{{ field.name }}</option>
      }
    </select>
  `
})
export class PivotAxisComponent {
  /** The layout being edited. */
  public readonly layout = input.required<Layout>();
  /** Rows or columns. */
  public readonly axis = input.required<Axis>();
  /** The heading. */
  public readonly title = input.required<string>();
  /** Columns that can be added. */
  public readonly fields = input.required<readonly PivotField[]>();
  /** Fires with each edited layout. */
  public readonly changed = output<Layout>();

  private readonly adder = viewChild.required<ElementRef<HTMLSelectElement>>("adder");
  protected readonly columns = computed(() => this.layout()[this.axis()]);
  protected readonly available = computed(() => this.fields().filter((f) => !this.columns().includes(f.name)));

  protected move(index: number, delta: number): Layout {
    return moveGroup(this.layout(), this.axis(), index, delta);
  }

  protected remove(index: number): Layout {
    return removeGroup(this.layout(), this.axis(), index);
  }

  protected add(): void {
    const element = this.adder().nativeElement;
    const column = element.value;
    element.value = "";
    if (column !== "") {
      this.changed.emit(addGroup(this.layout(), this.axis(), column));
    }
  }
}
