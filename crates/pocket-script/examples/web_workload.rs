//! The script host's web report (`pocket_script::web::report`) as a WebAssembly module with no
//! imports: the compiled workload goes in through `alloc` and `run`, the report comes out through
//! `report_ptr`. Built with
//! `cargo build -p pocket-script --example web_workload --target wasm32-unknown-unknown --release`
//! and run by `node crates/pocket-script/tests/web/run.mjs <the .wasm> <set.json> [native report]`.

use std::sync::Mutex;

static INPUT: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static REPORT: Mutex<String> = Mutex::new(String::new());

/// The ticks the report runs; the native test uses the same (tests/web.rs).
const TICKS: u64 = 600;

/// Room for `len` bytes of input; returns its address.
#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut input = INPUT.lock().unwrap_or_else(|e| e.into_inner());
    *input = vec![0; len];
    input.as_mut_ptr()
}

/// Runs the report over the input (the compiled set as JSON); returns the report's length.
#[unsafe(no_mangle)]
pub extern "C" fn run() -> usize {
    let input = std::mem::take(&mut *INPUT.lock().unwrap_or_else(|e| e.into_inner()));
    let json = String::from_utf8(input).unwrap_or_default();
    let r = pocket_script::web::report(&json, TICKS);
    let len = r.len();
    *REPORT.lock().unwrap_or_else(|e| e.into_inner()) = r;
    len
}

/// The address of the report `run` produced.
#[unsafe(no_mangle)]
pub extern "C" fn report_ptr() -> *const u8 {
    REPORT.lock().unwrap_or_else(|e| e.into_inner()).as_ptr()
}
