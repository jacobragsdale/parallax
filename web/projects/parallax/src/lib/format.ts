/**
 * Display text for cell values.
 *
 * This is the grid experiment's formatter: exact for decimals and integers, with
 * fixed grouping and no locale. The milestone's formatter, which follows Excel's
 * display rules so that the screen and exports agree, replaces it in step 5.
 */

import { CellFlag } from "./frame";
import type { CellValue } from "./frame";

function group(digits: string): string {
  return digits.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
}

/** Formats an unscaled decimal exactly, grouping the whole part. */
export function formatDecimal(unscaled: bigint, scale: number): string {
  const negative = unscaled < 0n;
  const magnitude = negative ? -unscaled : unscaled;
  const text = magnitude.toString().padStart(scale + 1, "0");
  const whole = text.slice(0, text.length - scale);
  const fraction = scale > 0 ? `.${text.slice(text.length - scale)}` : "";
  const shown = `${group(whole)}${fraction}`;
  return negative && magnitude !== 0n ? `-${shown}` : shown;
}

function formatNumber(value: number): string {
  if (Number.isNaN(value)) {
    return "NaN";
  }
  if (!Number.isFinite(value)) {
    return value > 0 ? "∞" : "-∞";
  }
  if (value === 0) {
    return "0";
  }
  if (Math.abs(value) >= 1e15 || Math.abs(value) < 1e-4) {
    return value.toExponential(4);
  }
  const fixed = value.toFixed(Math.abs(value) >= 100 ? 2 : 4);
  const [whole = "0", fraction] = fixed.replace("-", "").split(".");
  const sign = value < 0 && Number(fixed) !== 0 ? "-" : "";
  return `${sign}${group(whole)}${fraction === undefined ? "" : `.${fraction}`}`;
}

const DAY_MS = 86_400_000;

/** Formats microseconds since the epoch exactly: RFC 3339 with `Z` for instants, a civil date-time for wall-clock values. */
export function timestampText(micros: bigint, zoned: boolean): string {
  const million = 1_000_000n;
  let seconds = micros / million;
  let fraction = micros % million;
  if (fraction < 0n) {
    fraction += million;
    seconds -= 1n;
  }
  const whole = new Date(Number(seconds) * 1000).toISOString().slice(0, 19);
  return `${whole}.${fraction.toString().padStart(6, "0")}${zoned ? "Z" : ""}`;
}

/** Returns the display text of a value. */
export function formatCell(value: CellValue): string {
  switch (value.kind) {
    case "null":
      return "";
    case "number":
      return formatNumber(value.value);
    case "integer":
      return value.value < 0n ? `-${group((-value.value).toString())}` : group(value.value.toString());
    case "decimal":
      return formatDecimal(value.unscaled, value.scale);
    case "text":
      return value.value;
    case "boolean":
      return value.value ? "true" : "false";
    case "date":
      return new Date(value.days * DAY_MS).toISOString().slice(0, 10);
    case "timestamp": {
      const ms = Number(value.micros / 1000n);
      const text = new Date(ms).toISOString().replace("T", " ").replace("Z", "");
      return value.zoned ? `${text} UTC` : text;
    }
  }
}

/**
 * Converts a group value to the JSON a layout path holds, exactly: decimals and
 * integers beyond 2^53 as strings, infinities as `"inf"` and `"-inf"`, timestamps
 * to the microsecond. NaN cannot be a group value, so it becomes `"NaN"`, which the
 * engine rejects rather than matching another group.
 */
export function pathValue(value: CellValue): string | number | boolean | null {
  switch (value.kind) {
    case "null":
      return null;
    case "number":
      if (Number.isNaN(value.value)) {
        return "NaN";
      }
      return Number.isFinite(value.value) ? value.value : value.value > 0 ? "inf" : "-inf";
    case "integer":
      return Number.isSafeInteger(Number(value.value)) ? Number(value.value) : value.value.toString();
    case "decimal":
      return formatDecimal(value.unscaled, value.scale).replaceAll(",", "");
    case "text":
      return value.value;
    case "boolean":
      return value.value;
    case "date":
      return new Date(value.days * DAY_MS).toISOString().slice(0, 10);
    case "timestamp":
      return timestampText(value.micros, value.zoned);
  }
}

/** Returns a cell's text: its value, or the marker its flag calls for. */
export function cellText(value: CellValue, flag: CellFlag): string {
  switch (flag) {
    case CellFlag.Ok:
      return formatCell(value);
    case CellFlag.Overflow:
      return "overflow";
    case CellFlag.Mixed:
      return "mixed";
    case CellFlag.Undefined:
      return "—";
  }
}

/** Returns `true` for negative numbers, which show in the negative colour. */
export function isNegative(value: CellValue): boolean {
  return (value.kind === "number" && value.value < 0) || (value.kind === "decimal" && value.unscaled < 0n) || (value.kind === "integer" && value.value < 0n);
}
