//! Perception, actions, affordances and time modes (shared/contract/; architecture.md 4.5).
//!
//! Slice 1 built what the runtime's loop needs: the time model's answer at each boundary
//! ([`Pace`], threads.md 3.3) for stepped pacing and real time with pauses. Slice 2 adds
//! [`perception`] with its [`projection`]s, the [`action`] layer with the sailing intents, and the
//! rest of the time modes in [`time`]; their decisions are docs/spec/perception-slice2.md,
//! actions-slice2.md and time-slice2.md.

#![deny(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

pub mod action;
pub mod perception;
pub mod projection;
pub mod time;

pub use time::{MAX_CATCH_UP, MAX_SPEED, Pace, Pacing, TimeModel};
