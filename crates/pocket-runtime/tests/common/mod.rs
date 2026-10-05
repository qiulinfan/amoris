//! Helpers the runtime's tests share: the sailing sample's setup and a thread with the stack scripts
//! need (script-sandbox.md 4.3).
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use pocket_runtime::{GameSetup, Project};

pub fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The sailing sample, compiled once per test binary.
pub fn sailing() -> Arc<GameSetup> {
    static SETUP: OnceLock<Arc<GameSetup>> = OnceLock::new();
    SETUP
        .get_or_init(|| {
            let p =
                Project::load(&repo().join("samples/sailing")).unwrap_or_else(|e| panic!("{e:#?}"));
            Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")))
        })
        .clone()
}

/// Runs `f` on a thread whose stack holds the script host's limit.
pub fn big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(pocket_script::host::THREAD_STACK_BYTES)
        .spawn(f)
        .expect("a thread")
        .join()
        .unwrap_or_else(|p| std::panic::resume_unwind(p))
}
