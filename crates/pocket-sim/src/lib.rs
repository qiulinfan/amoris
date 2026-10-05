//! The simulation core (docs/spec/simulation.md, numeric.md, rng.md; architecture.md 4.1): the
//! world and its fixed-timestep tick, boundaries and the writes they take, phases and the system
//! order, events delivered one tick later, the hooks the time modes drive, entity ids and their
//! deterministic allocation, iteration in id order, the deterministic math library, numeric
//! conversion, the RNG, the component registry, and the persistence hooks that physics, scripts
//! and persistence build on.
//!
//! Determinism rules hold here by construction and by lint (`clippy.toml`): no wall clock, no
//! hash-map iteration whose order reaches the world, no platform transcendental functions (use
//! [`math`]), no change detection driving logic, no float-to-integer `as` casts (use [`num`]).

#![deny(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

pub mod data;
pub mod entity;
pub mod event;
pub mod math;
pub mod num;
pub mod order;
pub mod persisted;
pub mod registry;
pub mod rng;
pub mod schedule;
pub mod sim;
pub mod time;

pub use data::PlainData;
pub use entity::{EntityAllocator, EntityId, EntityIndex, MAX_ENTITY_ID, Name, SimCommands};
pub use event::{
    Emit, Event, EventCounter, EventInbox, EventKind, EventOutbox, EventSeq, NewEvent,
};
pub use persisted::{ContentHash, Persisted, PersistedCache, RegisterPersisted, Staged};
pub use registry::ComponentRegistry;
pub use rng::{KeyPart, Pcg32, RngTable, WorldSeed};
pub use schedule::{NoHooks, StepHooks, SystemCtx, SystemKey, TickPhase};
pub use sim::{Boundary, DecisionRequest, Sim, SimConfig, StepReport, TickOutput};
pub use time::{RunCondition, SimClock, Tick, TickRate};

/// A web test: its name and the function that runs it.
pub type WebTest = (&'static str, fn() -> Result<(), String>);

/// The web tests of this crate (checks.md 7.2, `tests`): each runs in the WebAssembly build as
/// natively and must pass on both.
pub const WEB_TESTS: &[WebTest] = &[
    ("math.golden", math::sweep::check_golden),
    ("rng.vectors", rng::check_vectors),
    ("number.convert", num::check_conversions),
];

/// The web tests' report: one `ok name` or `FAIL name: why` line per test of [`WEB_TESTS`], then
/// the math sweeps' hashes. The WebAssembly build (`examples/web_vectors.rs`) must produce the same
/// bytes as the native builds.
pub fn web_report() -> String {
    let mut out = String::new();
    for (name, test) in WEB_TESTS {
        match test() {
            Ok(()) => out.push_str(&format!("ok {name}\n")),
            Err(e) => out.push_str(&format!("FAIL {name}: {e}\n")),
        }
    }
    out.push_str(&math::sweep::report());
    out
}
