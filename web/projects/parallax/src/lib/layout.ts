/**
 * Layouts: the JSON document that describes a view.
 *
 * The Rust types in `parallax-protocol` are the source of truth; their JSON Schema
 * is committed as `schema/layout.schema.json`. This schema mirrors them so that a
 * layout from storage or a URL is checked before use. A test reads layouts the Rust
 * crate wrote and requires each to come back unchanged, so the two cannot drift.
 */

import * as z from "zod/mini";
import { en } from "zod/locales";

/**
 * English messages for the library's own parses. Zod's tree-shakable build has none until given a locale, and
 * setting one globally would override whatever the host application chose.
 */
const ENGLISH = { error: en().localeError };

/** The layout version this library writes. */
export const LAYOUT_VERSION = 1;

const name = z.string();
const count = z.int().check(z.nonnegative());
/** A JSON scalar: filter operands and group-path values. */
const scalar = z.union([z.string(), z.number(), z.boolean(), z.null()]);

/** `schema`, or null when absent. */
function orNull<T extends z.core.SomeType>(schema: T): z.ZodMiniDefault<z.ZodMiniNullable<T>> {
  return z._default(z.nullable(schema), null);
}

const formatSchema = z.strictObject({
  decimals: orNull(z.int().check(z.minimum(0), z.maximum(30))),
  thousands: orNull(z.boolean()),
  percent: z._default(z.boolean(), false),
  abbreviate: orNull(z.enum(["thousands", "millions", "billions"])),
  negative: z._default(z.enum(["minus", "parentheses"]), "minus"),
  zone: orNull(z.string()),
  precision: z._default(z.enum(["seconds", "milliseconds", "microseconds"]), "seconds")
});

/** The aggregates a measure can use. */
export const AGGREGATES = ["count", "count_all", "sum", "mean", "weighted_mean", "min", "max", "first", "last", "distinct_count", "variance", "std_dev", "median", "percentile"] as const;

/** How a measure can be shown. */
export const SHOW_AS = ["value", "percent_of_grand_total", "percent_of_column_total", "percent_of_row_total", "percent_of_parent"] as const;

/** The filter operators. */
export const FILTER_OPS = ["eq", "ne", "lt", "le", "gt", "ge", "in", "not_in", "is_null", "is_not_null", "contains", "starts_with"] as const;

const comparisons = ["gt", "ge", "lt", "le", "eq", "ne"] as const;

const measureSchema = z.strictObject({
  column: orNull(name),
  aggregate: z.enum(AGGREGATES),
  percentile: orNull(z.number().check(z.minimum(0), z.maximum(1))),
  weight: orNull(name),
  unit_column: orNull(name),
  show_as: z._default(z.enum(SHOW_AS), "value"),
  format: orNull(formatSchema)
});

const filterSchema = z.strictObject({ column: name, op: z.enum(FILTER_OPS), value: z._default(scalar, null), values: z._default(z.array(scalar), []) });

const sortSchema = z.strictObject({ column: name, descending: z._default(z.boolean(), false) });

const groupSortSchema = z.strictObject({ by: z._default(z.union([z.literal("key"), z.strictObject({ measure: count })]), "key"), descending: z._default(z.boolean(), false) });

const havingSchema = z.strictObject({ measure: count, op: z.enum(comparisons), value: z.number() });

const topNSchema = z.strictObject({ n: count, measure: count, largest: z._default(z.boolean(), true), other: z._default(z.boolean(), true) });

const expandedSchema = z.strictObject({ path: z.array(scalar), expanded: z.boolean() });

/** What a chart draws. Bar charts and heatmaps draw the layout's grouped view; the engine reduces the others. */
export const CHART_KINDS = ["line", "area", "bar", "scatter", "heatmap", "histogram", "candlestick", "ohlc"] as const;

const chartSchema = z.strictObject({
  kind: z.enum(CHART_KINDS),
  x: z._default(z.string(), ""),
  y: z._default(z.array(name), []),
  y_right: z._default(z.array(name), []),
  series: orNull(name),
  ohlc: orNull(z.strictObject({ open: name, high: name, low: name, close: name })),
  axis: z._default(z.enum(["value", "trading"]), "value"),
  log_y: z._default(z.boolean(), false),
  stacked: z._default(z.boolean(), false),
  horizontal: z._default(z.boolean(), false),
  range: orNull(z.strictObject({ from: z.number(), to: z.number() })),
  hidden: z._default(z.array(z.string()), [])
});

/** The schema of a layout document. Unknown fields are errors, as in Rust. */
export const layoutSchema = z.strictObject({
  // Older versions are read and brought up to date, as the Rust crate does.
  version: z.pipe(
    z.optional(z.int().check(z.minimum(0), z.maximum(LAYOUT_VERSION))),
    z.transform((): typeof LAYOUT_VERSION => LAYOUT_VERSION)
  ),
  dataset: z._default(z.string(), ""),
  parameters: z._default(z.record(z.string(), z.string()), {}),
  columns: z._default(z.array(name), []),
  group_by: z._default(z.array(name), []),
  split_by: z._default(z.array(name), []),
  measures: z._default(z.array(measureSchema), []),
  filters: z._default(z.array(filterSchema), []),
  sort: z._default(z.array(sortSchema), []),
  group_sort: z._default(groupSortSchema, { by: "key", descending: false }),
  having: z._default(z.array(havingSchema), []),
  top_n: orNull(topNSchema),
  expand_depth: z._default(count, 0),
  expanded: z._default(z.array(expandedSchema), []),
  formats: z._default(z.record(name, formatSchema), {}),
  column_widths: z._default(z.record(z.string(), z.int().check(z.minimum(0), z.maximum(4_294_967_295))), {}),
  chart: orNull(chartSchema)
});

/** A complete layout, with every default filled in. */
export type Layout = z.output<typeof layoutSchema>;
/** A layout as written by hand: every field may be left out. */
export type LayoutInput = z.input<typeof layoutSchema>;
/** One measure of a layout. */
export type LayoutMeasure = z.output<typeof measureSchema>;
/** One filter of a layout. */
export type LayoutFilter = z.output<typeof filterSchema>;
/** A chart of a layout. */
export type LayoutChart = z.output<typeof chartSchema>;
/** A display format. */
export type LayoutFormat = z.output<typeof formatSchema>;
/** A JSON scalar in a filter or group path. */
export type LayoutScalar = z.output<typeof scalar>;

/** Reads and checks a layout from any source, filling in defaults. Throws a Zod error with English messages. */
export function parseLayout(value: unknown): Layout {
  return layoutSchema.parse(value, ENGLISH);
}

/** The result of checking a layout: the layout with its defaults, or what is wrong with it, in English. */
export type LayoutCheck = { readonly ok: true; readonly layout: Layout } | { readonly ok: false; readonly error: string };

/** Checks a layout from any source, filling in defaults, and describes any problem in English. */
export function checkLayout(value: unknown): LayoutCheck {
  const result = layoutSchema.safeParse(value, ENGLISH);
  return result.success ? { ok: true, layout: result.data } : { ok: false, error: z.prettifyError(result.error) };
}

/** The prefix of measure keys in `column_widths`, which no column name is expected to start with. */
export const MEASURE_WIDTH_PREFIX = "__measure:";

/** The key a measure's width is stored under in `column_widths`. */
export function measureWidthKey(measure: number): string {
  return `${MEASURE_WIDTH_PREFIX}${String(measure)}`;
}

/** The measure a `column_widths` key belongs to, or null for a column name. */
export function measureOfWidthKey(key: string): number | null {
  const rest = key.startsWith(MEASURE_WIDTH_PREFIX) ? key.slice(MEASURE_WIDTH_PREFIX.length) : "";
  return /^\d+$/.test(rest) ? Number(rest) : null;
}

/** The key the group column's width is stored under in `column_widths`. */
export const TREE_WIDTH_KEY = "__tree";
