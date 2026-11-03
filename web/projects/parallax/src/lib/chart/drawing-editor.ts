/**
 * Making and editing a chart's drawings with the pointer. With a tool in hand, a
 * press anchors a drawing and a drag stretches it to its second anchor; a
 * horizontal line takes one press, and a note asks for its text. Without one, a
 * press on a drawing selects it, a drag of one of its handles moves that anchor,
 * and Delete removes it. Every anchor snaps to the data, and a change reaches the
 * layout only when the pointer is let go, as one edit.
 */

import { computed, signal } from "@angular/core";
import type { Signal } from "@angular/core";

import type { ChartFrameData } from "../chart-frame";
import type { LayoutDrawing } from "../layout";
import { drawingAt, drawingFrom, handleAt, moved, shapesOf, snap } from "./drawings";
import type { DrawingTool, Point, Shape, Snap } from "./drawings";
import type { ChartModel } from "./model";

/** What the editor reads from its chart, and how it saves. */
export interface DrawingHost {
  readonly model: Signal<ChartModel | null>;
  readonly frame: Signal<ChartFrameData | null>;
  /** The layout's drawings. */
  readonly drawings: Signal<readonly LayoutDrawing[]>;
  readonly save: (drawings: readonly LayoutDrawing[]) => void;
  /** Measures a label's width in the chart font. */
  readonly measure: (text: string) => number;
  /** Series hidden from the plot, by their legend labels, which anchors never snap to. */
  readonly hidden: Signal<readonly string[]>;
}

/** A drawing being made: its tool and its two anchors so far. */
interface Sketch {
  readonly tool: DrawingTool;
  readonly from: Snap;
  readonly to: Snap;
}

/** A drawing's anchor being dragged: the drawing, as it was when grabbed, the anchor and where it is now. */
interface Move {
  readonly index: number;
  readonly key: string;
  readonly anchor: number;
  readonly to: Snap | null;
}

/** A drawing chosen by its place in the layout and what it was there, so that a layout changed since cannot redirect it. */
interface Choice {
  readonly index: number;
  readonly key: string;
}

/** What a drawing is, to tell whether the drawing at a place is still the one chosen. */
function keyOf(drawing: LayoutDrawing): string {
  return JSON.stringify(drawing);
}

/** The fewest pixels between a drawing's anchors: a press without a drag makes nothing. */
const SMALLEST = 4;

export class DrawingEditor {
  /** The tool in hand, or null. */
  public readonly tool = signal<DrawingTool | null>(null);
  private readonly choice = signal<Choice | null>(null);
  /** The selected drawing, by its place in the layout, or null; none once the layout no longer holds it there. */
  public readonly selected = computed((): number | null => {
    const choice = this.choice();
    return choice !== null && this.still(choice) ? choice.index : null;
  });
  /** Where a note being placed is anchored, while its text is typed. */
  public readonly note = signal<Snap | null>(null);
  private readonly sketch = signal<Sketch | null>(null);
  private readonly moving = signal<Move | null>(null);

  /** The drawings as drawn now: a drawing being made or moved included. */
  public readonly shown = computed((): readonly LayoutDrawing[] => {
    const drawings = [...this.host.drawings()];
    const move = this.moving();
    const to = move?.to ?? null;
    const target = move === null ? undefined : drawings[move.index];
    if (move !== null && to !== null && target !== undefined) {
      drawings[move.index] = moved(target, move.anchor, to);
    }
    const sketch = this.sketch();
    if (sketch !== null) {
      drawings.push(drawingFrom(sketch.tool, sketch.from, sketch.to));
    }
    return drawings;
  });

  /** The drawings in pixels. */
  public readonly shapes = computed((): readonly Shape[] => {
    const [model, frame] = [this.host.model(), this.host.frame()];
    return model === null || frame === null ? [] : shapesOf(this.shown(), frame, model, this.host.measure);
  });

  constructor(private readonly host: DrawingHost) {}

  /** Whether the layout still holds the chosen drawing at its place. */
  private still(choice: Choice): boolean {
    const drawing = this.host.drawings()[choice.index];
    return drawing !== undefined && keyOf(drawing) === choice.key;
  }

  private select(index: number | null): void {
    const drawing = index === null ? undefined : this.host.drawings()[index];
    this.choice.set(index === null || drawing === undefined ? null : { index, key: keyOf(drawing) });
  }

  /** An anchor at `at`, held within the price's plot: drawings go over the price, not its panes. */
  private snapAt(at: Point): Snap | null {
    const [model, frame] = [this.host.model(), this.host.frame()];
    if (model === null || frame === null) {
      return null;
    }
    const { plot } = model;
    return snap(frame, model, { x: at.x, y: Math.min(plot.y + plot.h, Math.max(plot.y, at.y)) }, this.host.hidden());
  }

  /** Whether `at` is on the price's plot, where drawings are made. */
  private onPrice(at: Point): boolean {
    const plot = this.host.model()?.plot;
    return plot !== undefined && at.y >= plot.y && at.y <= plot.y + plot.h;
  }

  /** Picks up a tool, or puts it down with null. */
  public pick(tool: DrawingTool | null): void {
    this.tool.set(tool);
    this.select(null);
  }

  /** A press on the plot: true if it made, picked or grabbed a drawing, and so is not a pan or a brush. */
  public down(at: Point): boolean {
    const tool = this.tool();
    if (tool !== null && this.onPrice(at)) {
      this.start(tool, at);
      return true;
    }
    const shapes = this.shapes();
    const selected = shapes.find((s) => s.index === this.selected());
    const handle = selected === undefined ? null : grab(selected, at);
    const grabbed = selected === undefined ? undefined : this.host.drawings()[selected.index];
    if (selected !== undefined && handle !== null && grabbed !== undefined) {
      this.moving.set({ index: selected.index, key: keyOf(grabbed), anchor: handle, to: null });
      return true;
    }
    const hit = drawingAt(shapes, at);
    this.select(hit);
    return hit !== null;
  }

  private start(tool: DrawingTool, at: Point): void {
    const anchor = this.snapAt(at);
    if (anchor === null) {
      return;
    }
    if (tool === "horizontal") {
      this.add(drawingFrom(tool, anchor, anchor));
    } else if (tool === "note") {
      // The tool is put down while the text is typed, so a press elsewhere only leaves the box.
      this.tool.set(null);
      this.note.set(anchor);
    } else {
      this.sketch.set({ tool, from: anchor, to: anchor });
    }
  }

  /** A move of the pointer: true while it stretches or moves a drawing. */
  public move(at: Point): boolean {
    const [sketch, move] = [this.sketch(), this.moving()];
    if (sketch === null && move === null) {
      return false;
    }
    const to = this.snapAt(at);
    if (to !== null && sketch !== null) {
      this.sketch.set({ ...sketch, to });
    }
    if (to !== null && move !== null) {
      this.moving.set({ ...move, to });
    }
    return true;
  }

  /** The pointer let go: true if it finished making or moving a drawing, which then reaches the layout. */
  public up(): boolean {
    const [sketch, move] = [this.sketch(), this.moving()];
    this.sketch.set(null);
    this.moving.set(null);
    if (sketch !== null) {
      const { from, to } = sketch;
      if (Math.hypot(to.point.x - from.point.x, to.point.y - from.point.y) >= SMALLEST) {
        this.add(drawingFrom(sketch.tool, from, to));
      }
      return true;
    }
    const to = move?.to ?? null;
    const target = move === null ? undefined : this.host.drawings()[move.index];
    // Saved only if the layout still holds the drawing grabbed, where it was.
    if (move !== null && to !== null && target !== undefined && keyOf(target) === move.key) {
      const changed = moved(target, move.anchor, to);
      this.host.save(this.host.drawings().map((d, i) => (i === move.index ? changed : d)));
      this.select(move.index);
    }
    return move !== null;
  }

  private add(drawing: LayoutDrawing): void {
    this.host.save([...this.host.drawings(), drawing]);
    this.tool.set(null);
    this.select(this.host.drawings().length - 1);
  }

  /** A note's text typed, or null if it was dropped. */
  public noteDone(text: string | null): void {
    const at = this.note();
    this.note.set(null);
    if (at !== null && text !== null) {
      this.add(drawingFrom("note", at, at, text));
    }
  }

  /** Removes the selected drawing, dropping any drag in progress: true if there was one. */
  public remove(): boolean {
    const selected = this.selected();
    this.abandon();
    if (selected === null) {
      return false;
    }
    this.select(null);
    this.host.save(this.host.drawings().filter((_, i) => i !== selected));
    return true;
  }

  /** Drops a drawing being made or moved, as when the browser cancels the pointer. */
  public abandon(): void {
    this.sketch.set(null);
    this.moving.set(null);
  }

  /** Escape: puts down the tool, drops a drawing in progress and the selection. True if there was any. */
  public cancel(): boolean {
    const busy = this.tool() !== null || this.sketch() !== null || this.moving() !== null || this.selected() !== null;
    this.tool.set(null);
    this.abandon();
    this.select(null);
    return busy;
  }
}

/** The anchor of a selected drawing under `at`: a handle, or anywhere along a horizontal line. */
function grab(shape: Shape, at: Point): number | null {
  if (shape.kind === "horizontal") {
    return Math.abs(at.y - shape.y) <= 6 ? 0 : null;
  }
  return handleAt(shape, at);
}
