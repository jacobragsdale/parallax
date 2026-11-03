# The grid experiment: canvas or DOM

parallax's grid draws on a Canvas 2D surface. A virtualized DOM grid, the approach
Perspective takes, spent 1.5 to 2.7 times as much main-thread time per frame on the
same views, and fell to 30 to 40 frames per second where the canvas grid held
about 60. This page records how that was measured, so the result can be repeated
on the Citrix desktops the decision is for.

## Method

Both grids share everything except drawing: the protocol client, the frame
decoder, the formatter, virtual scrolling, and the rule that at most one viewport
request per view is in flight. The canvas grid redraws only the cells whose text
changed. The DOM grid renders positioned row elements through Angular's
templates.

The Playwright suite in `web/e2e/grid-experiment.spec.ts` runs against the
reference server with `examples/experiment.toml`, serving 200,000 generated
positions and 1,000,000 generated trades.

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

Three runs on the build host on 2026-11-02 agreed within about 5%. Each cell
lists the three runs.

| Grid | View | Scroll | Main thread ms per frame | Mean frame ms | p95 frame ms |
|---|---|---|---|---|---|
| canvas | trades | smooth | 17.4 / 16.7 / 16.9 | 18.1 / 17.3 / 17.6 | 21.2 / 20.6 / 20.9 |
| canvas | trades | jumping | 15.7 / 15.7 / 15.9 | 17.1 / 17.2 / 17.4 | 30.3 / 30.7 / 31.0 |
| canvas | pivot | smooth | 11.1 / 10.5 / 10.7 | 16.5 / 16.6 / 16.6 | 17.6 / 17.3 / 17.3 |
| canvas | pivot | jumping | 12.5 / 11.6 / 11.6 | 16.6 / 16.6 / 16.6 | 17.9 / 17.4 / 17.4 |
| DOM | trades | smooth | 29.5 / 28.9 / 29.3 | 29.9 / 29.3 / 29.6 | 37.8 / 34.6 / 35.8 |
| DOM | trades | jumping | 24.2 / 24.0 / 24.2 | 24.5 / 24.4 / 24.6 | 33.1 / 30.7 / 31.2 |
| DOM | pivot | smooth | 17.5 / 17.6 / 16.9 | 17.9 / 18.0 / 17.4 | 21.8 / 22.1 / 22.2 |
| DOM | pivot | jumping | 31.7 / 32.5 / 31.9 | 32.3 / 33.0 / 32.5 | 40.5 / 42.9 / 41.4 |

The DOM grid also spent 2 to 10 ms per frame on layout and style, which the
canvas grid does not incur at all.

## Discussion

The canvas grid wins on every view and mode, most clearly where many cells change
at once: a wide flat view, or jumps through a pivot.

Neither grid meets the 8 ms main-thread budget under a four-times throttle. That
budget applies to the target desktops, which the throttle only approximates. The
real figure comes from the demo's performance overlay on a VDI.

The canvas grid still redraws every visible cell whenever the first visible row
changes. Shifting the existing pixels and drawing only the rows that scroll into
view is the next optimization, and it should benefit smooth scrolling most.

The DOM grid keeps native text selection and screen-reader access, which the
canvas grid has to provide separately. Screen-reader support is not a
first-milestone requirement, so it does not change the decision. The DOM grid
stays in the library for the comparison until the canvas grid has the features
the milestone needs.

## Repeat the measurement

```sh
cargo build --release -p parallax-server
cd web
pnpm build:demo
pnpm experiment
```

The results are written to `web/test-results/grid-experiment.json`.
