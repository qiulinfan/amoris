//! The browser entry points (docs/spec/architecture.md 4.14; threads.md 7): one WebAssembly module
//! carrying both roles of the browser-local run form. The page compiles it once and instantiates
//! the presenter role ([`bindings::Presenter`]); a module Web Worker instantiates the same compiled
//! module and runs the game role ([`bindings::GameWorker`]). They share no memory: snapshots cross
//! as transferred buffers, at most two in flight, and commands and replies as messages, so the form
//! needs no cross-origin isolation (threads.md 7.5).
//!
//! - [`package`]: the web package, a project with its scripts compiled natively (the shipped module
//!   has no transpiler, architecture.md 8.3), and the [`GameSetup`] built from it.
//! - [`worker`]: the game role's loop, target-independent so native tests drive it with their own
//!   clock: boundaries in the canonical order, held commands, the controls, the pacing, publication
//!   with flow control, and every tick's world hash streamed beside the snapshots.
//! - [`presenter`]: the page's side: every snapshot rebuilt from its bytes and checked against the
//!   worker's world hash, the publication order checked, and a view of the named bodies.
//! - [`bindings`]: the `wasm-bindgen` exports and the injected clock (`performance.now()`).
//!
//! [`GameSetup`]: pocket_runtime::GameSetup

// A tick-code neighbour: the loop decides which boundary a command reaches, so it is held to the
// numeric rules of the crates it drives (numeric.md 8).
#![deny(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

pub mod bindings;
pub mod package;
pub mod presenter;
pub mod worker;

/// The time model of every loop, the game thread's (threads.md 3.3; the copy that stood here until
/// 2026-10-09 went when the runtime exported it on every target, threads-slice1.md 13).
pub use pocket_runtime::{MAX_CATCH_UP, MAX_SPEED, Pace, Pacing, TimeModel};
