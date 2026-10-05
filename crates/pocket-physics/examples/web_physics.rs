//! The web tests of `pocket-physics` (`pocket_physics::WEB_TESTS`: the sailing scene's hash chain
//! against its golden value, forks at every tick, parry's BVH `log2`) and the scene's hash at every
//! tick, as a WebAssembly module with no imports, so they run in a JavaScript engine exactly as
//! natively (docs/spec/numeric.md 3.1 and 5; checks.md 7.2).
//!
//! Built with `cargo build -p pocket-physics --example web_physics --target wasm32-unknown-unknown
//! --release` and run by `node crates/pocket-physics/tests/web/run.mjs <the .wasm> [native report]`,
//! which prints the report and exits 1 on a failure or a difference from the native report
//! (`pocket_physics::web_report`, which the `web_report` test writes to
//! `<target>/tmp/pocket-physics-web-report.txt`).

use std::sync::Mutex;

static REPORT: Mutex<String> = Mutex::new(String::new());

/// Runs the tests and returns the report's length; `report_ptr` gives its address.
#[unsafe(no_mangle)]
pub extern "C" fn run() -> usize {
    let r = pocket_physics::web_report();
    let len = r.len();
    *REPORT.lock().unwrap_or_else(|e| e.into_inner()) = r;
    len
}

/// The address of the report `run` produced.
#[unsafe(no_mangle)]
pub extern "C" fn report_ptr() -> *const u8 {
    REPORT.lock().unwrap_or_else(|e| e.into_inner()).as_ptr()
}
