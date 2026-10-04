//! In-process checks (docs/spec/checks.md 8; charter 3.7; architecture.md 4.9): determinism, fork
//! consistency, replay and reload equivalence over one project, each run per seed of its
//! `check.toml`, with the inputs it names, on `Game` driven synchronously; and the lint of `types`.
//! `pocket check <project>` prints their [`Report`]; `cargo xtask check` merges the rows.
//!
//! The variants that need another process (determinism across processes, a replay verified in a
//! fresh one) ask a [`Child`] the caller supplies: `pocket` runs itself again, a test answers in
//! process.

pub mod config;
pub mod determinism;
pub mod fork;
pub mod reload;
pub mod replay;
pub mod report;
pub mod runs;
pub mod types;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pocket_contract::{Problem, detail};
use serde_json::json;

pub use config::{CheckToml, Input, Inputs};
pub use pocket_persist::replay::TickRef;
pub use pocket_persist::{Snapshot, WorldHash};
pub use replay::{VerifySummary, verify_bytes};
pub use report::{Measurement, Report, StepResult, Verdict, exit_code};
pub use runs::{Build, Chain, Subject};

/// The checks `pocket check` runs, in order.
pub const CHECKS: &[&str] = &["types", "determinism", "fork", "replay", "reload"];

/// What a check asks of another process.
#[derive(Clone, Debug, PartialEq)]
pub enum ChildRequest {
    /// The hash chain of a run of the seed (`pocket check --chain-only`).
    Chain { seed: u64 },
    /// The world after tick `tick` of a run of the seed (`--snapshot-at`).
    Snapshot { seed: u64, tick: u64 },
    /// Verify a replay file (`pocket replay --verify`).
    Verify { replay: PathBuf },
}

/// What it answered.
#[derive(Clone, Debug)]
pub enum ChildAnswer {
    Chain(Chain),
    Snapshot(Snapshot),
    Verify(Box<VerifySummary>),
}

/// Another process, asked about the project under check.
pub type Child = Arc<dyn Fn(&Subject, &ChildRequest) -> Result<ChildAnswer, Problem> + Send + Sync>;

/// How `pocket check` runs.
#[derive(Clone, Default)]
pub struct Options {
    /// The seeds to run instead of `check.toml`'s.
    pub seeds: Option<Vec<u64>>,
    /// The checks to run (default all of [`CHECKS`]).
    pub only: Option<Vec<String>>,
    pub child: Option<Child>,
    /// Where replay files go (default `out/check/replays`).
    pub out_dir: Option<PathBuf>,
    /// The command line, for the report.
    pub command: String,
    /// The caller's clock in milliseconds, for durations (none on a target without one).
    pub clock: Option<Arc<dyn Fn() -> f64 + Send + Sync>>,
}

impl Options {
    fn wants(&self, check: &str) -> bool {
        self.only
            .as_ref()
            .is_none_or(|o| o.iter().any(|c| c == check))
    }

    fn now(&self) -> f64 {
        self.clock.as_ref().map_or(0.0, |c| c())
    }
}

/// `check.usage`: an unknown check name, with the nearest one.
pub fn unknown_check(name: &str) -> Problem {
    let suggestions = pocket_contract::suggest_names(name, CHECKS.iter().copied());
    Problem::new(
        "check.usage",
        format!(
            "There is no check '{name}'; the checks are {CHECKS:?}; did you mean {suggestions:?}?"
        ),
        detail([("check", json!(name)), ("suggestions", json!(suggestions))]),
    )
}

/// Runs the checks a subject takes part in (all but `types`, which reads the project's files).
pub fn check_subject(subject: &Subject, opts: &Options) -> Vec<StepResult> {
    let seeds = opts
        .seeds
        .clone()
        .unwrap_or_else(|| subject.config.run.seeds.clone());
    let dir = opts
        .out_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("out/check/replays"));
    let child = opts.child.as_ref();
    let mut steps = Vec::new();
    for &check in &CHECKS[1..] {
        if !opts.wants(check) {
            continue;
        }
        let t0 = opts.now();
        let mut step = match check {
            "determinism" => determinism::check(subject, &seeds, child),
            "fork" => fork::check(subject, &seeds),
            "replay" => replay::check(subject, &seeds, &dir, child),
            _ => reload::check(subject, &seeds),
        };
        step.duration_ms = (opts.now() - t0).max(0.0) as u64;
        steps.push(step);
    }
    steps
}

/// `pocket check <project>`: the project's checks as one report.
pub fn check_project(dir: &Path, opts: &Options) -> Report {
    let t0 = opts.now();
    let name = dir.display().to_string().replace('\\', "/");
    let mut steps = Vec::new();
    if opts.wants("types") {
        let t = opts.now();
        let mut step = types::check(dir, &name);
        step.duration_ms = (opts.now() - t).max(0.0) as u64;
        steps.push(step);
    }
    let rest: Vec<&str> = CHECKS[1..]
        .iter()
        .copied()
        .filter(|c| opts.wants(c))
        .collect();
    if !rest.is_empty() {
        match load(dir) {
            Ok(subject) => steps.extend(check_subject(&subject, opts)),
            Err(p) => {
                for c in rest {
                    let mut step = StepResult::new(c);
                    step.summary = format!("{name} did not load");
                    step.error(p.clone());
                    steps.push(step);
                }
            }
        }
    }
    Report::of(&opts.command, steps, (opts.now() - t0).max(0.0) as u64)
}

#[cfg(feature = "native")]
fn load(dir: &Path) -> Result<Subject, Problem> {
    Subject::load(dir)
}

#[cfg(not(feature = "native"))]
fn load(_dir: &Path) -> Result<Subject, Problem> {
    Err(Problem::new(
        "scripts.transpile_unavailable",
        "This build cannot compile a project's TypeScript; give the checks a compiled setup.",
        detail([]),
    ))
}
