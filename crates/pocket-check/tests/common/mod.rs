//! Helpers the check tests share: projects of the repository as subjects, a child that answers in
//! process, and a thread with the stack scripts need.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pocket_check::{Child, ChildAnswer, ChildRequest, Subject, runs, verify_bytes};
use pocket_contract::Problem;

pub fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A project of the repository, loaded as `pocket check` loads it.
pub fn subject(rel: &str) -> Subject {
    Subject::load(&repo().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e:#?}"))
}

/// A child that answers in this process with a fresh game: the requests' shapes without a second
/// process (the cross-process controls run the test binary itself).
pub fn in_process() -> Child {
    Arc::new(
        |s: &Subject, req: &ChildRequest| -> Result<ChildAnswer, Problem> {
            Ok(match req {
                ChildRequest::Chain { seed } => {
                    ChildAnswer::Chain(runs::chain(s, *seed, s.ticks())?)
                }
                ChildRequest::Snapshot { seed, tick } => {
                    ChildAnswer::Snapshot(runs::run_to(s, *seed, *tick)?.snapshot()?)
                }
                ChildRequest::Verify { replay } => {
                    let bytes = std::fs::read(replay).map_err(|e| {
                        Problem::new(
                            "replay.unreadable",
                            e.to_string(),
                            pocket_contract::detail([]),
                        )
                    })?;
                    ChildAnswer::Verify(Box::new(verify_bytes(&bytes)?))
                }
            })
        },
    )
}

/// A scratch directory for replay files, under the target directory.
pub fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Runs `f` on a thread whose stack holds the script host's limit.
pub fn big_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(pocket_runtime::GAME_STACK_BYTES)
        .spawn(f)
        .expect("a thread")
        .join()
        .unwrap_or_else(|p| std::panic::resume_unwind(p))
}
