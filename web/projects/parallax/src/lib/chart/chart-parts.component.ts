/** Pieces of `<px-chart>` that sit over or beside its canvas: the legend, and the crosshair with its tooltip. */

import { ChangeDetectionStrategy, Component, ViewEncapsulation, input, output } from "@angular/core";

import type { TipRow } from "./interaction";
import type { LegendEntry } from "./model";

/** A reading placed on the plot. */
export interface PlacedReading {
  readonly title: string;
  readonly rows: readonly TipRow[];
  readonly crosshair: number | null;
  readonly top: number;
  readonly height: number;
  readonly left: number;
  readonly tipTop: number;
}

/** The legend: a button per series that hides it or shows it again. */
@Component({
  selector: "px-chart-legend",
  changeDetection: ChangeDetectionStrategy.OnPush,
  encapsulation: ViewEncapsulation.None,
  host: { class: "px-chart-legend", role: "group", "aria-label": "Series" },
  template: `
    @for (entry of entries(); track entry.label) {
      <button type="button" class="px-legend-entry" [attr.aria-pressed]="!entry.hidden" (click)="toggled.emit(entry.label)">
        <span class="px-legend-key" [class.px-legend-line]="entry.shape === 'line'" [style.background]="entry.color"></span>
        <span>{{ entry.label }}</span>
      </button>
    }
  `
})
export class ChartLegendComponent {
  public readonly entries = input.required<readonly LegendEntry[]>();
  public readonly toggled = output<string>();
}

/** The crosshair and its tooltip: values lead, labels follow, each keyed by its series' colour. */
@Component({
  selector: "px-chart-tip",
  changeDetection: ChangeDetectionStrategy.OnPush,
  encapsulation: ViewEncapsulation.None,
  template: `
    @if (reading().crosshair !== null) {
      <div class="px-chart-crosshair" [style.left.px]="reading().crosshair" [style.top.px]="reading().top" [style.height.px]="reading().height"></div>
    }
    <div class="px-chart-tip" role="tooltip" [style.left.px]="reading().left" [style.top.px]="reading().tipTop">
      <div class="px-tip-title">{{ reading().title }}</div>
      @for (row of reading().rows; track $index) {
        <div class="px-tip-row">
          <span class="px-tip-key" [style.background]="row.color ?? 'transparent'"></span>
          <strong>{{ row.value }}</strong>
          <span>{{ row.label }}</span>
        </div>
      }
    </div>
  `
})
export class ChartTipComponent {
  public readonly reading = input.required<PlacedReading>();
}
