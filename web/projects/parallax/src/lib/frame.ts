/**
 * Frames: windows of a view, decoded from Arrow IPC.
 *
 * A frame crosses an I/O boundary, so every column is checked against the type the
 * engine promises before any UI code reads it. Values are read from the raw Arrow
 * buffers rather than Arrow JS's convenience getters: those turn microsecond
 * timestamps into fractional milliseconds and decimals into objects, and either
 * would lose digits.
 */

import { DataType, Table, tableFromIPC } from "apache-arrow";
import type { Field, Vector } from "apache-arrow";

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

/** One column of a frame. */
export interface FrameColumn {
  /** The column's name as the engine produced it. */
  readonly name: string;
  /** Reads row `row` of the window. */
  readonly value: (row: number) => CellValue;
  /** Reads row `row`'s flag, for measure columns that carry one. */
  readonly flag: (row: number) => CellFlag;
  /** `true` for numbers, which align right. */
  readonly numeric: boolean;
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
  /** Present for grouped views. */
  readonly tree: TreeRows | null;
}

/** A frame the client cannot trust. */
export class FrameError extends Error {
  public override readonly name = "FrameError";
}

const META = new Set(["__depth", "__kind", "__expanded", "__label", "__seq"]);

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

function flagReader(vector: Vector | null): (row: number) => CellFlag {
  if (vector === null) {
    return () => CellFlag.Ok;
  }
  const values = only(vector).values;
  if (!(values instanceof Uint8Array)) {
    throw new FrameError("flag columns must be 8-bit");
  }
  return (row) => {
    const value = values[row] ?? 0;
    switch (value) {
      case 1:
        return CellFlag.Overflow;
      case 2:
        return CellFlag.Mixed;
      case 3:
        return CellFlag.Undefined;
      default:
        return CellFlag.Ok;
    }
  };
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

function treeRows(table: Table): TreeRows | null {
  const depth = table.getChild("__depth");
  const kind = table.getChild("__kind");
  const expanded = table.getChild("__expanded");
  const label = table.getChild("__label");
  if (depth === null || kind === null || expanded === null || label === null) {
    return null;
  }
  const depths = only(depth).values;
  if (!(depths instanceof Int32Array)) {
    throw new FrameError("__depth must be 32-bit integers");
  }
  const kinds = only(kind);
  // Every row kind is checked here, at decode time, so a frame the client does not
  // understand is refused before any grid draws it.
  const rowKinds = Array.from({ length: table.numRows }, (_, row) => rowKind(kinds.get(row)));
  const flags = only(expanded);
  const labels = only(label);
  return {
    depth: (row) => depths[row] ?? 0,
    kind: (row) => rowKinds[row] ?? "leaf",
    expanded: (row) => {
      const value = flags.get(row);
      return typeof value === "boolean" ? value : null;
    },
    label: (row) => {
      const value = labels.get(row);
      return typeof value === "string" ? value : "";
    }
  };
}

/** Decodes and validates a frame. */
export function decodeFrame(frame: Frame): FrameData {
  const decoded = tableFromIPC(frame.batch);
  if (!(decoded instanceof Table)) {
    throw new FrameError("a frame's batch must decode synchronously");
  }
  const table = decoded;
  const columns: FrameColumn[] = [];
  for (const field of table.schema.fields) {
    if (META.has(field.name) || field.name.endsWith("#flag")) {
      continue;
    }
    const vector = table.getChild(field.name);
    if (vector === null) {
      throw new FrameError(`missing column ${field.name}`);
    }
    const value = reader(field, vector);
    columns.push({
      name: field.name,
      value,
      flag: flagReader(table.getChild(`${field.name}#flag`)),
      numeric: DataType.isFloat(field.type) || DataType.isInt(field.type) || DataType.isDecimal(field.type)
    });
  }
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
    tree: treeRows(table)
  };
}
