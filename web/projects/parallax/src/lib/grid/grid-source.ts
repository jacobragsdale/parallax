/**
 * The data side shared by both grids: the open view, the latest frame, and the
 * mapping from scroll position to rows. The two grids differ only in how they draw.
 */

import { DestroyRef, effect, inject, signal, untracked } from "@angular/core";
import type { InputSignal, Signal } from "@angular/core";
import { z } from "zod";

import { ParallaxClient } from "../client";
import type { FrameData } from "../frame";
import type { ViewHandle } from "../connection";

/** Browsers cap element heights near 33 million pixels; stay well below. */
const MAX_SCROLL_PX = 15_000_000;

/** The fields of a layout the grid itself needs; the engine validates the rest. */
const layoutFields = z.looseObject({ group_by: z.array(z.string()).optional() });

function parseJson(text: string): unknown {
  try {
    const value: unknown = JSON.parse(text);
    return value;
  } catch {
    return null;
  }
}

/** The grid's view of a source: what to draw and how to ask for more. */
export interface GridSource {
  readonly frame: Signal<FrameData | null>;
  readonly error: Signal<string | null>;
  readonly groupBy: Signal<readonly string[]>;
  /** Asks for rows `first..first + count`, with a margin on both sides. */
  readonly request: (first: number, count: number) => void;
  readonly expand: (path: readonly (string | number | boolean | null)[], expanded: boolean) => void;
}

/** Opens a view for a grid component and follows its inputs. Call in an injection context. */
export function gridSource(dataset: InputSignal<string>, layout: InputSignal<string>): GridSource {
  const client = inject(ParallaxClient);
  const frame = signal<FrameData | null>(null);
  const error = signal<string | null>(null);
  const groupBy = signal<readonly string[]>([]);
  let view: ViewHandle | null = null;
  let openedFor = "";
  let wanted: { readonly first: number; readonly count: number } | null = null;

  const margin = 40;
  const request = (first: number, count: number): void => {
    wanted = { first, count };
    view?.setViewport(Math.max(0, first - margin), count + 2 * margin);
  };

  effect(() => {
    const name = dataset();
    const json = layout();
    const parsed = layoutFields.safeParse(parseJson(json));
    groupBy.set(parsed.success ? (parsed.data.group_by ?? []) : []);
    untracked(() => {
      if (view !== null && openedFor === name) {
        view.update(json);
        return;
      }
      view?.close();
      openedFor = name;
      client.connection().then(
        (connection) => {
          view = connection.open(name, json, {
            frame: (f) => {
              error.set(null);
              frame.set(f);
            },
            error: (message) => {
              error.set(message);
            }
          });
          if (wanted !== null) {
            request(wanted.first, wanted.count);
          }
        },
        (failure: unknown) => {
          error.set(failure instanceof Error ? failure.message : String(failure));
        }
      );
    });
  });

  inject(DestroyRef).onDestroy(() => {
    view?.close();
  });

  return {
    frame,
    error,
    groupBy,
    request,
    expand: (path, expanded) => {
      view?.setExpanded(path, expanded);
    }
  };
}

/** Maps a scroll offset to the first visible row, scaling when there are too many rows to scroll natively. */
export interface ScrollGeometry {
  readonly spacerPx: number;
  readonly firstRow: (scrollTop: number, viewportPx: number) => number;
  /** Where row `firstRow` starts relative to the viewport's top edge, in pixels (zero or negative). */
  readonly offset: (scrollTop: number, viewportPx: number) => number;
}

/** Computes scroll geometry for `rows` rows of `rowPx` pixels. */
export function scrollGeometry(rows: number, rowPx: number): ScrollGeometry {
  const natural = rows * rowPx;
  if (natural <= MAX_SCROLL_PX) {
    return { spacerPx: natural, firstRow: (top) => Math.floor(top / rowPx), offset: (top) => -(top % rowPx) };
  }
  return {
    spacerPx: MAX_SCROLL_PX,
    firstRow: (top, viewport) => {
      const fraction = top / Math.max(1, MAX_SCROLL_PX - viewport);
      return Math.min(rows - 1, Math.floor(fraction * Math.max(0, rows - viewport / rowPx)));
    },
    offset: () => 0
  };
}
