/**
 * Frames: windows of a view, decoded from Arrow IPC.
 *
 * A frame crosses an I/O boundary, so every column is checked against the type the
 * engine promises before any UI code reads it. Each field names its role in its
 * metadata, and data columns are matched to their flags and text by index, never by
 * name: names are for people and can repeat.
 *
 * Display text comes from the engine's formatter, the one the Excel export uses,
 * so the screen and the export agree. Values are read from the raw Arrow buffers
 * rather than Arrow JS's convenience getters: those turn microsecond timestamps
 * into fractional milliseconds and decimals into objects, and either would lose
 * digits.
 */

import { DataType, Table, tableFromIPC } from "apache-arrow";
import { z } from "zod";
import type { Field, Vector } from "apache-arrow";

import { exactDecimal } from "./format";
import type { Frame } from "./proto/parallax_pb";

/** A cell value, typed by what the engine sent. */
export type CellValue =
  | { readonly kind: "null" }
  | { readonly kind: "number"; readonly value: number }
  | { readonly kind: "integer"; readonly value: bigint }
  | { readonly kind: "decimal"; readonly unscaled: bigint; readonly scale: number }
  | { readonly kind: "text"; readonly value: string }
  | { readonly kind: "boolean"; readonly value: boolean }
  | { readonly kind: "date"; readonly days: number }
  | { readonly kind: "timestamp"; readonly micros: bigint; readonly zoned: boolean };

/** Cell flags carried next to each measure column. */
export const enum CellFlag {
  Ok = 0,
  Overflow = 1,
  Mixed = 2,
  Undefined = 3
}

/** One data column of a frame. */
export interface FrameColumn {
  /** The column's identity as the engine produced it. */
  readonly name: string;
  /** The header text, naming the display zone of instants. */
  readonly title: string;
  /** Reads row `row` of the window. */
  readonly value: (row: number) => CellValue;
  /** Reads row `row`'s flag, for measure columns that carry one. */
  readonly flag: (row: number) => CellFlag;
  /** Reads row `row`'s display text. */
  readonly text: (row: number) => string;
  /** Reads row `row`'s text for copying: every digit of numbers and timestamps. */
  readonly copy: (row: number) => string;
  /** `true` for numbers, which align right. */
  readonly numeric: boolean;
  /** For a column under column pivots, its pivot values as exact JSON; null otherwise. */
  readonly split: readonly (string | number | boolean | null)[] | null;
}

/** The key of one group level, on every row of a grouped view. */
export interface GroupKey {
  readonly name: string;
  readonly value: (row: number) => CellValue;
}

/** Tree columns of a grouped view's rows. */
export interface TreeRows {
  readonly depth: (row: number) => number;
  readonly kind: (row: number) => "total" | "group" | "leaf" | "other";
  readonly expanded: (row: number) => boolean | null;
  readonly label: (row: number) => string;
}

/** A decoded, validated window of a view. */
export interface FrameData {
  readonly viewId: number;
  readonly configVersion: bigint;
  readonly loadVersion: bigint;
  readonly firstRow: number;
  readonly firstColumn: number;
  readonly rowCount: number;
  readonly totalRows: number;
  readonly totalColumns: number;
  readonly sourceAsOfMicros: bigint;
  readonly pushed: boolean;
  /** Data columns, in display order. */
  readonly columns: readonly FrameColumn[];
  /** Group keys from the outermost level, for grouped views. */
  readonly groups: readonly GroupKey[];
  /** Present for grouped views. */
  readonly tree: TreeRows | null;
}

/** A frame the client cannot trust. */
export class FrameError extends Error {
  public override readonly name = "FrameError";
}

/** The single chunk of a frame column, with its values typed as unknown until checked. */
interface Chunk {
  readonly values: unknown;
  readonly valid: (row: number) => boolean;
  readonly get: (row: number) => unknown;
}

function only(vector: Vector): Chunk {
  const first = vector.data.at(0);
  if (first === undefined || vector.data.length > 1) {
    throw new FrameError("a frame must hold exactly one record batch");
  }
  const values: unknown = first.values;
  return {
    values,
    valid: (row) => first.getValid(row),
    get: (row) => {
      const value: unknown = vector.get(row);
      return value;
    }
  };
}

/** Reconstructs a signed 128-bit integer from four little-endian 32-bit words. */
export function decimalFromWords(words: Uint32Array, offset: number): bigint {
  let value = 0n;
  for (let i = 3; i >= 0; i -= 1) {
    value = (value << 32n) | BigInt(words[offset + i] ?? 0);
  }
  return BigInt.asIntN(128, value);
}

const NULL: CellValue = { kind: "null" };

function numberReader(name: string, chunk: Chunk): (row: number) => CellValue {
  const values = chunk.values;
  if (!(values instanceof Float64Array)) {
    throw new FrameError(`${name}: expected 64-bit floats`);
  }
  return (row) => (chunk.valid(row) ? { kind: "number", value: values[row] ?? Number.NaN } : NULL);
}

function integerReader(name: string, chunk: Chunk): (row: number) => CellValue {
  const values = chunk.values;
  if (values instanceof BigInt64Array) {
    return (row) => (chunk.valid(row) ? { kind: "integer", value: values[row] ?? 0n } : NULL);
  }
  if (values instanceof Int32Array) {
    return (row) => (chunk.valid(row) ? { kind: "integer", value: BigInt(values[row] ?? 0) } : NULL);
  }
  throw new FrameError(`${name}: unsupported integer width`);
}

function reader(field: Field, vector: Vector): (row: number) => CellValue {
  const chunk = only(vector);
  const type: unknown = field.type;
  const name = field.name;
  if (DataType.isFloat(type)) {
    return numberReader(name, chunk);
  }
  if (DataType.isInt(type)) {
    return integerReader(name, chunk);
  }
  if (DataType.isDecimal(type)) {
    const values = chunk.values;
    if (!(values instanceof Uint32Array) || type.bitWidth !== 128) {
      throw new FrameError(`${name}: expected a 128-bit decimal`);
    }
    const scale = type.scale;
    return (row) => (chunk.valid(row) ? { kind: "decimal", unscaled: decimalFromWords(values, row * 4), scale } : NULL);
  }
  if (DataType.isUtf8(type)) {
    return (row) => {
      const text = chunk.get(row);
      return typeof text === "string" ? { kind: "text", value: text } : NULL;
    };
  }
  if (DataType.isBool(type)) {
    return (row) => {
      const flag = chunk.get(row);
      return typeof flag === "boolean" ? { kind: "boolean", value: flag } : NULL;
    };
  }
  if (DataType.isDate(type)) {
    const values = chunk.values;
    if (!(values instanceof Int32Array)) {
      throw new FrameError(`${name}: expected day-precision dates`);
    }
    return (row) => (chunk.valid(row) ? { kind: "date", days: values[row] ?? 0 } : NULL);
  }
  if (DataType.isTimestamp(type)) {
    const values = chunk.values;
    if (!(values instanceof BigInt64Array)) {
      throw new FrameError(`${name}: expected 64-bit timestamps`);
    }
    const zoned = type.timezone !== null;
    return (row) => (chunk.valid(row) ? { kind: "timestamp", micros: values[row] ?? 0n, zoned } : NULL);
  }
  throw new FrameError(`${name}: unsupported type ${String(type)}`);
}

function cellFlag(value: number): CellFlag {
  switch (value) {
    case 0:
      return CellFlag.Ok;
    case 1:
      return CellFlag.Overflow;
    case 2:
      return CellFlag.Mixed;
    case 3:
      return CellFlag.Undefined;
    default:
      throw new FrameError(`unknown cell flag ${String(value)}`);
  }
}

// Flags and text are checked for every row at decode time, so a frame the client
// cannot draw is refused before any grid reads it.

function flagReader(vector: Vector | null, rowCount: number): (row: number) => CellFlag {
  if (vector === null) {
    return () => CellFlag.Ok;
  }
  const values = only(vector).values;
  if (!(values instanceof Uint8Array)) {
    throw new FrameError("flag columns must be 8-bit");
  }
  const flags = Array.from({ length: rowCount }, (_, row) => cellFlag(values[row] ?? 0));
  return (row) => flags[row] ?? CellFlag.Ok;
}

function textReader(name: string, vector: Vector, rowCount: number): (row: number) => string {
  const chunk = only(vector);
  const texts = Array.from({ length: rowCount }, (_, row) => {
    const text = chunk.get(row);
    if (typeof text !== "string") {
      throw new FrameError(`${name}: display text is missing`);
    }
    return text;
  });
  return (row) => texts[row] ?? "";
}

function rowKind(value: unknown): "total" | "group" | "leaf" | "other" {
  switch (value) {
    case "total":
    case "group":
    case "leaf":
    case "other":
      return value;
    default:
      throw new FrameError(`unknown row kind ${String(value)}`);
  }
}

/** The roles a frame field can have, as the engine writes them. */
type Role = "seq" | "depth" | "kind" | "expanded" | "label" | "group" | "value" | "flag" | "text" | "copy";

function roleOf(field: Field): Role {
  const role = field.metadata.get("parallax.role") ?? "";
  switch (role) {
    case "seq":
    case "depth":
    case "kind":
    case "expanded":
    case "label":
    case "group":
    case "value":
    case "flag":
    case "text":
    case "copy":
      return role;
    default:
      throw new FrameError(`${field.name}: unknown role "${role}"`);
  }
}

function indexOf(field: Field, key: "parallax.index" | "parallax.level"): number {
  const text = field.metadata.get(key) ?? "";
  if (!/^\d+$/.test(text)) {
    throw new FrameError(`${field.name}: ${key} must be a non-negative integer`);
  }
  return Number(text);
}

function treeRows(rowCount: number, byRole: ReadonlyMap<Role, Vector>): TreeRows | null {
  const depth = byRole.get("depth");
  const kind = byRole.get("kind");
  const expanded = byRole.get("expanded");
  const label = byRole.get("label");
  if (depth === undefined && kind === undefined && expanded === undefined && label === undefined) {
    return null;
  }
  if (depth === undefined || kind === undefined || expanded === undefined || label === undefined) {
    throw new FrameError("a grouped frame needs depth, kind, expanded and label columns");
  }
  const depths = only(depth).values;
  if (!(depths instanceof Int32Array)) {
    throw new FrameError("__depth must be 32-bit integers");
  }
  const kinds = only(kind);
  // Every row kind is checked here, at decode time, so a frame the client does not
  // understand is refused before any grid draws it.
  const rowKinds = Array.from({ length: rowCount }, (_, row) => rowKind(kinds.get(row)));
  const flags = only(expanded);
  const labels = textReader("__label", label, rowCount);
  return {
    depth: (row) => depths[row] ?? 0,
    kind: (row) => rowKinds[row] ?? "leaf",
    expanded: (row) => {
      const value = flags.get(row);
      return typeof value === "boolean" ? value : null;
    },
    label: labels
  };
}

/** Text for copying a value: every digit, in a form spreadsheets parse. */
function exactText(value: CellValue): string | null {
  switch (value.kind) {
    case "number":
      return Number.isFinite(value.value) ? String(value.value) : null;
    case "integer":
      return value.value.toString();
    case "decimal":
      return exactDecimal(value.unscaled, value.scale);
    case "null":
    case "text":
    case "boolean":
    case "date":
    case "timestamp":
      return null;
  }
}

interface DataParts {
  value?: { readonly field: Field; readonly vector: Vector };
  flag?: Vector;
  text?: Vector;
  copy?: Vector;
}

/** Returns `true` for the Arrow types that hold numbers. */
export function isNumericType(type: unknown): boolean {
  return DataType.isFloat(type) || DataType.isInt(type) || DataType.isDecimal(type);
}

const splitSchema = z.array(z.union([z.string(), z.number(), z.boolean(), z.null()]));

/** Reads a column's pivot values from its metadata. */
function splitOf(field: Field): FrameColumn["split"] {
  const text = field.metadata.get("parallax.split");
  if (text === undefined) {
    return null;
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new FrameError(`${field.name}: pivot values are not JSON`);
  }
  const result = splitSchema.safeParse(parsed);
  if (!result.success) {
    throw new FrameError(`${field.name}: pivot values must be a list of plain values`);
  }
  return result.data;
}

function dataColumn(index: number, parts: DataParts, rowCount: number): FrameColumn {
  const { value, text } = parts;
  if (value === undefined || text === undefined) {
    throw new FrameError(`data column ${String(index)} needs values and display text`);
  }
  const { field, vector } = value;
  const read = reader(field, vector);
  const flag = flagReader(parts.flag ?? null, rowCount);
  const shown = textReader(`${field.name}#text`, text, rowCount);
  const copied = parts.copy === undefined ? null : textReader(`${field.name}#copy`, parts.copy, rowCount);
  const title = field.metadata.get("parallax.title") ?? field.name;
  return {
    name: field.name,
    title,
    value: read,
    flag,
    text: shown,
    copy: (row) => {
      if (flag(row) !== CellFlag.Ok) {
        return shown(row);
      }
      if (copied !== null) {
        return copied(row);
      }
      return exactText(read(row)) ?? shown(row);
    },
    numeric: isNumericType(field.type),
    split: splitOf(field)
  };
}

/** Decodes and validates a frame. */
export function decodeFrame(frame: Frame): FrameData {
  const decoded = tableFromIPC(frame.batch);
  if (!(decoded instanceof Table)) {
    throw new FrameError("a frame's batch must decode synchronously");
  }
  const table = decoded;
  const byRole = new Map<Role, Vector>();
  const data = new Map<number, DataParts>();
  const groups = new Map<number, GroupKey>();
  table.schema.fields.forEach((field, i) => {
    const vector = table.getChildAt(i);
    if (vector === null) {
      throw new FrameError(`missing column ${field.name}`);
    }
    const role = roleOf(field);
    switch (role) {
      case "seq":
      case "depth":
      case "kind":
      case "expanded":
      case "label":
        byRole.set(role, vector);
        return;
      case "group":
        groups.set(indexOf(field, "parallax.level"), { name: field.name, value: reader(field, vector) });
        return;
      case "value":
      case "flag":
      case "text":
      case "copy": {
        const index = indexOf(field, "parallax.index");
        const parts = data.get(index) ?? {};
        if (parts[role] !== undefined) {
          throw new FrameError(`data column ${String(index)} has two ${role} columns`);
        }
        if (role === "value") {
          parts.value = { field, vector };
        } else {
          parts[role] = vector;
        }
        data.set(index, parts);
        return;
      }
    }
  });
  const columns = Array.from({ length: data.size }, (_, index) => {
    const parts = data.get(index);
    if (parts === undefined) {
      throw new FrameError(`data columns must be numbered from 0; ${String(index)} is missing`);
    }
    return dataColumn(index, parts, table.numRows);
  });
  const levels = Array.from({ length: groups.size }, (_, level) => {
    const key = groups.get(level);
    if (key === undefined) {
      throw new FrameError(`group levels must be numbered from 0; ${String(level)} is missing`);
    }
    return key;
  });
  return {
    viewId: frame.viewId,
    configVersion: frame.configVersion,
    loadVersion: frame.loadVersion,
    firstRow: Number(frame.firstRow),
    firstColumn: frame.firstColumn,
    rowCount: table.numRows,
    totalRows: Number(frame.totalRows),
    totalColumns: frame.totalColumns,
    sourceAsOfMicros: frame.sourceAsOfMicros,
    pushed: frame.pushed,
    columns,
    groups: levels,
    tree: treeRows(table.numRows, byRole)
  };
}

/** One value of a column with its display text and how many rows hold it. */
export interface DistinctValue {
  readonly value: CellValue;
  readonly text: string;
  readonly rows: bigint;
}

/** Decodes and checks the values the engine lists for a filter. */
export function decodeDistinct(bytes: Uint8Array): readonly DistinctValue[] {
  const decoded = tableFromIPC(bytes);
  if (!(decoded instanceof Table)) {
    throw new FrameError("distinct values must decode synchronously");
  }
  const table = decoded;
  const field = (name: string): { readonly field: Field; readonly vector: Vector } => {
    const index = table.schema.fields.findIndex((f) => f.name === name);
    const vector = table.getChildAt(index);
    const found = table.schema.fields[index];
    if (found === undefined || vector === null) {
      throw new FrameError(`distinct values need a ${name} column`);
    }
    return { field: found, vector };
  };
  const value = field("value");
  const read = reader(value.field, value.vector);
  const text = textReader("text", field("text").vector, table.numRows);
  const count = field("count");
  const counts = reader(count.field, count.vector);
  return Array.from({ length: table.numRows }, (_, row) => {
    const rows = counts(row);
    if (rows.kind !== "integer") {
      throw new FrameError("distinct-value counts must be integers");
    }
    return { value: read(row), text: text(row), rows: rows.value };
  });
}
