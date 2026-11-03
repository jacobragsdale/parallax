//! The protocol conversation, run as WebAssembly. It must match the native build.

use parallax_session::testing::{fingerprint, transcript};
use wasm_bindgen_test::wasm_bindgen_test;

/// The same value as `TRANSCRIPT_FINGERPRINT` in parallax-session's native tests.
const NATIVE_FINGERPRINT: u64 = 14_296_254_853_414_913_943;

#[wasm_bindgen_test]
fn webassembly_answers_like_the_native_build() {
    assert_eq!(fingerprint(&transcript()), NATIVE_FINGERPRINT);
}

/// The bundled time-zone database works in the browser build: New York's 2026
/// spring change, as the native formatter tests show it.
#[wasm_bindgen_test]
fn webassembly_formats_instants_in_new_york() {
    let f = parallax_engine::Formatter::default();
    let before = 1_772_953_199_000_000;
    assert_eq!(f.timestamp(before, true), "2026-03-08 01:59:59");
    assert_eq!(f.timestamp(before + 1_000_000, true), "2026-03-08 03:00:00");
    assert_eq!(f.float(2.675), "2.68");
}
