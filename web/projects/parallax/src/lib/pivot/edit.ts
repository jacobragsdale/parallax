/**
 * Edits to a pivot layout, as pure functions. Each returns a new layout and keeps
 * the parts that refer to measures or groups by position consistent: removing a
 * measure drops the sorts, filters and widths that named it and renumbers the
 * rest, and changing the row groups forgets which groups were expanded, because
 * their paths no longer mean the same thing.
 */

import type { FieldKind } from "../grid/grid-source";
import { measureOfWidthKey, measureWidthKey } from "../layout";
import type { Layout, LayoutFilter, LayoutMeasure, LayoutScalar } from "../layout";

/** Which list of group columns an edit applies to. */
export type Axis = "group_by" | "split_by";

function move<T>(items: readonly T[], index: number, delta: number): readonly T[] {
  const target = index + delta;
  if (index < 0 || index >= items.length || target < 0 || target >= items.length) {
    return items;
  }
  const out = [...items];
  const [item] = out.splice(index, 1);
  if (item !== undefined) {
    out.splice(target, 0, item);
  }
  return out;
}

/** Keeps `expanded` only while the row groups stay the same. */
function withAxis(layout: Layout, axis: Axis, columns: readonly string[]): Layout {
  return axis === "group_by" ? { ...layout, group_by: [...columns], expanded: [] } : { ...layout, split_by: [...columns] };
}

/** Adds a column to the end of the row or column groups, if it is not there already. */
export function addGroup(layout: Layout, axis: Axis, column: string): Layout {
  return layout[axis].includes(column) ? layout : withAxis(layout, axis, [...layout[axis], column]);
}

/** Removes the group at `index`. */
export function removeGroup(layout: Layout, axis: Axis, index: number): Layout {
  return withAxis(
    layout,
    axis,
    layout[axis].filter((_, i) => i !== index)
  );
}

/** Moves the group at `index` up (`-1`) or down (`1`). */
export function moveGroup(layout: Layout, axis: Axis, index: number, delta: number): Layout {
  return withAxis(layout, axis, move(layout[axis], index, delta));
}

/** Renumbers measure references after the measures were reordered: `from[old]` is the new position, or null if removed. */
function renumber(layout: Layout, measures: readonly LayoutMeasure[], from: readonly (number | null)[]): Layout {
  const at = (old: number): number | null => from[old] ?? null;
  const by = layout.group_sort.by;
  const sortedBy = by === "key" ? null : at(by.measure);
  const widths: Record<string, number> = {};
  for (const [key, width] of Object.entries(layout.column_widths)) {
    const old = measureOfWidthKey(key);
    if (old === null) {
      widths[key] = width;
    } else {
      const now = at(old);
      if (now !== null) {
        widths[measureWidthKey(now)] = width;
      }
    }
  }
  const topMeasure = layout.top_n === null ? null : at(layout.top_n.measure);
  return {
    ...layout,
    measures: [...measures],
    group_sort: sortedBy === null ? { by: "key", descending: by === "key" ? layout.group_sort.descending : false } : { by: { measure: sortedBy }, descending: layout.group_sort.descending },
    having: layout.having.flatMap((h) => {
      const now = at(h.measure);
      return now === null ? [] : [{ ...h, measure: now }];
    }),
    top_n: layout.top_n === null || topMeasure === null ? null : { ...layout.top_n, measure: topMeasure },
    column_widths: widths
  };
}

/** A measure with every field at its default. */
export function measure(aggregate: LayoutMeasure["aggregate"], column: string | null): LayoutMeasure {
  return { column, aggregate, percentile: aggregate === "percentile" ? 0.5 : null, weight: null, unit_column: null, show_as: "value", format: null };
}

/** Adds a measure at the end. */
export function addMeasure(layout: Layout, added: LayoutMeasure): Layout {
  return { ...layout, measures: [...layout.measures, added] };
}

/** Removes the measure at `index`, with the sorts, filters and widths that named it. */
export function removeMeasure(layout: Layout, index: number): Layout {
  const from = layout.measures.map((_, i) => (i === index ? null : i < index ? i : i - 1));
  return renumber(
    layout,
    layout.measures.filter((_, i) => i !== index),
    from
  );
}

/** Moves the measure at `index` up (`-1`) or down (`1`). */
export function moveMeasure(layout: Layout, index: number, delta: number): Layout {
  const target = index + delta;
  if (index < 0 || index >= layout.measures.length || target < 0 || target >= layout.measures.length) {
    return layout;
  }
  const order = move(
    layout.measures.map((_, i) => i),
    index,
    delta
  );
  const from = layout.measures.map((_, old) => order.indexOf(old));
  return renumber(
    layout,
    order.map((old) => layout.measures[old]).filter((m) => m !== undefined),
    from
  );
}

/** Replaces fields of the measure at `index`. */
export function updateMeasure(layout: Layout, index: number, patch: Partial<LayoutMeasure>): Layout {
  return { ...layout, measures: layout.measures.map((m, i) => (i === index ? { ...m, ...patch } : m)) };
}

/** A filter with every field at its default. */
export function filter(column: string, op: LayoutFilter["op"]): LayoutFilter {
  return { column, op, value: null, values: [] };
}

/** Adds a filter at the end. */
export function addFilter(layout: Layout, added: LayoutFilter): Layout {
  return { ...layout, filters: [...layout.filters, added] };
}

/** Removes the filter at `index`. */
export function removeFilter(layout: Layout, index: number): Layout {
  return { ...layout, filters: layout.filters.filter((_, i) => i !== index) };
}

/** Replaces fields of the filter at `index`. */
export function updateFilter(layout: Layout, index: number, patch: Partial<LayoutFilter>): Layout {
  return { ...layout, filters: layout.filters.map((f, i) => (i === index ? { ...f, ...patch } : f)) };
}

/** Whether an operator takes one value, a list, or none. */
export function operandOf(op: LayoutFilter["op"]): "one" | "many" | "none" {
  switch (op) {
    case "in":
    case "not_in":
      return "many";
    case "is_null":
    case "is_not_null":
      return "none";
    case "eq":
    case "ne":
    case "lt":
    case "le":
    case "gt":
    case "ge":
    case "contains":
    case "starts_with":
      return "one";
  }
}

/**
 * Reads typed filter text as the JSON value the engine expects for the column:
 * numbers for doubles, booleans for booleans, and text for everything else, which
 * keeps decimals and large integers exact. Empty text is null.
 */
export function filterValue(kind: FieldKind, text: string): LayoutScalar {
  const trimmed = text.trim();
  if (trimmed === "") {
    return null;
  }
  switch (kind) {
    case "float": {
      const number = Number(trimmed);
      return Number.isFinite(number) ? number : trimmed;
    }
    case "boolean":
      return trimmed.toLowerCase() === "true" ? true : trimmed.toLowerCase() === "false" ? false : trimmed;
    case "integer":
    case "decimal":
    case "date":
    case "timestamp":
      return trimmed;
    case "text":
      return text;
  }
}

/** Sets or clears top-N. */
export function setTopN(layout: Layout, topN: Layout["top_n"]): Layout {
  return { ...layout, top_n: topN };
}

/** Sets how many levels open by default, forgetting groups opened or closed by hand. */
export function setExpandDepth(layout: Layout, depth: number): Layout {
  return { ...layout, expand_depth: Math.max(0, Math.floor(depth)), expanded: [] };
}

/**
 * A flat view of the rows behind one group: the pivot's filters plus one filter
 * per group level and, for a cell under column pivots, one per pivot level, with
 * the pivot's formats. An empty path is the grand total.
 */
export function drillLayout(layout: Layout, path: readonly LayoutScalar[], split: readonly LayoutScalar[] | null = null): Layout {
  const match = (columns: readonly string[], values: readonly LayoutScalar[]): (LayoutFilter | null)[] =>
    values.map((value, i) => {
      const column = columns[i];
      if (column === undefined) {
        return null;
      }
      return value === null ? filter(column, "is_null") : { ...filter(column, "eq"), value };
    });
  const levels = [...match(layout.group_by, path), ...match(layout.split_by, split ?? [])];
  return {
    ...layout,
    columns: [],
    group_by: [],
    split_by: [],
    measures: [],
    filters: [...layout.filters, ...levels.filter((f) => f !== null)],
    sort: [],
    group_sort: { by: "key", descending: false },
    having: [],
    top_n: null,
    expand_depth: 0,
    expanded: [],
    column_widths: {}
  };
}
