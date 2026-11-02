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

## How changes land

1. Work on a branch named for the milestone step, for example
   `m1/engine-core`.
2. For engine behaviour, write the test first. Prefer a differential test against
   DuckDB over hand-written expected values. See
   [How the guarantees are tested](docs/design.md#how-the-guarantees-are-tested).
3. Open a pull request. CI must pass, and the change gets a code review before
   it merges.
4. A change is finished when it is merged to `main`.

## Dependencies

Only permissive licences are allowed. The list is in `deny.toml`, and
`cargo deny check` enforces it in CI. Add a licence to that list only if it is
permissive, and explain the addition in the pull request.
