//! The time spent inside the host's natives, for benchmarks (`ScriptHost::time_natives`;
//! docs/bench/script.md, section 8): what a system's `run` spends in the engine's `Math`
//! functions, in `amoris:host`'s natives and in the console, by [`NativeKind`], against the rest,
//! interpretation.
//!
//! A program instantiated while a host times natives gets timed copies of them; others run the
//! untimed natives, which this module never touches. The engine's `Math` functions are C
//! functions QuickJS-ng calls with no context, so the clock and the sums are the thread's: one
//! host runs on a thread at a time. A native that runs JavaScript (a getter of an argument) and
//! through it another native is timed once, as the outer one.

use std::cell::Cell;

use crate::host::{NATIVE_KINDS, NativeKind};

thread_local! {
    static CLOCK: Cell<Option<fn() -> f64>> = const { Cell::new(None) };
    static SUMS: Cell<[(f64, u64); NATIVE_KINDS]> = const { Cell::new([(0.0, 0); NATIVE_KINDS]) };
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// The clock timed natives read on this thread (`None` stops timing).
pub(crate) fn set_clock(clock: Option<fn() -> f64>) {
    CLOCK.set(clock);
}

/// Runs `f`, the body of a native of `kind`, adding its time and one call to this thread's sums.
#[inline]
pub(crate) fn timed<R>(kind: NativeKind, f: impl FnOnce() -> R) -> R {
    let Some(clock) = CLOCK.get() else {
        return f();
    };
    let depth = DEPTH.get();
    DEPTH.set(depth + 1);
    let t0 = if depth == 0 { clock() } else { 0.0 };
    let r = f();
    if depth == 0 {
        let spent = clock() - t0;
        let mut sums = SUMS.get();
        let s = &mut sums[kind as usize];
        s.0 += spent;
        s.1 += 1;
        SUMS.set(sums);
    }
    DEPTH.set(depth);
    r
}

/// This thread's sums so far by kind: the time inside natives, in the clock's unit, and the calls.
pub(crate) fn totals() -> [(f64, u64); NATIVE_KINDS] {
    SUMS.get()
}
