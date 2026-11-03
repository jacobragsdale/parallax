/** A labelled select for the pivot settings. */

import { ChangeDetectionStrategy, Component, input, output } from "@angular/core";

import type { Choice } from "./labels";

@Component({
  selector: "px-select",
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <select [attr.aria-label]="label()" [value]="value()" [disabled]="disabled()" (change)="pick($event)">
      @for (choice of choices(); track choice.value) {
        <option [value]="choice.value" [selected]="choice.value === value()">{{ choice.text }}</option>
      }
    </select>
  `
})
export class SelectComponent {
  /** The accessible name. */
  public readonly label = input.required<string>();
  /** The options. */
  public readonly choices = input.required<readonly Choice[]>();
  /** The chosen value. */
  public readonly value = input<string>("");
  /** Whether the select is disabled. */
  public readonly disabled = input(false);
  /** Fires with the value picked. */
  public readonly picked = output<string>();

  protected pick(event: Event): void {
    if (event.target instanceof HTMLSelectElement) {
      this.picked.emit(event.target.value);
    }
  }
}
