/*
 * Public API Surface of @jacobragsdale/parallax
 */

export { provideParallax, ParallaxClient } from "./lib/client";
export type { ParallaxConfig } from "./lib/client";
export { Connection, ViewHandle } from "./lib/connection";
export type { Transport, Viewport, ViewListener } from "./lib/connection";
export { CellFlag, decodeFrame, decimalFromWords, FrameError } from "./lib/frame";
export type { CellValue, FrameColumn, FrameData, GroupKey, TreeRows } from "./lib/frame";
export { exactDecimal, isNegativeText, pathValue } from "./lib/format";
export { AGGREGATES, FILTER_OPS, LAYOUT_VERSION, SHOW_AS, TREE_WIDTH_KEY, layoutSchema, measureWidthKey, parseLayout } from "./lib/layout";
export type { Layout, LayoutFilter, LayoutFormat, LayoutInput, LayoutMeasure, LayoutScalar } from "./lib/layout";
export { THEMES, TOKENS, readTheme } from "./lib/theme";
export type { Theme, ThemeName } from "./lib/theme";
export { GridComponent } from "./lib/grid/grid.component";
export type { Density, DrawStats } from "./lib/grid/grid.component";
