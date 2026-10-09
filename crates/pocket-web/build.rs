//! The engine's identity for `EngineVersion`, computed by `pocket-app`'s own file so the native
//! and the web build of one tree report one source (docs/spec/versions.md 3.1 and V9).

#[path = "../pocket-app/build/engine_version.rs"]
mod engine_version;

fn main() {
    engine_version::emit();
}
