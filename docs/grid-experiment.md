# The grid experiment: canvas or DOM

parallax's grid draws on a Canvas 2D surface. A virtualized DOM grid, the approach
Perspective takes, spent 1.1 to 2 times as much main-thread time per frame on the
same views. Neither grid meets the 8 ms budget everywhere yet: with the CPU
throttled four times, the canvas grid took 7.3 to 9.5 ms on the pivot and 12.5 to
15.2 ms on the flat view. This page records how that was measured, so the result
can be repeated on the Citrix desktops the decision is for.

## Method

Both grids share everything except drawing: the protocol client, the frame
decoder, the formatter, virtual scrolling, and the rule that at most one viewport
request per view is in flight. Both draw only the visible rows and columns. When
a frame arrives for rows already on screen, the canvas grid redraws only the cells
whose text changed, but any scroll redraws every visible cell. The DOM grid renders
positioned cell elements through Angular's templates.

The Playwright suite in `web/e2e/grid-experiment.spec.ts`, as of commit
`b255e25`, runs against the reference server with `examples/experiment.toml`,
serving 200,000 generated positions and 1,000,000 generated trades.

The browser is Chromium (Playwright's headless shell, build 1248) with
`--disable-gpu`, `--disable-gpu-compositing` and `--disable-software-rasterizer`,
and the CPU throttled four times through the DevTools protocol, as a stand-in for
a GPU-less Citrix desktop. The window is 1,400 × 900.

Each view is scrolled 240 steps, waiting one animation frame per step, in two
modes:

- **smooth:** 3 rows per step, mostly within the rows already received
- **jumping:** 40 rows per step, so most steps need new rows from the server

Main-thread time comes from the DevTools `TaskDuration` metric, and frame times
from animation-frame timestamps. The views are:

- **trades:** a flat view of 1,000,000 trades, 25 columns
- **pivot:** positions grouped by fund, strategy, sector and ticker, split by
  asset class, three measures, three levels expanded

## Results

Three runs on the build host on 2026-11-02 agreed within about 15%. Each cell
lists the three runs.

| Grid | View | Scroll | Main thread ms per frame | Mean frame ms | p95 frame ms |
|---|---|---|---|---|---|
| canvas | trades | smooth | 12.5 / 12.6 / 12.6 | 16.6 / 16.6 / 16.6 | 17.8 / 17.8 / 17.9 |
| canvas | trades | jumping | 15.1 / 15.1 / 15.2 | 16.7 / 16.7 / 16.8 | 25.7 / 26.9 / 27.2 |
| canvas | pivot | smooth | 7.3 / 7.6 / 8.5 | 16.6 / 16.5 / 16.6 | 17.4 / 17.6 / 17.4 |
| canvas | pivot | jumping | 8.8 / 8.7 / 9.5 | 16.6 / 16.6 / 16.6 | 17.6 / 17.6 / 17.7 |
| DOM | trades | smooth | 16.5 / 16.8 / 16.5 | 17.0 / 17.1 / 17.0 | 21.1 / 21.6 / 21.8 |
| DOM | trades | jumping | 16.6 / 16.5 / 16.6 | 16.8 / 16.8 / 16.9 | 26.6 / 26.3 / 27.4 |
| DOM | pivot | smooth | 14.0 / 14.6 / 13.7 | 16.6 / 16.7 / 16.6 | 19.3 / 19.7 / 19.2 |
| DOM | pivot | jumping | 17.4 / 20.0 / 17.6 | 17.9 / 20.7 / 18.3 | 26.7 / 30.3 / 27.5 |

A first round of runs had the grids draw every column and ask for rows on every
scroll step. Both grids were slower, and the DOM grid fell to 30 to 40 frames per
second. With columns windowed to the visible ones, and rows requested only when
the visible range leaves what has been fetched, both grids sustain about 60 frames
per second in most cases. The difference is now in how much main-thread time each
frame takes.

## Discussion

The canvas grid wins on every view and mode. The margin is largest on the pivot,
about two times, and smallest when jumping through the flat view, about 1.1 times.
Main-thread time per frame is the headroom a slower desktop has before frames
drop, so that margin matters more on a VDI than on this host.

The 8 ms main-thread budget in the [roadmap](roadmap.md#performance-budgets)
applies to the target desktops, which a four-times throttle only approximates.
Under the throttle the canvas grid is near it on the pivot and well over it on
the flat view. The real figure comes from the demo's performance overlay on a
VDI.

The canvas grid still redraws every visible cell whenever the first visible row
changes. Shifting the existing pixels and drawing only the rows that scroll into
view is the next optimization, and it should benefit smooth scrolling most.

The DOM grid keeps native text selection and screen-reader access, which the
canvas grid has to provide separately. Screen-reader support is not a
first-milestone requirement, so it does not change the decision. The DOM grid
was removed in step 5 of the first milestone, once the canvas grid had cell
selection and copying of its own.

## Repeat the measurement

The comparison needs the DOM grid, which is gone from the current code. Repeat
it from commit `b255e25`, the last with both grids:

```sh
git checkout b255e25
cargo build --release -p parallax-server
cd web
pnpm install --frozen-lockfile
pnpm build:demo
pnpm experiment
```

The results are written to `web/test-results/grid-experiment.json`.

The same method measures the current canvas grid alone. From `web/`, after
`pnpm build:demo`, run `pnpm performance`; the results go to
`web/test-results/grid-performance.json`.
