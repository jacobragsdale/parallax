/**
 * Paints a chart's draw model on a Canvas 2D context. Every decision about what
 * goes where is in the model; this only draws it, with the theme's tokens for
 * the surface, gridlines and text.
 */

import type { Theme } from "../theme";
import { handles } from "./drawings";
import type { Point, Shape } from "./drawings";
import { plotBottom } from "./model";
import type { ChartModel, Mark, Pane, Rect, ValueAxis } from "./model";

/** The data end of a bar or column is rounded; its baseline stays square. */
const RADIUS = 4;

type Side = "top" | "bottom" | "left" | "right" | "none";

function corners(side: Side, r: number): [number, number, number, number] {
  switch (side) {
    case "top":
      return [r, r, 0, 0];
    case "bottom":
      return [0, 0, r, r];
    case "right":
      return [0, r, r, 0];
    case "left":
      return [r, 0, 0, r];
    case "none":
      return [0, 0, 0, 0];
  }
}

/** Draws one model on one context. */
class Painter {
  constructor(
    private readonly ctx: CanvasRenderingContext2D,
    private readonly theme: Theme
  ) {}

  private rect(rect: Rect, side: Side): void {
    const r = Math.max(0, Math.min(RADIUS, rect.w / 2, rect.h / 2));
    this.ctx.beginPath();
    this.ctx.roundRect(rect.x, rect.y, rect.w, rect.h, corners(side, r));
    this.ctx.fill();
  }

  /** The wash under an area line, the series colour at 10 %, never a saturated block. */
  private area(segment: Float64Array, baseline: number, color: string): void {
    const ctx = this.ctx;
    ctx.globalAlpha = 0.1;
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.moveTo(segment[0] ?? 0, baseline);
    for (let i = 0; i < segment.length; i += 2) {
      ctx.lineTo(segment[i] ?? 0, segment[i + 1] ?? 0);
    }
    ctx.lineTo(segment[segment.length - 2] ?? 0, baseline);
    ctx.closePath();
    ctx.fill();
    ctx.globalAlpha = 1;
  }

  private stroke(segment: Float64Array, color: string, width = 2): void {
    const ctx = this.ctx;
    ctx.strokeStyle = color;
    ctx.lineWidth = width;
    ctx.lineJoin = "round";
    ctx.lineCap = "round";
    ctx.beginPath();
    ctx.moveTo(segment[0] ?? 0, segment[1] ?? 0);
    for (let i = 2; i < segment.length; i += 2) {
      ctx.lineTo(segment[i] ?? 0, segment[i + 1] ?? 0);
    }
    if (segment.length === 2) {
      // A lone point between gaps is drawn as a dot.
      ctx.arc(segment[0] ?? 0, segment[1] ?? 0, 2, 0, 2 * Math.PI);
    }
    ctx.stroke();
  }

  private line(mark: Extract<Mark, { kind: "line" }>): void {
    this.ctx.setLineDash(mark.dashed ? [4, 3] : []);
    for (const segment of mark.segments) {
      if (mark.baseline !== null && segment.length >= 4) {
        this.area(segment, mark.baseline, mark.color);
      }
      if (segment.length >= 2) {
        this.stroke(segment, mark.color, mark.width);
      }
    }
    this.ctx.setLineDash([]);
  }

  /** The wash between two lines: along the upper one and back along the lower one. */
  private band(mark: Extract<Mark, { kind: "band" }>): void {
    const ctx = this.ctx;
    ctx.globalAlpha = 0.1;
    ctx.fillStyle = mark.color;
    ctx.beginPath();
    for (let i = 0; i < mark.upper.length; i += 2) {
      ctx.lineTo(mark.upper[i] ?? 0, mark.upper[i + 1] ?? 0);
    }
    for (let i = mark.lower.length - 2; i >= 0; i -= 2) {
      ctx.lineTo(mark.lower[i] ?? 0, mark.lower[i + 1] ?? 0);
    }
    ctx.closePath();
    ctx.fill();
    ctx.globalAlpha = 1;
  }

  private columns(mark: Extract<Mark, { kind: "columns" }>): void {
    this.ctx.fillStyle = mark.color;
    for (let i = 0; i + 3 < mark.rects.length; i += 4) {
      this.ctx.fillRect(mark.rects[i] ?? 0, mark.rects[i + 1] ?? 0, mark.rects[i + 2] ?? 0, mark.rects[i + 3] ?? 0);
    }
  }

  private candle(mark: Extract<Mark, { kind: "candle" }>): void {
    const ctx = this.ctx;
    ctx.strokeStyle = mark.color;
    ctx.fillStyle = mark.color;
    const x = Math.round(mark.x) + 0.5;
    ctx.lineWidth = mark.bars ? 1.5 : 1;
    ctx.beginPath();
    ctx.moveTo(x, mark.high);
    ctx.lineTo(x, mark.low);
    if (mark.bars) {
      // An OHLC bar: the open ticks left, the close ticks right.
      const half = Math.max(1, mark.width / 2);
      ctx.moveTo(x - half, mark.open);
      ctx.lineTo(x, mark.open);
      ctx.moveTo(x, mark.close);
      ctx.lineTo(x + half, mark.close);
      ctx.stroke();
      return;
    }
    ctx.stroke();
    const body = { x: mark.x - mark.width / 2, y: Math.min(mark.open, mark.close), w: mark.width, h: Math.max(1, Math.abs(mark.close - mark.open)) };
    if (mark.hollow) {
      ctx.fillStyle = this.theme.background;
      ctx.fillRect(body.x, body.y, body.w, body.h);
      ctx.strokeRect(body.x + 0.5, body.y + 0.5, Math.max(0, body.w - 1), Math.max(0, body.h - 1));
    } else {
      ctx.fillRect(body.x, body.y, body.w, body.h);
    }
  }

  private mark(mark: Mark): void {
    switch (mark.kind) {
      case "line":
        this.line(mark);
        return;
      case "rect":
        this.ctx.fillStyle = mark.color;
        this.rect(mark.rect, mark.round);
        return;
      case "cell":
        this.ctx.fillStyle = mark.color;
        this.ctx.fillRect(mark.rect.x, mark.rect.y, mark.rect.w, mark.rect.h);
        return;
      case "candle":
        this.candle(mark);
        return;
      case "band":
        this.band(mark);
        return;
      case "columns":
        this.columns(mark);
        return;
    }
  }

  /** X-axis labels centred on their ticks, skipping any that would touch the one before. */
  private xLabels(model: ChartModel): void {
    const ctx = this.ctx;
    ctx.textAlign = "center";
    ctx.textBaseline = "top";
    let lastRight = Number.NEGATIVE_INFINITY;
    for (const tick of model.xTicks) {
      const half = ctx.measureText(tick.label).width / 2;
      if (tick.px - half < lastRight + 6 || tick.px - half < 0 || tick.px + half > model.width) {
        continue;
      }
      ctx.fillText(tick.label, tick.px, plotBottom(model) + 6);
      lastRight = tick.px + half;
    }
  }

  private horizontal(y: number, from: number, to: number): void {
    const at = Math.round(y) + 0.5;
    this.ctx.beginPath();
    this.ctx.moveTo(from, at);
    this.ctx.lineTo(to, at);
    this.ctx.stroke();
  }

  private axes(model: ChartModel): void {
    const ctx = this.ctx;
    const { plot } = model;
    ctx.font = this.theme.font;
    ctx.fillStyle = this.theme.muted;
    ctx.strokeStyle = this.theme.gridLine;
    ctx.lineWidth = 1;
    // Gridlines follow the left axis only: hairlines, recessive.
    for (const tick of model.yAxes.find((a) => a.side === "left")?.ticks ?? []) {
      this.horizontal(tick.px, plot.x, plot.x + plot.w);
    }
    ctx.textBaseline = "middle";
    for (const yAxis of model.yAxes) {
      this.labels(yAxis, plot);
    }
    ctx.textAlign = "right";
    for (const label of model.yLabels) {
      ctx.fillText(label.label, plot.x - 6, label.px);
    }
    for (const pane of model.panes) {
      this.paneAxes(pane);
    }
    ctx.fillStyle = this.theme.muted;
    ctx.strokeStyle = this.theme.gridLine;
    ctx.textAlign = "center";
    this.xLabels(model);
    this.horizontal(plotBottom(model), plot.x, plot.x + plot.w);
  }

  private labels(yAxis: ValueAxis, plot: Rect): void {
    const ctx = this.ctx;
    ctx.textAlign = yAxis.side === "left" ? "right" : "left";
    const x = yAxis.side === "left" ? plot.x - 6 : plot.x + plot.w + 6;
    for (const tick of yAxis.ticks) {
      ctx.fillText(tick.label, x, tick.px);
    }
  }

  /** A pane's separator, gridlines, guides, value labels and title. */
  private paneAxes(pane: Pane): void {
    const ctx = this.ctx;
    const { plot } = pane;
    ctx.strokeStyle = this.theme.gridLine;
    ctx.lineWidth = 1;
    this.horizontal(plot.y - 5, 0, plot.x + plot.w);
    for (const tick of pane.axis.ticks) {
      this.horizontal(tick.px, plot.x, plot.x + plot.w);
    }
    ctx.setLineDash([3, 3]);
    ctx.strokeStyle = this.theme.muted;
    for (const guide of pane.guides) {
      this.horizontal(guide, plot.x, plot.x + plot.w);
    }
    ctx.setLineDash([]);
    ctx.fillStyle = this.theme.muted;
    ctx.textBaseline = "middle";
    this.labels(pane.axis, plot);
    ctx.textAlign = "left";
    ctx.textBaseline = "top";
    ctx.fillText(pane.title, plot.x + 4, plot.y + 2);
  }

  private clipped(rect: Rect, marks: readonly Mark[]): void {
    const ctx = this.ctx;
    ctx.save();
    ctx.beginPath();
    ctx.rect(rect.x, rect.y - 1, rect.w, rect.h + 2);
    ctx.clip();
    for (const mark of marks) {
      this.mark(mark);
    }
    ctx.restore();
  }

  public paint(model: ChartModel): void {
    const ctx = this.ctx;
    ctx.fillStyle = this.theme.background;
    ctx.fillRect(0, 0, model.width, model.height);
    if (model.empty !== null) {
      ctx.font = this.theme.font;
      ctx.fillStyle = this.theme.muted;
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(model.empty, model.width / 2, model.height / 2);
      return;
    }
    this.axes(model);
    this.clipped(model.plot, model.marks);
    for (const pane of model.panes) {
      this.clipped(pane.plot, pane.marks);
    }
  }
}

/** Drawings over the price, and which is selected. */
export interface DrawingLayer {
  readonly shapes: readonly Shape[];
  readonly selected: number | null;
}

/** Paints drawings in the price plot: in the text colour, the selected one in the selection colour with its handles. */
class DrawingPainter {
  constructor(
    private readonly ctx: CanvasRenderingContext2D,
    private readonly theme: Theme
  ) {}

  private segment(a: Point, b: Point): void {
    this.ctx.beginPath();
    this.ctx.moveTo(a.x, a.y);
    this.ctx.lineTo(b.x, b.y);
    this.ctx.stroke();
  }

  private shape(shape: Shape, color: string): void {
    const ctx = this.ctx;
    ctx.strokeStyle = color;
    ctx.fillStyle = color;
    switch (shape.kind) {
      case "trend":
        this.segment(shape.a, shape.b);
        return;
      case "horizontal":
        this.segment({ x: shape.left, y: shape.y }, { x: shape.right, y: shape.y });
        return;
      case "fibonacci":
        this.fibonacci(shape);
        return;
      case "rectangle":
        this.rectangle(shape.a, shape.b);
        return;
      case "note":
        this.note(shape, color);
        return;
    }
  }

  private fibonacci(shape: Extract<Shape, { kind: "fibonacci" }>): void {
    const ctx = this.ctx;
    const [left, right] = [Math.min(shape.a.x, shape.b.x), Math.max(shape.a.x, shape.b.x)];
    ctx.setLineDash([3, 3]);
    this.segment(shape.a, shape.b);
    ctx.setLineDash([]);
    ctx.textAlign = "left";
    ctx.textBaseline = "bottom";
    for (const level of shape.levels) {
      this.segment({ x: left, y: level.y }, { x: right, y: level.y });
      ctx.fillText(level.label, left + 2, level.y - 1);
    }
  }

  private rectangle(a: Point, b: Point): void {
    const ctx = this.ctx;
    const [x, y, w, h] = [Math.min(a.x, b.x), Math.min(a.y, b.y), Math.abs(b.x - a.x), Math.abs(b.y - a.y)];
    ctx.globalAlpha = 0.08;
    ctx.fillRect(x, y, w, h);
    ctx.globalAlpha = 1;
    ctx.strokeRect(x, y, w, h);
  }

  private note(shape: Extract<Shape, { kind: "note" }>, color: string): void {
    const ctx = this.ctx;
    const { box } = shape;
    ctx.beginPath();
    ctx.arc(shape.at.x, shape.at.y, 3, 0, 2 * Math.PI);
    ctx.fill();
    ctx.fillStyle = this.theme.background;
    ctx.fillRect(box.x, box.y, box.w, box.h);
    ctx.strokeRect(box.x + 0.5, box.y + 0.5, box.w - 1, box.h - 1);
    ctx.fillStyle = this.theme.foreground;
    ctx.textAlign = "left";
    ctx.textBaseline = "middle";
    ctx.fillText(shape.text, box.x + 6, box.y + box.h / 2);
    ctx.fillStyle = color;
  }

  private handles(shape: Shape): void {
    const ctx = this.ctx;
    for (const h of handles(shape)) {
      ctx.fillStyle = this.theme.background;
      ctx.fillRect(h.x - 3.5, h.y - 3.5, 7, 7);
      ctx.strokeRect(h.x - 3.5, h.y - 3.5, 7, 7);
    }
  }

  public paint(model: ChartModel, layer: DrawingLayer): void {
    const ctx = this.ctx;
    ctx.save();
    ctx.beginPath();
    ctx.rect(model.plot.x, model.plot.y, model.plot.w, model.plot.h);
    ctx.clip();
    ctx.font = this.theme.font;
    ctx.lineWidth = 1.5;
    for (const shape of layer.shapes) {
      const selected = shape.index === layer.selected;
      const color = selected ? this.theme.selectionBorder : this.theme.foreground;
      ctx.lineWidth = selected ? 2 : 1.5;
      this.shape(shape, color);
      if (selected) {
        ctx.strokeStyle = color;
        ctx.lineWidth = 1;
        this.handles(shape);
      }
    }
    ctx.restore();
  }
}

/** Paints `model` over the whole canvas, and any drawings over its price. The context's transform must already map CSS pixels. */
export function paintChart(ctx: CanvasRenderingContext2D, model: ChartModel, theme: Theme, drawings: DrawingLayer = { shapes: [], selected: null }): void {
  new Painter(ctx, theme).paint(model);
  if (drawings.shapes.length > 0 && model.empty === null) {
    new DrawingPainter(ctx, theme).paint(model, drawings);
  }
}
