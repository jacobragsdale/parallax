import { describe, expect, it } from "vitest";

import { clampCell, contains, moveFocus, rangeOf, selectAll, toTsv } from "./selection";

const extent = { rows: 100, columns: 5, tree: true };

describe("selection", () => {
  it("normalizes a range whichever way it was dragged", () => {
    expect(rangeOf({ anchor: { row: 9, column: 3 }, focus: { row: 2, column: -1 } })).toEqual({ top: 2, bottom: 9, left: -1, right: 3 });
    expect(contains({ top: 2, bottom: 9, left: -1, right: 3 }, 5, -1)).toBe(true);
    expect(contains({ top: 2, bottom: 9, left: -1, right: 3 }, 10, 0)).toBe(false);
  });

  it("moves the focus within the view, extending only with Shift", () => {
    const at = { anchor: { row: 5, column: 1 }, focus: { row: 5, column: 1 } };
    expect(moveFocus(at, 1, 0, false, extent)).toEqual({ anchor: { row: 6, column: 1 }, focus: { row: 6, column: 1 } });
    expect(moveFocus(at, 1, 1, true, extent)).toEqual({ anchor: { row: 5, column: 1 }, focus: { row: 6, column: 2 } });
    expect(moveFocus(at, -1000, -1000, false, extent).focus).toEqual({ row: 0, column: -1 });
    expect(moveFocus(at, 1000, 1000, false, extent).focus).toEqual({ row: 99, column: 4 });
  });

  it("has no group column in a flat view", () => {
    expect(clampCell({ row: 0, column: -1 }, { rows: 3, columns: 2, tree: false })).toEqual({ row: 0, column: 0 });
    expect(selectAll({ rows: 3, columns: 2, tree: false })).toEqual({ anchor: { row: 0, column: 0 }, focus: { row: 2, column: 1 } });
  });
});

describe("toTsv", () => {
  it("joins cells with tabs and rows with CRLF, quoting as Excel does", () => {
    expect(
      toTsv([
        ["a", "1234567.891"],
        ["b", ""]
      ])
    ).toBe("a\t1234567.891\r\nb\t");
    expect(toTsv([['say "hi"', "two\tcells", "two\nlines"]])).toBe('"say ""hi"""\t"two\tcells"\t"two\nlines"');
  });
});
