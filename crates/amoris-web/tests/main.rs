//! amoris-web's integration tests as one binary (docs/spec/checks.md 6.2): each file under
//! `tests/` is a module of it, so a change links the crate's tests once, not once per file.

mod snapshot_restore;
mod water_async;
mod worker;
mod worker_players;
