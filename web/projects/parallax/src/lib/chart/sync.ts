/** Shares the pointer's x between charts, so their crosshairs and tooltips move together. */

import { Injectable, signal } from "@angular/core";

/** Where the pointer is, in axis positions, over a chart of a group. */
export interface SyncedPointer {
  readonly group: string;
  readonly x: number;
  /** The chart the pointer is over, which draws its own reading. */
  readonly source: object;
}

/** The pointer shared by charts of the same group. */
@Injectable({ providedIn: "root" })
export class ChartSync {
  public readonly pointer = signal<SyncedPointer | null>(null);
}
