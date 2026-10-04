//! The web tests of `pocket-sim` (`pocket_sim::WEB_TESTS`: the math sweeps against their golden
//! hashes, the RNG reference vectors, the conversion cases) as a WebAssembly module with no imports,
//! so they run in a JavaScript engine exactly as natively (docs/spec/numeric.md 10; rng.md 10).
//!
//! Built with `cargo build -p pocket-sim --example web_vectors --target wasm32-unknown-unknown
//! --release` and run by `node crates/pocket-sim/tests/web/run.mjs <the .wasm> [native report]`,
//! which prints the report and exits 1 on a failure or a difference from the native report
//! (`pocket_sim::web_report`, which the `web_report` test writes to
//! `<target>/tmp/pocket-sim-web-report.txt`).

use std::sync::Mutex;

static REPORT: Mutex<String> = Mutex::new(String::new());

/// Runs the tests and returns the report's length; `report_ptr` gives its address.
#[unsafe(no_mangle)]
pub extern "C" fn run() -> usize {
    let r = pocket_sim::web_report();
    let len = r.len();
    *REPORT.lock().unwrap_or_else(|e| e.into_inner()) = r;
    len
}

/// The address of the report `run` produced.
#[unsafe(no_mangle)]
pub extern "C" fn report_ptr() -> *const u8 {
    REPORT.lock().unwrap_or_else(|e| e.into_inner()).as_ptr()
}
