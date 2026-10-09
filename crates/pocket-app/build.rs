//! The engine's identity for `EngineVersion` (docs/spec/architecture.md 4.13, versions.md 3.1),
//! computed by `crates/engine_version.rs`, which `pocket-web`'s build script runs too, so both
//! builds of a tree report one source.

#[path = "../engine_version.rs"]
mod engine_version;

fn main() {
    engine_version::emit();
}
