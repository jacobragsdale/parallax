"""Spike 0.3: reads back the browser's exports and compares them with native ones.

    uv run --no-build --with openpyxl==3.1.5 --with duckdb==2.0.0 --with pyarrow==26.0.1 python -I spike/readback.py DIR

DIR holds browser-NAME.xlsx and .csv, saved from the page's Blob downloads, and
native-NAME.xlsx and .csv, written by `spike_native` from the same rows. Every
cell's value and number format, the outline levels, frozen panes and column
widths must match; the CSVs must be identical. DuckDB then sums the generated
positions itself, as a check on the numbers that does not use parallax.
"""

import hashlib
import sys
from decimal import Decimal
from pathlib import Path

import duckdb
import openpyxl
import pyarrow.ipc


def sheet_facts(path: Path) -> dict:
    book = openpyxl.load_workbook(path)
    facts = {"sheets": book.sheetnames}
    for sheet in book.worksheets:
        cells = {}
        for row in sheet.iter_rows():
            for cell in row:
                if cell.value is not None:
                    cells[cell.coordinate] = (cell.value, cell.number_format, cell.alignment.indent)
        facts[sheet.title] = {
            "cells": cells,
            "outline": {r: d.outline_level for r, d in sheet.row_dimensions.items() if d.outline_level},
            "freeze": sheet.freeze_panes,
            "widths": {k: round(v.width or 0, 2) for k, v in sheet.column_dimensions.items()},
            "dimensions": sheet.dimensions,
        }
    return facts


def compare_xlsx(browser: Path, native: Path) -> bool:
    a, b = sheet_facts(browser), sheet_facts(native)
    same = a["sheets"] == b["sheets"]
    print(f"{browser.name}: sheets {a['sheets']}")
    for name in a["sheets"]:
        sa, sb = a[name], b.get(name, {})
        diffs = [k for k in sa["cells"].keys() | sb.get("cells", {}).keys() if sa["cells"].get(k) != sb.get("cells", {}).get(k)]
        ok = not diffs and all(sa[k] == sb.get(k) for k in ("outline", "freeze", "widths", "dimensions"))
        same &= ok
        print(
            f"  {name}: {len(sa['cells'])} cells, {sa['dimensions']}, outline rows {len(sa['outline'])},"
            f" frozen at {sa['freeze']}: {'identical to native' if ok else 'DIFFERS'}"
        )
        for key in sorted(diffs)[:5]:
            print(f"    {key}: browser {sa['cells'].get(key)!r} native {sb.get('cells', {}).get(key)!r}")
    return same


def check_sums(folder: Path) -> None:
    """Prints the browser's xlsx headers and first rows of the outlined pivot, beside DuckDB's sums."""
    outlined = folder / "browser-outlined.xlsx"
    positions = folder / "positions.arrows"
    if not (outlined.exists() and positions.exists()):
        return
    sheet = openpyxl.load_workbook(outlined)["View"]
    header = [c.value for c in sheet[1]]
    print("outlined pivot header:", header)
    lots = pyarrow.ipc.open_stream(positions.read_bytes()).read_all()
    sums = dict(duckdb.sql("select fund, sum(market_value_usd) from lots group by fund").fetchall())
    column = header.index("Total | sum(market_value_usd)")
    worst = Decimal(0)
    checked = 0
    for row in sheet.iter_rows(min_row=2):
        name = row[0].value
        if (sheet.row_dimensions[row[0].row].outline_level or 0) <= 1 and name in sums:
            cell = row[column].value
            checked += 1
            worst = max(worst, abs(Decimal(repr(cell)) - sums[name]))
            if checked <= 3:
                print(f"  {name}: xlsx {cell!r}, DuckDB {sums[name]}")
    print(f"DuckDB sums of market_value_usd by fund against the xlsx: {checked} funds, largest difference {worst}")


def main() -> None:
    folder = Path(sys.argv[1])
    ok = True
    for browser in sorted(folder.glob("browser-*.xlsx")):
        ok &= compare_xlsx(browser, folder / browser.name.replace("browser-", "native-"))
    for browser in sorted(folder.glob("browser-*.csv")):
        native = folder / browser.name.replace("browser-", "native-")
        digests = [hashlib.sha256(p.read_bytes()).hexdigest()[:16] for p in (browser, native)]
        same = digests[0] == digests[1]
        ok &= same
        print(f"{browser.name}: sha256 {digests[0]} browser, {digests[1]} native: {'identical' if same else 'DIFFERS'}")

    check_sums(folder)
    print("ALL MATCH" if ok else "SOME DIFFER")


if __name__ == "__main__":
    main()
