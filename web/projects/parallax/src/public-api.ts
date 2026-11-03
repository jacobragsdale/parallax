/*
 * Public API Surface of @jacobragsdale/parallax
 */

export { provideParallax, ParallaxClient } from "./lib/client";
export type { ParallaxConfig } from "./lib/client";
export { Connection, ViewHandle } from "./lib/connection";
export type { Transport, ViewListener } from "./lib/connection";
export { CellFlag, decodeFrame, decimalFromWords, FrameError } from "./lib/frame";
export type { CellValue, FrameColumn, FrameData, GroupKey, TreeRows } from "./lib/frame";
export { exactDecimal, pathValue } from "./lib/format";
export { readTheme } from "./lib/theme";
export type { Theme } from "./lib/theme";
export { CanvasGridComponent } from "./lib/grid/canvas-grid.component";
export type { DrawStats } from "./lib/grid/canvas-grid.component";
export { DomGridComponent } from "./lib/grid/dom-grid.component";
