/*
 * Public API Surface of @jacobragsdale/parallax
 */

export { copyUrlFor, exportUrlFor, httpBaseFor, placeView, provideParallax, ParallaxClient } from "./lib/client";
export type { CopyingEngine, ParallaxConfig, Placement } from "./lib/client";
export { BrowserEngine, COPY_TIMEOUT_MS } from "./lib/browser-engine";
export type { WorkerPort } from "./lib/browser-engine";
export { Connection, ViewHandle } from "./lib/connection";
export type { ChartWindow, DistinctValues, EngineLocation, Transport, Viewport, ViewListener } from "./lib/connection";
export { decodeChartFrame } from "./lib/chart-frame";
export type { CandleData, ChartContent, ChartFrameData, ChartTickData, HistogramSeries, IndicatorLineData, SeriesData } from "./lib/chart-frame";
export { CellFlag, decodeFrame, decimalFromWords, FrameError } from "./lib/frame";
export type { CellValue, DistinctValue, FrameColumn, FrameData, GroupKey, TreeRows } from "./lib/frame";
export { exactDecimal, isNegativeText, pathValue } from "./lib/format";
export { AGGREGATES, CHART_KINDS, FILTER_OPS, INDICATOR_KINDS, LAYOUT_VERSION, SHOW_AS, TREE_WIDTH_KEY, checkLayout, layoutSchema, measureWidthKey, parseLayout } from "./lib/layout";
export type { Layout, LayoutChart, LayoutCheck, LayoutFilter, LayoutFormat, LayoutIndicator, LayoutInput, LayoutMeasure, LayoutScalar } from "./lib/layout";
export { CHART_COLORS, THEMES, TOKENS, readChartColors, readTheme } from "./lib/theme";
export type { ChartColors, Theme, ThemeName } from "./lib/theme";
export { GridComponent } from "./lib/grid/grid.component";
export type { Density, DrawStats, DrillRequest } from "./lib/grid/grid.component";
export type { FieldKind, SourceField, ViewPlacement } from "./lib/grid/grid-source";
export { PivotComponent } from "./lib/pivot/pivot.component";
export { ChartComponent } from "./lib/chart/chart.component";
export { chartModel } from "./lib/chart/model";
export type { CategoryData, ChartModel, LegendEntry, Mark } from "./lib/chart/model";
export { drillLayout } from "./lib/pivot/edit";
export { decodeLayout, encodeLayout } from "./lib/share";
