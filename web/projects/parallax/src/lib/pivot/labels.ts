/** Words for the pivot settings, by the layout's names. */

import type { FieldKind, SourceField } from "../grid/grid-source";
import type { LayoutMeasure, LayoutScalar } from "../layout";

const LABELS: Readonly<Record<string, string>> = {
  count: "Count",
  count_all: "Count rows",
  sum: "Sum",
  mean: "Mean",
  weighted_mean: "Weighted mean",
  min: "Min",
  max: "Max",
  first: "First",
  last: "Last",
  distinct_count: "Distinct count",
  variance: "Variance",
  std_dev: "Std dev",
  median: "Median",
  percentile: "Percentile",
  value: "Value",
  percent_of_grand_total: "% of total",
  percent_of_column_total: "% of column",
  percent_of_row_total: "% of row",
  percent_of_parent: "% of parent",
  eq: "=",
  ne: "≠",
  lt: "<",
  le: "≤",
  gt: ">",
  ge: "≥",
  in: "is one of",
  not_in: "is none of",
  is_null: "is blank",
  is_not_null: "is not blank",
  contains: "contains",
  starts_with: "starts with"
};

/** The label for a layout name. */
export function label(key: string): string {
  return LABELS[key] ?? key;
}

/** A column the pivot can use. */
export type PivotField = SourceField;

/** One option of a select. */
export interface Choice {
  readonly value: string;
  readonly text: string;
}

/** Choices for a list of layout names. */
export function choices(keys: readonly string[]): readonly Choice[] {
  return keys.map((key) => ({ value: key, text: label(key) }));
}

/** Choices for columns. */
export function fieldChoices(fields: readonly PivotField[]): readonly Choice[] {
  return fields.map((f) => ({ value: f.name, text: f.name }));
}

/** Kinds that hold numbers. */
export const NUMERIC_KINDS: readonly FieldKind[] = ["integer", "float", "decimal"];

/** Aggregates that need a numeric column. */
export const NUMERIC_AGGREGATES: readonly LayoutMeasure["aggregate"][] = ["sum", "mean", "weighted_mean", "variance", "std_dev", "median", "percentile"];

/** A short name for a measure, such as "Sum of pnl". */
export function measureName(m: LayoutMeasure): string {
  return m.aggregate === "count_all" ? "Count rows" : `${label(m.aggregate)} of ${m.column ?? "?"}`;
}

/** A filter operand as text; null is empty. */
export function operandText(value: LayoutScalar): string {
  return value === null ? "" : String(value);
}
