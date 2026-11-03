/**
 * Layouts: the JSON document that describes a view.
 *
 * The Rust types in `parallax-protocol` are the source of truth; their JSON Schema
 * is committed as `schema/layout.schema.json`. This schema mirrors them so that a
 * layout from storage or a URL is checked before use. A test reads layouts the Rust
 * crate wrote and requires each to come back unchanged, so the two cannot drift.
 */

import { z } from "zod";

/** The layout version this library writes. */
export const LAYOUT_VERSION = 1;

const name = z.string();
const count = z.number().int().nonnegative();
/** A JSON scalar: filter operands and group-path values. */
const scalar = z.union([z.string(), z.number(), z.boolean(), z.null()]);

const formatSchema = z.strictObject({
  decimals: z.number().int().min(0).max(30).nullable().default(null),
  thousands: z.boolean().nullable().default(null),
  percent: z.boolean().default(false),
  abbreviate: z.enum(["thousands", "millions", "billions"]).nullable().default(null),
  negative: z.enum(["minus", "parentheses"]).default("minus"),
  zone: z.string().nullable().default(null),
  precision: z.enum(["seconds", "milliseconds", "microseconds"]).default("seconds")
});

/** The aggregates a measure can use. */
export const AGGREGATES = ["count", "count_all", "sum", "mean", "weighted_mean", "min", "max", "first", "last", "distinct_count", "variance", "std_dev", "median", "percentile"] as const;

/** How a measure can be shown. */
export const SHOW_AS = ["value", "percent_of_grand_total", "percent_of_column_total", "percent_of_row_total", "percent_of_parent"] as const;

/** The filter operators. */
export const FILTER_OPS = ["eq", "ne", "lt", "le", "gt", "ge", "in", "not_in", "is_null", "is_not_null", "contains", "starts_with"] as const;

const comparisons = ["gt", "ge", "lt", "le", "eq", "ne"] as const;

const measureSchema = z.strictObject({
  column: name.nullable().default(null),
  aggregate: z.enum(AGGREGATES),
  percentile: z.number().min(0).max(1).nullable().default(null),
  weight: name.nullable().default(null),
  unit_column: name.nullable().default(null),
  show_as: z.enum(SHOW_AS).default("value"),
  format: formatSchema.nullable().default(null)
});

const filterSchema = z.strictObject({ column: name, op: z.enum(FILTER_OPS), value: scalar.default(null), values: z.array(scalar).default([]) });

const sortSchema = z.strictObject({ column: name, descending: z.boolean().default(false) });

const groupSortSchema = z.strictObject({ by: z.union([z.literal("key"), z.strictObject({ measure: count })]).default("key"), descending: z.boolean().default(false) });

const havingSchema = z.strictObject({ measure: count, op: z.enum(comparisons), value: z.number() });

const topNSchema = z.strictObject({ n: count, measure: count, largest: z.boolean().default(true), other: z.boolean().default(true) });

const expandedSchema = z.strictObject({ path: z.array(scalar), expanded: z.boolean() });

/** The schema of a layout document. Unknown fields are errors, as in Rust. */
export const layoutSchema = z.strictObject({
  // Older versions are read and brought up to date, as the Rust crate does.
  version: z
    .number()
    .int()
    .min(0)
    .max(LAYOUT_VERSION)
    .default(LAYOUT_VERSION)
    .transform((): typeof LAYOUT_VERSION => LAYOUT_VERSION),
  dataset: z.string().default(""),
  parameters: z.record(z.string(), z.string()).default({}),
  columns: z.array(name).default([]),
  group_by: z.array(name).default([]),
  split_by: z.array(name).default([]),
  measures: z.array(measureSchema).default([]),
  filters: z.array(filterSchema).default([]),
  sort: z.array(sortSchema).default([]),
  group_sort: groupSortSchema.default({ by: "key", descending: false }),
  having: z.array(havingSchema).default([]),
  top_n: topNSchema.nullable().default(null),
  expand_depth: count.default(0),
  expanded: z.array(expandedSchema).default([]),
  formats: z.record(name, formatSchema).default({}),
  column_widths: z.record(z.string(), z.number().int().min(0).max(4_294_967_295)).default({})
});

/** A complete layout, with every default filled in. */
export type Layout = z.output<typeof layoutSchema>;
/** A layout as written by hand: every field may be left out. */
export type LayoutInput = z.input<typeof layoutSchema>;
/** One measure of a layout. */
export type LayoutMeasure = z.output<typeof measureSchema>;
/** One filter of a layout. */
export type LayoutFilter = z.output<typeof filterSchema>;
/** A display format. */
export type LayoutFormat = z.output<typeof formatSchema>;
/** A JSON scalar in a filter or group path. */
export type LayoutScalar = z.output<typeof scalar>;

/** Reads and checks a layout from any source, filling in defaults. */
export function parseLayout(value: unknown): Layout {
  return layoutSchema.parse(value);
}

/** The key a measure's width is stored under in `column_widths`. */
export function measureWidthKey(measure: number): string {
  return String(measure);
}

/** The key the group column's width is stored under in `column_widths`. */
export const TREE_WIDTH_KEY = "__tree";
