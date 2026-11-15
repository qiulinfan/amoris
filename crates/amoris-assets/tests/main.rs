//! amoris-assets's integration tests as one binary (docs/spec/checks.md 6.2): each file under
//! `tests/` is a module of it, so a change links the crate's tests once, not once per file.

mod morph;
mod water_checked;
mod water_patches;
