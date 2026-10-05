//! The floating-point environment check (docs/spec/numeric.md 4): at the start of every tick, in
//! every build, one register read confirms round to nearest, no flush to zero and no traps. A
//! library or driver that changed the game thread's mode would otherwise change every subnormal
//! result without a sound.

use pocket_contract::{Problem, detail};
use serde_json::json;

/// MXCSR with its six sticky exception flags masked out must equal this on `x86_64`: all
/// exceptions masked, round to nearest, flush-to-zero and denormals-are-zero off.
#[cfg(target_arch = "x86_64")]
pub const EXPECTED_MXCSR: u32 = 0x1F80;

/// Reads MXCSR with `stmxcsr` (the `_mm_getcsr` intrinsic is deprecated).
#[cfg(target_arch = "x86_64")]
pub fn read() -> u64 {
    let mut v: u32 = 0;
    // SAFETY: stmxcsr stores the 32-bit MXCSR register to the given address, a local u32.
    unsafe {
        core::arch::asm!("stmxcsr dword ptr [{0}]", in(reg) &raw mut v, options(nostack, preserves_flags));
    }
    u64::from(v)
}

/// Reads FPCR.
#[cfg(target_arch = "aarch64")]
pub fn read() -> u64 {
    let v: u64;
    // SAFETY: reading FPCR has no side effects.
    unsafe {
        core::arch::asm!("mrs {0}, fpcr", out(reg) v, options(nomem, nostack, preserves_flags));
    }
    v
}

/// WebAssembly has no floating-point control register.
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub fn read() -> u64 {
    0
}

/// `Ok` when the register is the default, else the internal error `number.float_env_changed`.
pub fn check() -> Result<(), Problem> {
    let value = read();
    #[cfg(target_arch = "x86_64")]
    let (register, ok, expected) = (
        "MXCSR",
        (value & !0x3F) == u64::from(EXPECTED_MXCSR),
        "0x1f80",
    );
    #[cfg(target_arch = "aarch64")]
    let (register, ok, expected) = (
        "FPCR",
        value & ((0b11 << 22) | (1 << 24)) == 0,
        "rounding and FZ bits 0",
    );
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    let (register, ok, expected) = ("none", true, "none");
    if ok {
        return Ok(());
    }
    Err(Problem::new(
        "number.float_env_changed",
        format!(
            "The game thread's floating-point mode changed ({register} is {value:#x}, expected \
             {expected}); something the thread called altered it, and results would differ."
        ),
        detail([
            ("register", json!(register)),
            ("value", json!(format!("{value:#x}"))),
            ("expected", json!(expected)),
        ]),
    ))
}

/// Writes MXCSR (tests only: they set flush-to-zero to prove the check trips).
#[cfg(all(test, target_arch = "x86_64"))]
pub(crate) fn write(v: u32) {
    // SAFETY: ldmxcsr loads a 32-bit value into MXCSR; the callers pass valid control words.
    unsafe {
        core::arch::asm!("ldmxcsr dword ptr [{0}]", in(reg) &raw const v, options(nostack, preserves_flags));
    }
}
