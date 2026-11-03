import { describe, expect, it } from "vitest";
import * as z from "zod/mini";

import { checkLayout, layoutSchema, parseLayout } from "./layout";
import fixtures from "./testing/layouts.json";

// The fixtures are written by parallax-protocol's tests, from the Rust types.
const written: readonly unknown[] = fixtures;

describe("layoutSchema", () => {
  it("reads every layout the Rust crate writes back unchanged", () => {
    expect(written.length).toBeGreaterThan(1);
    for (const layout of written) {
      expect(parseLayout(layout)).toEqual(layout);
    }
  });

  it("fills in the same defaults as the Rust crate", () => {
    expect(parseLayout({})).toEqual(written[0]);
  });

  it("refuses unknown fields, as the Rust crate does", () => {
    expect(layoutSchema.safeParse({ group_by: ["fund"], groupBy: ["fund"] }).success).toBe(false);
    expect(layoutSchema.safeParse({ measures: [{ aggregate: "sum", column: "x", colour: "red" }] }).success).toBe(false);
  });

  it("refuses values the engine would refuse", () => {
    expect(layoutSchema.safeParse({ version: 2 }).success).toBe(false);
    expect(layoutSchema.safeParse({ column_widths: { x: -1 } }).success).toBe(false);
    expect(layoutSchema.safeParse({ measures: [{ aggregate: "percentile", percentile: 1.5 }] }).success).toBe(false);
    expect(layoutSchema.safeParse({ formats: { x: { decimals: 31 } } }).success).toBe(false);
  });

  it("brings older versions up to date, as the Rust crate does", () => {
    expect(parseLayout({ version: 0 }).version).toBe(1);
  });

  it("says what is wrong in English, without changing Zod's settings for the host", () => {
    const checked = checkLayout({ version: 1.5, colour: "red" });
    expect(checked.ok).toBe(false);
    expect(checked.ok ? "" : checked.error).toContain('Unrecognized key: "colour"');
    expect(checked.ok ? "" : checked.error).toContain("Invalid input: expected int, received number");
    expect(() => parseLayout({ expand_depth: -1 })).toThrow("Too small: expected number to be >=0");
    // Zod Mini's own default, which a host's schemas still get.
    expect(z.int().safeParse(1.5).error?.issues[0]?.message).toBe("Invalid input");
  });
});
