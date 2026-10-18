//! The shallow-water spike's bench scene (`shallow::bench_scene`) as a WebAssembly module with no
//! imports: `tests/web/shallow.mjs` times its substeps and prints the hash chain `shallow_bench`
//! prints natively (docs/spikes/shallow-water.md).
//!
//! `cargo build -p amoris-physics --example shallow_web --target wasm32-unknown-unknown --release`,
//! then `node crates/amoris-physics/tests/web/shallow.mjs <the .wasm> [N ...]`.

use std::cell::RefCell;

use amoris_physics::shallow::{Scheme, Shallow, bench_scene};

const DT: f64 = 1.0 / 120.0;

thread_local! {
    static STATE: RefCell<Option<Shallow>> = const { RefCell::new(None) };
}

/// Makes the `n` by `n` scene, moved by the pipes (`pipes` 1) or advected (0).
#[unsafe(no_mangle)]
pub extern "C" fn setup(n: u32, pipes: u32) {
    let n = usize::try_from(n).unwrap_or(0);
    let mut w = bench_scene(n);
    w.scheme = if pipes == 1 {
        Scheme::Pipes
    } else {
        Scheme::Advect
    };
    STATE.with(|s| *s.borrow_mut() = Some(w));
}

/// Takes `count` substeps of 1/120 s.
#[unsafe(no_mangle)]
pub extern "C" fn run(count: u32) {
    STATE.with(|s| {
        if let Some(w) = s.borrow_mut().as_mut() {
            w.step(DT * f64::from(count), count);
        }
    });
}

/// The state's hash, its high (`high` 1) or low 32 bits.
#[unsafe(no_mangle)]
pub extern "C" fn hash(high: u32) -> u32 {
    STATE.with(|s| {
        s.borrow().as_ref().map_or(0, |w| {
            let h = w.hash();
            let part = if high == 1 { h >> 32 } else { h & 0xffff_ffff };
            u32::try_from(part).unwrap_or(0)
        })
    })
}
