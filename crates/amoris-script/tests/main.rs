//! amoris-script's integration tests as one binary (docs/spec/checks.md 6.2): each file under
//! `tests/` is a module of it, so a change links the crate's tests once, not once per file.

mod common;

mod budget;
mod depth;
mod errors;
mod hot_update;
mod intents;
mod limits;
mod lint;
mod lockdown;
mod prelude;
mod profile;
mod reload;
mod rules;
mod slot_rules;
mod smoke;
mod web;
mod writes;
