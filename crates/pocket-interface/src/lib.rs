//! Perception, actions, affordances and time modes (shared/contract/; architecture.md 4.5).
//!
//! Slice 1 builds only what the runtime's loop needs: the time model's answer at each boundary
//! ([`Pace`], threads.md 3.3) for stepped pacing and real time with pauses. Perception, actions and
//! the rest of the time modes are slice 2.

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
