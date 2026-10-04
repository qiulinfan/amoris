//! TypeScript on QuickJS-ng: the script host (docs/spec/script-host.md, script-sandbox.md,
//! hot-update.md; architecture.md 4.6).
//!
//! - `compile` (feature `transpile`, oxc in process) turns a project's TypeScript into a
//!   `CompiledSet`: modules linted against module-level state, types stripped, `**` lowered to
//!   `Math.pow`, a source map each, and the harden epilogue appended.
//! - `ScriptHost::instantiate` makes a QuickJS-ng context, locks it down (removed and replaced
//!   globals, `Math` over `pocket_sim::math`, a deep freeze) and evaluates the modules into a
//!   `Program` under the load budget.
//! - `Program::run_system` runs one system as a transaction: typed-array columns sorted by entity
//!   id, writes checked and staged, applied all or nothing, under a step budget counted in work and
//!   a call depth counted in calls (the vendored QuickJS-ng's P1 and P5).
//! - `scripts` puts a world's program in its schedule (`script.update`) and swaps it at a boundary,
//!   keeping the old one on failure.
//!
//! The host holds no game state and never reads a clock; natives never panic across QuickJS-ng's
//! C frames.

pub mod access;
mod call;
pub(crate) mod caught;
#[cfg(feature = "transpile")]
pub mod compile;
pub(crate) mod convert;
pub mod debug;
mod define;
pub mod error;
pub mod ffi;
mod freeze;
pub mod host;
pub(crate) mod js;
mod loader;
pub(crate) mod natives;
pub mod program;
pub mod resolve;
pub(crate) mod sandbox;
pub mod scripts;
pub mod source;
pub mod sourcemap;
pub mod types;
pub mod web;

pub use access::{ComponentAccess, EngineFns, ScriptAccess, register_engine, register_project};
pub use call::CallCounts;
#[cfg(feature = "transpile")]
pub use compile::{CompileOptions, CompileReport, compile, compile_with_report};
pub use error::{ErrorPhase, JsError, ScriptError, ScriptErrorDetail, SourceLocation, StackFrame};
pub use host::{LogLine, ScriptHost, ScriptLimits};
pub use loader::{PRELUDE_JS, PRELUDE_MAP};
pub use program::{
    Program, SystemInfo, SystemKind, SystemOutcome, SystemStats, TickBudget, TickInfo,
};
pub use scripts::{Scripts, SwapOutcome, SwapReport, install, swap};
pub use source::{Bundle, CompiledModule, CompiledSet, ScriptSource};

/// The C compiler that built QuickJS-ng (P7): `EngineVersion.c_compiler`.
pub const QJS_CC: &str = ffi::QJS_CC;

/// The one import QuickJS-ng's wasm32 shim needs (the clock behind `Date` and `performance`, which
/// the sandbox leaves out): defined here so the module imports nothing from `env` (script-web
/// spike, Recipe 3). It returns 0; no script sees a clock.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
#[unsafe(no_mangle)]
pub extern "C" fn __rquickjs_host_now_us() -> f64 {
    0.0
}
