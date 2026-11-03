/**
 * The line under a grid that says where its numbers come from: the dataset, the
 * rows shown, when the source was read and how long ago, values the load changed,
 * and a failed refresh. Stale data is wrong data, so the age is always visible.
 */

import { ChangeDetectionStrategy, Component, DestroyRef, computed, inject, input, output, signal } from "@angular/core";

import type { TableStatus } from "../proto/parallax_pb";
import type { ViewPlacement } from "./grid-source";

/** The zone the status bar shows times in. */
const ZONE = "America/New_York";

const TIME = new Intl.DateTimeFormat("en-US", { timeZone: ZONE, year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23" });

/** Formats microseconds since the epoch as `YYYY-MM-DD HH:MM:SS` in New York. */
export function statusTime(micros: bigint): string {
  const parts = new Map(TIME.formatToParts(new Date(Number(micros / 1000n))).map((p) => [p.type, p.value]));
  const part = (type: Intl.DateTimeFormatPartTypes): string => parts.get(type) ?? "";
  return `${part("year")}-${part("month")}-${part("day")} ${part("hour")}:${part("minute")}:${part("second")}`;
}

/** Describes an age in whole units, such as `4 min ago`. */
export function ageText(ms: number): string {
  const seconds = Math.max(0, Math.floor(ms / 1000));
  if (seconds < 60) {
    return "just now";
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${String(minutes)} min ago`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 48) {
    return `${String(hours)} h ago`;
  }
  return `${String(Math.floor(hours / 24))} days ago`;
}

const GROUPED = new Intl.NumberFormat("en-US");

/** A short note beside the status, with its explanation on hover. */
interface Badge {
  readonly kind: string;
  readonly text: string;
  readonly classes: string;
  readonly role: string | null;
  readonly title: string;
}

@Component({
  selector: "px-status-bar",
  changeDetection: ChangeDetectionStrategy.OnPush,
  template: `
    <span class="px-status-dataset">{{ dataset() }}</span>
    <span class="px-status-rows">{{ rows() }}</span>
    @if (asOf(); as a) {
      <span class="px-status-age" [title]="a.title">as of {{ a.time }} · {{ a.age }}</span>
    }
    @for (b of badges(); track b.kind) {
      <span [class]="b.classes" [attr.role]="b.role" [title]="b.title">{{ b.text }}</span>
    }
    <span class="px-status-message" role="status">{{ message() }}</span>
    <button type="button" class="px-status-export" (click)="exported.emit('xlsx')">Export to Excel</button>
    <button type="button" class="px-status-export" (click)="exported.emit('csv')">CSV</button>
    <button type="button" class="px-status-info" [attr.aria-expanded]="open()" aria-label="About this data" (click)="open.set(!open())">i</button>
    @if (open()) {
      <dl class="px-info" role="dialog" aria-label="About this data">
        @for (item of details(); track item.term) {
          <dt>{{ item.term }}</dt>
          <dd>{{ item.value }}</dd>
        }
      </dl>
    }
  `
})
export class StatusBarComponent {
  /** The dataset shown. */
  public readonly dataset = input.required<string>();
  /** Its parameters. */
  public readonly parameters = input<Readonly<Record<string, string>>>({});
  /** Rows in the view. */
  public readonly viewRows = input<number | null>(null);
  /** When the source was read, in microseconds since the epoch. */
  public readonly sourceAsOf = input<bigint | null>(null);
  /** The table's status from the engine. */
  public readonly status = input<TableStatus | null>(null);
  /** Where the view runs. */
  public readonly placement = input<ViewPlacement | null>(null);
  /** A short note, such as the result of a copy. */
  public readonly message = input<string>("");
  /** Fires when the user asks for an export, with its format. */
  public readonly exported = output<"xlsx" | "csv">();

  protected readonly open = signal(false);
  private readonly now = signal(Date.now());

  protected readonly rows = computed(() => {
    const rows = this.viewRows();
    return rows === null ? "loading…" : `${GROUPED.format(rows)} rows`;
  });

  protected readonly asOf = computed(() => {
    // Zero means the source gave no time; fall back to when the table loaded.
    const source = this.sourceAsOf();
    const micros = source !== null && source !== 0n ? source : (this.status()?.loadedAtMicros ?? null);
    if (micros === null || micros === 0n) {
      return null;
    }
    return { time: statusTime(micros), age: ageText(this.now() - Number(micros / 1000n)), title: `Source read at ${statusTime(micros)} New York time` };
  });

  private readonly nans = computed(() => {
    const columns = this.status()?.nansToNull ?? [];
    const total = columns.reduce((sum, c) => sum + c.count, 0n);
    if (total === 0n) {
      return null;
    }
    const detail = columns.map((c) => `${c.column}: ${GROUPED.format(c.count)}`).join(", ");
    return { text: `${GROUPED.format(total)} NaN shown blank`, title: `NaN values became blanks when the data loaded. ${detail}` };
  });

  private readonly failed = computed(() => {
    const status = this.status();
    if (status === null || status.refreshError === "") {
      return null;
    }
    const at = status.refreshFailedAtMicros === 0n ? "" : ` at ${statusTime(status.refreshFailedAtMicros)}`;
    return { text: `refresh failed${at}`, title: `${status.refreshError}. The grid still shows the last good load.` };
  });

  /** A badge when the view runs in the browser, or runs on the server because its copy did not load. */
  private readonly where = computed((): Omit<Badge, "kind"> | null => {
    const placement = this.placement();
    if (placement?.location === "browser") {
      return { text: "in browser", classes: "px-badge px-status-where", role: null, title: "This view runs in your browser, on a copy of the server's table, with the same engine." };
    }
    if (placement !== null && placement.fallback !== null) {
      return { text: "on server", classes: "px-badge px-badge-warning px-status-where", role: null, title: placement.fallback };
    }
    return null;
  });

  /** The badges after the age: blanked NaNs, a failed refresh, and where the view runs. */
  protected readonly badges = computed((): readonly Badge[] => {
    const nans = this.nans();
    const failed = this.failed();
    const where = this.where();
    return [
      ...(nans === null ? [] : [{ kind: "nans", text: nans.text, classes: "px-badge px-badge-warning", role: null, title: nans.title }]),
      ...(failed === null ? [] : [{ kind: "failed", text: failed.text, classes: "px-badge px-badge-danger", role: "status", title: failed.title }]),
      ...(where === null ? [] : [{ kind: "where", ...where }])
    ];
  });

  protected readonly details = computed(() => {
    const status = this.status();
    const parameters = Object.entries(this.parameters())
      .map(([k, v]) => `${k} = ${v}`)
      .join(", ");
    const items = [
      { term: "Dataset", value: this.dataset() },
      { term: "Parameters", value: parameters === "" ? "none" : parameters },
      { term: "Rows in view", value: this.rows() },
      { term: "Rows in table", value: status === null ? "unknown" : GROUPED.format(status.rows) },
      { term: "Load", value: status === null ? "unknown" : `#${status.loadVersion.toString()}` },
      { term: "Loaded at", value: status === null || status.loadedAtMicros === 0n ? "unknown" : `${statusTime(status.loadedAtMicros)} New York` },
      { term: "Source read at", value: this.asOf()?.time ?? "unknown" }
    ];
    if (status !== null && status.refreshError !== "") {
      items.push({ term: "Last refresh", value: status.refreshError });
    }
    const placement = this.placement();
    if (placement !== null) {
      items.push({ term: "Runs", value: placement.location === "browser" ? "in this browser, on a copy of the server's table" : (placement.fallback ?? "on the server") });
    }
    return items;
  });

  constructor() {
    const timer = setInterval(() => {
      this.now.set(Date.now());
    }, 15_000);
    inject(DestroyRef).onDestroy(() => {
      clearInterval(timer);
    });
  }
}
