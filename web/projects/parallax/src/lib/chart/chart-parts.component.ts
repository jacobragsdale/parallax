/** Pieces of `<px-chart>` that sit over or beside its canvas: the legend, and the crosshair with its tooltip. */

import { ChangeDetectionStrategy, Component, ViewEncapsulation, afterNextRender, input, output, viewChild } from "@angular/core";
import type { ElementRef } from "@angular/core";

import { MAX_NOTE } from "../layout";
import type { DrawingTool, Point } from "./drawings";
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

/** The drawing tools, as their buttons name them. */
const TOOLS: readonly { readonly tool: DrawingTool; readonly label: string }[] = [
  { tool: "trend", label: "Trend line" },
  { tool: "horizontal", label: "Horizontal line" },
  { tool: "fibonacci", label: "Fibonacci" },
  { tool: "rectangle", label: "Rectangle" },
  { tool: "note", label: "Note" }
];

/** The drawing tools: a button per kind of drawing, pressed while it is the one in hand, and one to delete the selected drawing. */
@Component({
  selector: "px-chart-tools",
  changeDetection: ChangeDetectionStrategy.OnPush,
  encapsulation: ViewEncapsulation.None,
  host: { class: "px-chart-tools", role: "toolbar", "aria-label": "Drawing tools" },
  template: `
    @for (t of tools; track t.tool) {
      <button type="button" class="px-tool" [attr.aria-pressed]="tool() === t.tool" (click)="picked.emit(tool() === t.tool ? null : t.tool)">{{ t.label }}</button>
    }
    <button type="button" class="px-tool" [disabled]="!selected()" (click)="removed.emit()">Delete drawing</button>
  `
})
export class ChartToolsComponent {
  public readonly tool = input.required<DrawingTool | null>();
  public readonly selected = input.required<boolean>();
  public readonly picked = output<DrawingTool | null>();
  public readonly removed = output();
  protected readonly tools = TOOLS;
}

/** The text box of a note being placed: Enter keeps it, Escape drops it, and leaving it keeps any text. */
@Component({
  selector: "px-chart-note",
  changeDetection: ChangeDetectionStrategy.OnPush,
  encapsulation: ViewEncapsulation.None,
  template: `<input
    #box
    class="px-chart-note-input"
    type="text"
    aria-label="Note text"
    [attr.maxlength]="max"
    [style.left.px]="at().x + 8"
    [style.top.px]="at().y - 11"
    (keydown)="key($event)"
    (blur)="leave()"
  />`
})
export class ChartNoteComponent {
  public readonly at = input.required<Point>();
  public readonly done = output<string | null>();
  protected readonly max = MAX_NOTE;
  private readonly box = viewChild.required<ElementRef<HTMLInputElement>>("box");
  private finished = false;

  constructor() {
    afterNextRender(() => {
      this.box().nativeElement.focus();
    });
  }

  private finish(text: string | null): void {
    if (!this.finished) {
      this.finished = true;
      this.done.emit(text === null || text.trim() === "" ? null : text.trim());
    }
  }

  protected key(event: KeyboardEvent): void {
    // The plot's keys must not act on what is typed here.
    event.stopPropagation();
    if (event.key === "Enter") {
      this.finish(this.box().nativeElement.value);
    } else if (event.key === "Escape") {
      this.finish(null);
    }
  }

  protected leave(): void {
    this.finish(this.box().nativeElement.value);
  }
}

/** The chart's head: its legend, and the drawing tools on charts that take drawings. */
@Component({
  selector: "px-chart-head",
  changeDetection: ChangeDetectionStrategy.OnPush,
  encapsulation: ViewEncapsulation.None,
  imports: [ChartLegendComponent, ChartToolsComponent],
  host: { class: "px-chart-head" },
  template: `
    @if (entries().length > 0) {
      <px-chart-legend [entries]="entries()" (toggled)="toggled.emit($event)" />
    }
    @if (drawable()) {
      <px-chart-tools [tool]="tool()" [selected]="selected()" (picked)="picked.emit($event)" (removed)="removed.emit()" />
    }
  `
})
export class ChartHeadComponent {
  public readonly entries = input.required<readonly LegendEntry[]>();
  public readonly drawable = input.required<boolean>();
  public readonly tool = input.required<DrawingTool | null>();
  public readonly selected = input.required<boolean>();
  public readonly toggled = output<string>();
  public readonly picked = output<DrawingTool | null>();
  public readonly removed = output();
}
