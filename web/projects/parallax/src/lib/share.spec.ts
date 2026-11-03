import { describe, expect, it } from "vitest";

import { parseLayout } from "./layout";
import { decodeLayout, encodeLayout } from "./share";
import fixtures from "./testing/layouts.json";

const written: readonly unknown[] = fixtures;

describe("layout links", () => {
  it("round-trip every layout the Rust crate writes", async () => {
    for (const layout of written) {
      const link = await encodeLayout(parseLayout(layout));
      expect(link).toMatch(/^1\.[A-Za-z0-9_-]+$/);
      expect(await decodeLayout(link)).toEqual(layout);
    }
  });

  it("stay short enough for a URL", async () => {
    const full = parseLayout(written[1]);
    expect((await encodeLayout(full)).length).toBeLessThan(1500);
  });

  it("refuse text that is not a layout link, or a layout the schema rejects", async () => {
    await expect(decodeLayout("hello")).rejects.toThrow(/not a parallax layout link/);
    const edited = await encodeLayout({ dataset: "x" });
    await expect(decodeLayout(`${edited.slice(0, -4)}AAAA`)).rejects.toThrow();
  });
});
