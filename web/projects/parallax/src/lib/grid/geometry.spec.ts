import { describe, expect, it } from "vitest";

import { DEFAULT_TREE_WIDTH, DEFAULT_WIDTH, flatGeometry, groupedGeometry, rowGeometry } from "./geometry";

describe("flatGeometry", () => {
  const g = flatGeometry(4, ["a", "b", "c", "d"], { b: 60, d: 300 });

  it("places columns by their own widths", () => {
    expect([0, 1, 2, 3].map((j) => g.x(j))).toEqual([0, DEFAULT_WIDTH, DEFAULT_WIDTH + 60, 2 * DEFAULT_WIDTH + 60]);
    expect(g.contentWidth).toBe(2 * DEFAULT_WIDTH + 60 + 300);
    expect(g.width(3)).toBe(300);
  });

  it("finds the column under any offset", () => {
    expect(g.indexAt(0)).toBe(0);
    expect(g.indexAt(DEFAULT_WIDTH - 1)).toBe(0);
    expect(g.indexAt(DEFAULT_WIDTH)).toBe(1);
    expect(g.indexAt(DEFAULT_WIDTH + 59)).toBe(1);
    expect(g.indexAt(g.contentWidth + 1000)).toBe(3);
  });

  it("gives unnamed columns the default width", () => {
    const partial = flatGeometry(3, ["a"], {});
    expect(partial.contentWidth).toBe(3 * DEFAULT_WIDTH);
    expect(partial.widthKey(0)).toBe("a");
  });
});

describe("groupedGeometry", () => {
  // Three measures repeated for 1,000 pivot values and the row total.
  const g = groupedGeometry(3003, 3, { "0": 100, "2": 50, __tree: 200 });
  const span = 100 + DEFAULT_WIDTH + 50;

  it("repeats measure widths after the pinned group column", () => {
    expect(g.treePx).toBe(200);
    expect(g.x(0)).toBe(200);
    expect(g.x(3)).toBe(200 + span);
    expect(g.x(5)).toBe(200 + span + 100 + DEFAULT_WIDTH);
    expect(g.width(3002)).toBe(50);
    expect(g.contentWidth).toBe(200 + 1001 * span);
  });

  it("finds a column far to the right without walking the others", () => {
    const column = 2003;
    expect(g.indexAt(g.x(column))).toBe(column);
    expect(g.indexAt(g.x(column) + g.width(column) - 1)).toBe(column);
    expect(g.indexAt(0)).toBe(0);
    expect(g.indexAt(1e12)).toBe(3002);
    expect(g.widthKey(column)).toBe("2");
  });

  it("defaults the group column's width", () => {
    expect(groupedGeometry(1, 1, {}).treePx).toBe(DEFAULT_TREE_WIDTH);
  });
});

describe("rowGeometry", () => {
  it("maps rows to offsets and back when the view fits natively", () => {
    const rows = rowGeometry(1000, 24);
    expect(rows.firstRow(240, 600)).toBe(10);
    expect(rows.scrollTopFor(10, 600)).toBe(240);
  });

  it("scales when there are too many rows to scroll natively", () => {
    const rows = rowGeometry(20_000_000, 24);
    expect(rows.spacerPx).toBeLessThan(20_000_000 * 24);
    for (const row of [0, 1, 12_345, 10_000_000, 19_999_970]) {
      expect(rows.firstRow(rows.scrollTopFor(row, 600), 600)).toBe(row);
    }
  });
});
