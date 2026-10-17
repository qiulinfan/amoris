//! The rule workload in WebAssembly (docs/bench/script.md, `web.tick.rules`): the worlds of
//! `rules_bench/workload.rs` behind exports, so a JavaScript host times each tick with its own
//! `performance.now()` (`bench/rules/web/bench.mjs`, under Node and in a browser's Web Worker).
//! Its one import, `bench.now`, is that clock, for the step hooks and the script calls' phases.
//!
//! ```text
//! cargo build -p pocket-script --example rules_web --target wasm32-unknown-unknown --profile web
//! ```
//!
//! The compiled sets come from `rules_bench --emit DIR` (the web build has no transpiler).

use std::cell::RefCell;

#[path = "rules_bench/workload.rs"]
mod workload;

use workload::{Bench, Sample};

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "bench")]
unsafe extern "C" {
    /// `performance.now()`, in milliseconds.
    fn now() -> f64;
}

#[cfg(target_arch = "wasm32")]
fn now_ms() -> f64 {
    // SAFETY: an import with no arguments that returns a number.
    unsafe { now() }
}

/// Natively (where `cargo test` builds every example) the module has no host to import from.
#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> f64 {
    0.0
}

thread_local! {
    static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static BENCH: RefCell<Option<Bench>> = const { RefCell::new(None) };
    static ERROR: RefCell<String> = const { RefCell::new(String::new()) };
    static SAMPLE: RefCell<[f64; 7]> = const { RefCell::new([0.0; 7]) };
    static VERIFIED: RefCell<(u64, u64)> = const { RefCell::new((0, 0)) };
}

/// Room for `len` bytes of input (a compiled set as JSON); returns its address.
#[unsafe(no_mangle)]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    INPUT.with_borrow_mut(|input| {
        *input = vec![0; len];
        input.as_mut_ptr()
    })
}

/// Records an error for `error_ptr` and returns its length (never 0).
fn fail(e: String) -> usize {
    ERROR.with_borrow_mut(|err| {
        *err = e;
        err.len().max(1)
    })
}

/// The last error's address; `fail` returned its length, which `error_len` gives too.
#[unsafe(no_mangle)]
pub extern "C" fn error_ptr() -> *const u8 {
    ERROR.with_borrow(|err| err.as_ptr())
}

/// The last error's length in bytes, for an export that returns no length (`verify`).
#[unsafe(no_mangle)]
pub extern "C" fn error_len() -> usize {
    ERROR.with_borrow(String::len)
}

fn input_set() -> Result<pocket_script::CompiledSet, String> {
    let json = INPUT.with_borrow_mut(std::mem::take);
    let json = String::from_utf8(json).map_err(|e| e.to_string())?;
    pocket_script::CompiledSet::from_json(&json).map_err(|e| e.message)
}

/// A fresh world: the twin (`probe` 3), or the script host with the input's set, timing `damage`
/// (`probe` 0), `idle` (1) or `scan` (2). Returns 0, or an error's length.
#[unsafe(no_mangle)]
pub extern "C" fn init(probe: u32) -> usize {
    let made = match probe {
        3 => Bench::twin(now_ms),
        0..=2 => input_set().and_then(|set| {
            let name = ["damage", "idle", "scan"][probe as usize];
            Bench::script(&set, name, now_ms)
        }),
        _ => Err(format!("no probe {probe}")),
    };
    match made {
        Ok(b) => {
            BENCH.with_borrow_mut(|slot| *slot = Some(b));
            0
        }
        Err(e) => fail(e),
    }
}

/// One tick (`Sim::step` alone, so the caller's clock around this call times the tick). Returns
/// 0, or an error's length.
#[unsafe(no_mangle)]
pub extern "C" fn tick() -> usize {
    let r = BENCH.with_borrow_mut(|b| match b {
        Some(b) => b.tick(),
        None => Err("no world: call init first".into()),
    });
    match r {
        Ok(()) => 0,
        Err(e) => fail(e),
    }
}

/// What the last tick did, as seven doubles at the returned address: `script.update` (or the
/// twin's systems), the measured system, prepare, run, diff, apply (ms) and steps.
#[unsafe(no_mangle)]
pub extern "C" fn sample() -> *const f64 {
    let s: Sample = BENCH.with_borrow(|b| b.as_ref().map(Bench::sample).unwrap_or_default());
    SAMPLE.with_borrow_mut(|out| {
        #[allow(clippy::cast_precision_loss)] // steps per tick, far below 2^53
        let steps = s.steps as f64;
        *out = [
            s.update,
            s.system,
            s.phases.prepare,
            s.phases.run,
            s.phases.diff,
            s.phases.apply,
            steps,
        ];
        out.as_ptr()
    })
}

/// The world's digest after the last tick.
#[unsafe(no_mangle)]
pub extern "C" fn digest() -> u64 {
    BENCH.with_borrow(|b| b.as_ref().map_or(0, Bench::digest))
}

/// Runs the input's set (the `rules` bundle) and the twin side by side for `ticks` ticks,
/// comparing their digests at every tick: returns 0 when all agree, else the first tick that
/// differs (or `u32::MAX` on an error, whose message `error_ptr` and `error_len` give).
/// `verified_final` and `verified_chain` then give the last digest and the chain of all of them,
/// as `rules_bench --emit` writes them natively.
#[unsafe(no_mangle)]
pub extern "C" fn verify(ticks: u32) -> u32 {
    let run = || -> Result<u32, String> {
        let set = input_set()?;
        let mut script = Bench::script(&set, "damage", now_ms)?;
        let mut twin = Bench::twin(now_ms)?;
        let mut chain = 0xcbf2_9ce4_8422_2325u64;
        for t in 1..=ticks {
            script.tick()?;
            twin.tick()?;
            let a = script.digest();
            if a != twin.digest() {
                return Ok(t);
            }
            chain = (chain ^ a).wrapping_mul(0x0000_0100_0000_01b3);
        }
        VERIFIED.with_borrow_mut(|v| *v = (script.digest(), chain));
        Ok(0)
    };
    run().unwrap_or_else(|e| {
        fail(e);
        u32::MAX
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn verified_final() -> u64 {
    VERIFIED.with_borrow(|v| v.0)
}

#[unsafe(no_mangle)]
pub extern "C" fn verified_chain() -> u64 {
    VERIFIED.with_borrow(|v| v.1)
}
