/**
 * Exact conversions of cell values for the protocol and for colouring.
 *
 * Display text is not made here: the engine formats every cell with the rules
 * the Excel export uses and sends the text in the frame. These helpers only turn
 * values into exact JSON for layout paths and read the sign of display text.
 */

import type { CellValue } from "./frame";

/** Writes an unscaled decimal exactly, without grouping, such as `-1234.50`. */
export function exactDecimal(unscaled: bigint, scale: number): string {
  const negative = unscaled < 0n;
  const digits = (negative ? -unscaled : unscaled).toString().padStart(scale + 1, "0");
  const whole = digits.slice(0, digits.length - scale);
  const fraction = scale > 0 ? `.${digits.slice(digits.length - scale)}` : "";
  return `${negative ? "-" : ""}${whole}${fraction}`;
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
      return exactDecimal(value.unscaled, value.scale);
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

/**
 * Returns `true` when display text shows a negative number, which draws in the
 * negative colour. The text decides, not the value: -0.001 at two decimals shows
 * as an unsigned 0.00 and is not coloured.
 */
export function isNegativeText(text: string): boolean {
  return text.startsWith("-") || text.startsWith("(");
}
