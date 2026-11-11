//! `cargo run -p yo-bindings --bin uniffi-bindgen -- generate --library <libyo> --language swift|kotlin --out-dir <dir>`
fn main() {
    uniffi::uniffi_bindgen_main()
}
