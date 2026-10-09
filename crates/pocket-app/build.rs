//! The engine's identity for `EngineVersion` (docs/spec/architecture.md 4.13, versions.md 3.1);
//! `pocket-web`'s build script runs the same file, so both builds of a tree report one source.

#[path = "build/engine_version.rs"]
mod engine_version;

fn main() {
    engine_version::emit();
}
