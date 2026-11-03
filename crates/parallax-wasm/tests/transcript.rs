//! The protocol conversation, run as WebAssembly. It must match the native build.

use parallax_session::testing::{fingerprint, transcript};
use wasm_bindgen_test::wasm_bindgen_test;

/// The same value as `TRANSCRIPT_FINGERPRINT` in parallax-session's native tests.
const NATIVE_FINGERPRINT: u64 = 11_268_558_718_760_388_314;

#[wasm_bindgen_test]
fn webassembly_answers_like_the_native_build() {
    assert_eq!(fingerprint(&transcript()), NATIVE_FINGERPRINT);
}
