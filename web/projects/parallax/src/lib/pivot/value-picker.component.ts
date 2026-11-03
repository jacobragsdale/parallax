/**
 * Picks filter values from the column's own values, searched on the engine with
 * the view's other filters applied, as a spreadsheet's filter list does. Values
 * travel as exact JSON, so a decimal or a large integer matches what was picked.
 */

import { ChangeDetectionStrategy, Component, DestroyRef, computed, effect, inject, input, output, signal, untracked } from "@angular/core";

import type { DistinctValues } from "../connection";
import { pathValue } from "../format";
import type { LayoutScalar } from "../layout";

/** Lists a column's values for a search. */
export type ValueSource = (column: string, search: string, limit: number) => Promise<DistinctValues>;

/** How many values one search shows. */
const LIMIT = 200;

interface Option {
  readonly key: string;
  readonly value: LayoutScalar;
  readonly text: string;
  readonly rows: string;
}

@Component({
  selector: "px-value-picker",
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    @if (searchable()) {
      <input type="search" aria-label="Search values" placeholder="Search…" [value]="search()" (input)="onSearch($event)" />
    }
    <ul role="listbox" aria-multiselectable="true">
      @for (option of options(); track option.key) {
        <li>
          <label>
            <input type="checkbox" [checked]="chosen().has(option.key)" (change)="toggle(option)" />
            <span>{{ option.text === "" ? "(blank)" : option.text }}</span>
            <span class="px-muted">{{ option.rows }}</span>
          </label>
        </li>
      } @empty {
        <li class="px-muted">{{ note() }}</li>
      }
    </ul>
    @if (truncated()) {
      <p class="px-muted">Showing the first {{ limit }}. Search to narrow them.</p>
    }
  `
})
export class ValuePickerComponent {
  /** The column. */
  public readonly column = input.required<string>();
  /** Where values come from. */
  public readonly source = input.required<ValueSource>();
  /** The values already chosen. */
  public readonly values = input.required<readonly LayoutScalar[]>();
  /** Whether the list can be searched: the engine searches text columns only. */
  public readonly searchable = input(true);
  /** Fires with the chosen values. */
  public readonly changed = output<readonly LayoutScalar[]>();

  protected readonly limit = LIMIT;
  protected readonly search = signal("");
  protected readonly options = signal<readonly Option[]>([]);
  protected readonly truncated = signal(false);
  protected readonly note = signal("Loading…");
  protected readonly chosen = computed((): ReadonlySet<string> => new Set(this.values().map((v) => JSON.stringify(v))));
  private generation = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;

  constructor() {
    inject(DestroyRef).onDestroy(() => {
      this.generation += 1;
      if (this.timer !== null) {
        clearTimeout(this.timer);
      }
    });
    // A new column starts a new list, unsearched.
    effect(() => {
      const column = this.column();
      untracked(() => {
        this.search.set("");
        this.options.set([]);
        this.load(column, "");
      });
    });
  }

  protected onSearch(event: Event): void {
    const text = event.target instanceof HTMLInputElement ? event.target.value : "";
    this.search.set(text);
    if (this.timer !== null) {
      clearTimeout(this.timer);
    }
    this.timer = setTimeout(() => {
      this.load(this.column(), text);
    }, 200);
  }

  protected toggle(option: Option): void {
    const next = new Set(this.chosen());
    if (next.has(option.key)) {
      next.delete(option.key);
    } else {
      next.add(option.key);
    }
    const known = new Map([...this.values().map((v): [string, LayoutScalar] => [JSON.stringify(v), v]), ...this.options().map((o): [string, LayoutScalar] => [o.key, o.value])]);
    this.changed.emit([...next].flatMap((key) => (known.has(key) ? [known.get(key) ?? null] : [])));
  }

  private load(column: string, search: string): void {
    this.generation += 1;
    const mine = this.generation;
    this.note.set("Loading…");
    this.source()(column, search, LIMIT).then(
      (result) => {
        if (mine !== this.generation) {
          return;
        }
        this.options.set(
          result.values.map((v) => {
            const value = pathValue(v.value);
            return { key: JSON.stringify(value), value, text: v.text, rows: v.rows.toLocaleString("en-US") };
          })
        );
        this.truncated.set(result.truncated);
        this.note.set("No values match.");
      },
      (failure: unknown) => {
        if (mine === this.generation) {
          this.note.set(failure instanceof Error ? failure.message : String(failure));
        }
      }
    );
  }
}
