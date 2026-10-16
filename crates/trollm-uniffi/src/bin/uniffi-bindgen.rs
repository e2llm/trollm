//! Local `uniffi-bindgen` CLI shim. UniFFI 0.28 no longer publishes an
//! installable `uniffi-bindgen` crate; the CLI entrypoint is
//! `uniffi::uniffi_bindgen_main()` behind the `cli` feature. Wrapping it here
//! pins bindings generation to the exact UniFFI version this crate compiles
//! against:
//!
//! ```sh
//! cargo run -p trollm-uniffi --bin uniffi-bindgen --features bindgen-cli -- \
//!     generate --library target/debug/libtrollm_uniffi.so \
//!     --language kotlin --out-dir apps/android/app/build/generated/uniffi
//! ```

fn main() {
    uniffi::uniffi_bindgen_main();
}
