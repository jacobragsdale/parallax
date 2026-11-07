#!/usr/bin/env bash
# Spike 0.3: builds the measurement pages into web/spike/dist. Run from web/.
# CARGO_BUILD_JOBS=1 keeps rustc within a small machine's memory.
set -euo pipefail
cd "$(dirname "$0")/.."
target="${CARGO_TARGET_DIR:-../target}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
# The data generator first: both builds write the same file under target/.
cargo build --release -p parallax-wasm --features datagen --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir spike/dist/datagen --out-name datagen_wasm "$target/wasm32-unknown-unknown/release/parallax_wasm.wasm"
# The engine as an app would ship it, with export and without the generator.
cargo build --release -p parallax-wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir projects/parallax/wasm --out-name parallax_wasm "$target/wasm32-unknown-unknown/release/parallax_wasm.wasm"
npx tsc -p projects/parallax/worker/tsconfig.json
node spike/build.mjs
