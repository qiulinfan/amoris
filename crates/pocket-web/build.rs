//! The engine's identity for `EngineVersion`, computed by `crates/engine_version.rs`, which
//! `pocket-app`'s build script runs too, so the native and the web build of one tree report one
//! source (docs/spec/versions.md 3.1 and V9; architecture.md 4.13).

#[path = "../engine_version.rs"]
mod engine_version;

fn main() {
    engine_version::emit();
}
