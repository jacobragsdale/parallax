# parallax

parallax is a data grid, pivot table and charting stack for portfolio managers
and quant researchers. It is a Rust engine with an Angular front end, built to
load and pivot tens of millions of rows on GPU-less Citrix desktops without
showing a wrong number.

parallax is in milestone 1 of its [roadmap](docs/roadmap.md). Everything in it
is built: the engine, the server and its connectors (SQL Server, Oracle
Database, Parquet, MongoDB and HTTP), Excel export, and the Angular grid and
pivot, which can run views on the server or in the browser. The performance
budgets and the check on a Citrix desktop remain. Nothing is published to npm
or crates.io yet. [Contributing](CONTRIBUTING.md) shows how to build it and run
the tests.

## Run the demo

The demo shows the grid and pivot over generated positions, trades and prices,
with a performance overlay. It needs the Rust toolchain that
`rust-toolchain.toml` names, the `wasm-bindgen` command-line tool at the version
in `Cargo.lock`, Node.js 24 and pnpm.

1. Build the browser engine and the demo:

   ```sh
   cd web
   pnpm install
   pnpm build:wasm
   pnpm build:demo
   ```

2. Start the reference server, which serves the demo next to the generated
   data, from the repository root:

   ```sh
   cargo run --release -p parallax-server -- --config examples/experiment.toml
   ```

3. Open <http://127.0.0.1:8791>.

The server generates each dataset when a view first opens it; the million
trades take a few seconds. `pnpm e2e`, run from `web`, tests the demo in
Chromium with the GPU off, starting the release server built in step 2 if it is
not already running.

## Documentation

- [How parallax is designed](docs/design.md) explains the architecture and
  the correctness rules, and why each choice was made.
- [Roadmap](docs/roadmap.md) lists the milestones, the build order and the
  performance budgets.
- [Deferred work](docs/future.md) records what was left out, and why.
- [The grid experiment](docs/grid-experiment.md) records why the grid draws on
  a canvas, with the measurements.

## License

Apache License 2.0. See [LICENSE](LICENSE).
