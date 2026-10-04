//! The pinned vectors of `pocket-persist` (persistence.md 12, P4: the hash test vectors, the tiny
//! world's snapshot and hash, a run with a fork) as a WebAssembly module with no imports, so they
//! run in a JavaScript engine exactly as natively.
//!
//! Built with `cargo build -p pocket-persist --example web_vectors --target wasm32-unknown-unknown
//! --release` and run by `node crates/pocket-persist/tests/web/run.mjs <the .wasm> [native report]`,
//! which prints the report and exits 1 on a failure or a difference from the native report (written
//! by the `web_report` test to `<target>/tmp/pocket-persist-web-report.txt`).

use std::sync::Mutex;

static REPORT: Mutex<String> = Mutex::new(String::new());

/// Runs the tests and returns the report's length; `report_ptr` gives its address.
#[unsafe(no_mangle)]
pub extern "C" fn run() -> usize {
    let r = pocket_persist::vectors::web_report();
    let len = r.len();
    *REPORT.lock().unwrap_or_else(|e| e.into_inner()) = r;
    len
}

/// The address of the report `run` produced.
#[unsafe(no_mangle)]
pub extern "C" fn report_ptr() -> *const u8 {
    REPORT.lock().unwrap_or_else(|e| e.into_inner()).as_ptr()
}
