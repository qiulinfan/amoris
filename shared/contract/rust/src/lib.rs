//! The shared contract's types (shared/contract/README.md; docs/spec/architecture.md 4.16): the
//! error protocol, `Problem {code, message, detail}` with one constructor per code of the
//! contract's families, and the refusal of unknown names with "did you mean" suggestions
//! (shared/contract/errors.md).
//!
//! Every place that takes a request checks it before applying anything: [`shape::decode`] (or a
//! cached [`shape::Shape`]) refuses unknown fields, wrong types and out-of-range values from the
//! request type's JSON Schema, and the `codes` constructors with [`suggest`] refuse unknown
//! methods, seats, controls, intents and other names.

pub mod codes;
pub mod pointer;
pub mod problem;
pub mod render;
pub mod shape;
pub mod suggest;

pub use pointer::Pointer;
pub use problem::{Detail, MAX_ALSO, Problem, Use, detail};
pub use shape::{CheckOptions, Checked, Decoded, Shape, decode};
pub use suggest::{Candidate, suggest, suggest_names};
