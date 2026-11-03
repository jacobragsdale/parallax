# Contributing to parallax

## Build and test

Prerequisites:

- [rustup](https://rustup.rs). `rust-toolchain.toml` pins Rust 1.99 with
  `clippy`, `rustfmt` and the `wasm32-unknown-unknown` target. rustup installs
  them on the first `cargo` command, or when you run `rustup toolchain install`.
- [cargo-deny](https://github.com/EmbarkStudios/cargo-deny), for the dependency
  checks.

Run the same checks as CI:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check licenses bans sources advisories
```

On a machine with little free memory, set `CARGO_BUILD_JOBS=1`. Parallel
`rustc` processes can exhaust memory and get killed by an out-of-memory guard
such as earlyoom.

The WebAssembly tests run in Node. They need `wasm-bindgen-test-runner` from
[wasm-bindgen's releases](https://github.com/wasm-bindgen/wasm-bindgen/releases),
at the same version as `wasm-bindgen` in `Cargo.lock`. `.cargo/config.toml`
makes it the runner for the `wasm32-unknown-unknown` target:

```sh
cargo test -p parallax-wasm --target wasm32-unknown-unknown
```

That test replays the protocol conversation in `parallax-session`'s `testing`
module and checks its fingerprint against the native build's. The browser engine
must answer byte for byte like the server.

## Run the server

`examples/parallax.toml` serves generated positions, trades and prices on
`127.0.0.1:8790`:

```sh
cargo run --release -p parallax-server -- --config examples/parallax.toml
```

The protocol runs on `/ws`. `POST /invalidate/DATASET` reloads every cached
table of `DATASET` and pushes fresh frames to open views. `GET /health` answers
`ok`.

## Generate test data

`parallax-datagen` writes reproducible synthetic finance data as Arrow IPC
files. The same seed and options always produce the same values, whatever the
batch size or thread count. To write the milestone 1 datasets:

```sh
cargo build --release -p parallax-datagen
./target/release/parallax-datagen positions --out data/positions.arrow
./target/release/parallax-datagen trades --out data/trades.arrow
./target/release/parallax-datagen prices --out data/prices.arrow
```

The defaults give 5 million positions, 20 million trades, and daily bars for 500
equities from 2016-01-04 to 2026-10-30. Add `--hostile` for nulls, NaN,
infinities, negative zero, extreme magnitudes, Unicode and blank tickers, and
wall-clock times that daylight-saving changes make ambiguous or impossible. Add
`--duplicate-key-rate 0.01` for repeated primary keys. Run
`parallax-datagen <dataset> --help` for every option. `data/` is ignored by git.

The `golden_rows_are_stable` tests lock the generator's output with hashes. If a
change alters generated values on purpose, update the hashes in the same pull
request and say so in its description. Any stored expectation built from
generated data becomes invalid at that point.

### TA-Lib fixtures

The indicator tests compare the engine with TA-Lib's output, stored in
`crates/parallax-engine/tests/fixtures/talib.json`. The script beside it writes
that file from generated bars with TA-Lib's Python wheel, which bundles the C
library; Python is used for nothing else. To regenerate it after changing the
bars or the parameters it covers, run from that directory:

```sh
uv run generate_talib.py
```

The script pins TA-Lib and NumPy, and the test checks the TA-Lib version
recorded in the file.

## How changes land

1. Work on a branch named for the milestone step, for example
   `m1/engine-core`.
2. For engine behaviour, write the test first. Prefer a differential test against
   DuckDB over hand-written expected values. See
   [How the guarantees are tested](docs/design.md#how-the-guarantees-are-tested).
3. Open a pull request. CI must pass, and the change gets a code review before
   it merges.
4. A change is finished when it is merged to `main`.

## Release

A release publishes the `parallax-*` crates to crates.io and
`@jacobragsdale/parallax`, with its WebAssembly worker, to npm. Publishing
cannot be undone, so it is done by hand by the maintainer, never by CI. From a
clean checkout of `main` with CI green:

1. Set the version in `Cargo.toml` (`[workspace.package]` and the
   `[workspace.dependencies]` entries of the `parallax-*` crates) and in
   `web/projects/parallax/package.json`.
2. Check that every crate packages:

   ```sh
   cargo package --workspace
   ```

   Dev-dependencies on other `parallax-*` crates name only a path, so
   publishing drops them; the generator and the engine depend on each other
   only through tests.
3. Build the library and its worker, and look at what npm would receive:

   ```sh
   cd web
   pnpm build:wasm && pnpm build
   cd dist/parallax && npm pack --dry-run
   ```

4. Publish the crates (`cargo publish --workspace`), then the package
   (`npm publish --access public` from `web/dist/parallax`), and tag the
   commit `v<version>`.

## Dependencies

Only permissive licences are allowed. The list is in `deny.toml`, and
`cargo deny check` enforces it in CI. Add a licence to that list only if it is
permissive, and explain the addition in the pull request.
