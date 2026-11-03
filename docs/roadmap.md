# Roadmap

This roadmap lists the milestones, the build order within milestone 1, and the
performance budgets that decide when a milestone is done. For the reasons
behind the design, see [How parallax is designed](design.md).

## Milestones

| Milestone | Scope |
|---|---|
| 1 | Engine, grid, pivot, Excel export, and connectors for SQL Server (including JSON columns), Oracle Database, Parquet, MongoDB and HTTP (existing REST GETs). Angular package and demo app. |
| 2 | Charts: line, area, bar (vertical or horizontal, grouped or stacked), scatter as density, heatmap, histogram, candlestick and OHLC. Crosshair with synced tooltips, wheel and drag zoom and pan, a range brush that filters linked views, legend toggling, two y-axes, log scale, and a trading-time axis driven by a per-dataset trading calendar. |
| 3 | Technical-analysis tools, described in [Milestone 3 tools](#milestone-3-tools). |

Releases are semver 0.x until milestone 3. Each milestone publishes
`@jacobragsdale/parallax` to npm and the `parallax-*` crates to crates.io.

## Milestone 3 tools

- **Indicators:** SMA, EMA, WMA, Bollinger Bands, RSI, MACD, ATR, VWAP,
  stochastic and volume. Each is checked against TA-Lib.
- **Panes:** stacked price and indicator panes with synced zoom.
- **Drawing tools:** trend line, horizontal line, Fibonacci retracement,
  rectangle and text note, saved in the layout.
- **Other tools:** compare mode, which normalizes series to percentage change
  from the start; event markers; and a measure tool.

## Milestone 1 build order

1. Repository, CI, licence checks and the synthetic data generator.
2. Engine core, with the DuckDB differential test suite.
3. Server, protocol and the WebAssembly build, with tests that the native and
   WebAssembly builds return identical results.
4. The grid experiment: canvas against virtualized DOM, measured, and the
   choice between them.
5. Grid features, pivot UI and layouts.
6. Excel export, with read-back tests.
7. Connectors:
   - SQL Server, including JSON columns
   - Oracle Database, through the same ODBC path
   - Parquet, with the NFS cache
   - MongoDB
   - HTTP, pulling from existing REST GETs as Arrow or JSON
8. The Angular package, the demo app and the Playwright suite.

Milestone 1 is done when all of the following hold:

- Every budget in [Performance budgets](#performance-budgets) is met on the
  benchmark host.
- Every test layer passes in CI.
- The demo has run on a real Citrix VDI.

## Target data for milestone 1

The generator produces two datasets for milestone 1:

- **Positions:** 5 million lots by 40 columns, covering fund, strategy, PM,
  sector, country, currency, ticker, quantity, price, market value, cost,
  unrealized and daily P&L, weights and risk measures. The reference pivot is
  fund → strategy → sector → ticker by asset class, showing market value, P&L
  and weight.
- **Trades:** 20 million rows, which exercise the scroll and filter budgets.

The design envelope covers:

| Quantity | Design target |
|---|---|
| Largest table held on the server | 20 million rows × 60 columns |
| Largest table held in the browser | 5 million rows × 40 columns |
| Distinct values in a text column | about 10,000 |
| Concurrent users per server | 50 |

## Performance budgets

These are starting targets, revised after measurement. Benchmarks run nightly
on one fixed machine and are tracked over time. Shared CI runners are too noisy
for these numbers.

| Measure | Budget |
|---|---|
| Browser tab memory, server-held view | ≤ 300 MB |
| Main-thread time per frame | ≤ 8 ms |
| Redraw rate | ≤ 30 frames per second |
| First paint of a 20-million-row server view | ≤ 300 ms |
| Regroup or re-pivot of 20 million rows | ≤ 500 ms |
| Server memory | ≤ 1.5 × the raw Arrow size of loaded tables |
| Excel export, 1 million rows × 40 columns | ≤ 30 s |

On the build host, single-threaded, an Excel export of 1,000,000 generated
positions by 40 columns took 28.0 s and the same CSV 8.1 s (2026-11-02).

Grouped views build on 16 threads of the build host. Its timings vary about
twofold under load, so these are the best of three runs (2026-11-02):

| Grouped view of generated positions | 1M rows | 2.5M rows |
|---|---|---|
| Count by fund | 5 ms | 23 ms |
| Three sums by fund | 11 ms | 44 ms |
| Three sums by fund, strategy, sector and ticker | 121 ms | 261 ms |
| The reference pivot: those groups by asset class | 217 ms | 426 ms |

The reference pivot took 1,045 ms at 1M rows before the build went leaf-first and
parallel. Groupings with few groups are on course for the 500 ms budget at 20M
rows. The reference pivot, with about 150,000 leaf groups at 1M rows, is not:
its time now goes to building one tree node per group, which needs a more
compact tree to come down further. The build host has too little free memory to
load 20M rows, so the budget is still unmeasured at that size.

The demo's initial bundle is 558 kB, 134 kB compressed (2026-11-02). Zod's
tree-shakable build cut it from 981 kB. Of the rest, Apache Arrow's
JavaScript is 204 kB and Angular 126 kB. The build warns above 600 kB.

A budget for live updates, 10,000 row updates per second with a p99 of
≤ 250 ms from update to screen, waits until a streaming source exists. See
[Deferred work](future.md).

## Verification on a VDI

The demo app has a performance overlay, opened by its **Performance** button.
It shows frame time, the main-thread time blocked by long frames, the grid's
drawing time per frame, tab memory and the time from asking for rows to drawing
them, and a **Copy report** button copies those figures with the browser and
window they were taken on. Tab memory covers the whole tab, worker included,
only when the server sends `cross_origin_isolated` headers and the page is a
secure context (HTTPS or localhost); otherwise it is Chromium's JavaScript heap
for the page, and the report says which. At each milestone the demo runs on a Citrix VDI
against a test deployment, and the copied report joins the benchmark history
next to the benchmark host's figures. The Excel verification workbook is opened
on the same VDI.
