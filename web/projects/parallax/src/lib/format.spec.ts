import { describe, expect, it } from "vitest";

import { cellText, formatDecimal, pathValue } from "./format";
import { CellFlag, decimalFromWords } from "./frame";

function words(value: bigint): Uint32Array {
  const out = new Uint32Array(4);
  let v = BigInt.asUintN(128, value);
  for (let i = 0; i < 4; i += 1) {
    out[i] = Number(v & 0xffff_ffffn);
    v >>= 32n;
  }
  return out;
}

describe("decimalFromWords", () => {
  it.each([0n, 1n, -1n, 12_345n, -98_765_432_100n, 10n ** 37n, -(10n ** 37n), 2n ** 127n - 1n, -(2n ** 127n)])("rebuilds %s exactly", (value) => {
    expect(decimalFromWords(words(value), 0)).toBe(value);
  });

  it("reads the row at an offset", () => {
    const both = new Uint32Array([...words(7n), ...words(-3n)]);
    expect(decimalFromWords(both, 4)).toBe(-3n);
  });
});

describe("formatDecimal", () => {
  it.each([
    [12_345n, 2, "123.45"],
    [-5n, 3, "-0.005"],
    [0n, 2, "0.00"],
    [123_456_789_012n, 2, "1,234,567,890.12"],
    [-(10n ** 20n), 2, "-1,000,000,000,000,000,000.00"],
    [42n, 0, "42"]
  ])("formats %s at scale %s as %s", (unscaled, scale, expected) => {
    expect(formatDecimal(unscaled, scale)).toBe(expected);
  });
});

describe("cellText", () => {
  it("shows markers for flagged cells instead of a value", () => {
    const value = { kind: "decimal", unscaled: 100n, scale: 2 } as const;
    expect(cellText(value, CellFlag.Ok)).toBe("1.00");
    expect(cellText(value, CellFlag.Mixed)).toBe("mixed");
    expect(cellText(value, CellFlag.Overflow)).toBe("overflow");
    expect(cellText(value, CellFlag.Undefined)).toBe("—");
  });
});

describe("pathValue", () => {
  it("keeps decimals exact and infinities representable", () => {
    expect(pathValue({ kind: "decimal", unscaled: 123_456_789_012_345_678_901n, scale: 4 })).toBe("12345678901234567.8901");
    expect(pathValue({ kind: "number", value: Number.POSITIVE_INFINITY })).toBe("inf");
    expect(pathValue({ kind: "integer", value: 2n ** 60n })).toBe("1152921504606846976");
    expect(pathValue({ kind: "null" })).toBeNull();
    expect(pathValue({ kind: "number", value: Number.NaN })).toBe("NaN");
  });

  it("keeps timestamps to the microsecond, with Z only for instants", () => {
    expect(pathValue({ kind: "timestamp", micros: 1_793_511_000_123_456n, zoned: true })).toBe("2026-11-01T05:30:00.123456Z");
    expect(pathValue({ kind: "timestamp", micros: 1_793_496_600_123_456n, zoned: false })).toBe("2026-11-01T01:30:00.123456");
    expect(pathValue({ kind: "timestamp", micros: -1n, zoned: true })).toBe("1969-12-31T23:59:59.999999Z");
  });
});
