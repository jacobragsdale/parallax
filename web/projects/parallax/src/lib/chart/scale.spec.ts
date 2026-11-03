import { describe, expect, it } from "vitest";

import { linearScale, logDomain, logScale, niceDomain, tickLabels, valueTicks } from "./scale";

describe("scales", () => {
  it("round a domain to steps, reaching zero when asked", () => {
    expect(niceDomain(3, 97, 5, false)).toEqual([0, 100]);
    expect(niceDomain(30, 97, 5, true)).toEqual([0, 100]);
    expect(niceDomain(-12, 37, 5, false)).toEqual([-20, 40]);
    expect(niceDomain(5, 5, 5, false)[0]).toBeLessThan(5);
    expect(niceDomain(Number.NaN, 1, 5, false)).toEqual([0, 1]);
  });

  it("map and invert linearly and logarithmically", () => {
    const linear = linearScale([0, 100], [200, 0]);
    expect(linear.map(25)).toBe(150);
    expect(linear.invert(150)).toBe(25);
    const log = logScale(logDomain(3, 4_000), [0, 400]);
    expect(log.domain).toEqual([1, 10_000]);
    expect(log.map(100)).toBe(200);
    expect(log.map(0)).toBe(0);
  });

  it("label ticks in full under 10,000 and compactly above, one style per axis", () => {
    expect(tickLabels([0, 2_500, 5_000, 7_500], 2_500)).toEqual(["0", "2,500", "5,000", "7,500"]);
    expect(tickLabels([0, 10_000, 20_000], 10_000)).toEqual(["0", "10K", "20K"]);
    expect(tickLabels([0, 2_500_000, 5_000_000], 2_500_000)).toEqual(["0", "2.5M", "5.0M"]);
    expect(tickLabels([0, 0.25, 0.5], 0.25)).toEqual(["0", "0.25", "0.50"]);
  });

  it("place round ticks across the domain, powers of ten on a log scale", () => {
    expect(valueTicks(linearScale([0, 100], [0, 1]), 5).map((t) => t.value)).toEqual([0, 20, 40, 60, 80, 100]);
    expect(valueTicks(logScale([1, 1_000], [0, 1]), 5).map((t) => t.label)).toEqual(["1", "10", "100", "1,000"]);
    // One style per axis: once a log axis reaches 10,000, its thousands are compact too.
    expect(valueTicks(logScale([0.1, 100_000], [0, 1]), 5).map((t) => t.label)).toEqual(["0.1", "1", "10", "100", "1K", "10K", "100K"]);
  });
});
