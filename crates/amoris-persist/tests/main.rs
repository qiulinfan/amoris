//! amoris-persist's integration tests as one binary (docs/spec/checks.md 6.2): each file under
//! `tests/` is a module of it, so a change links the crate's tests once, not once per file.

mod common;

mod depth;
mod fork;
mod pce;
mod project;
mod repeated;
mod replay;
mod replay_edges;
mod snapshot;
mod versions;
mod web;
