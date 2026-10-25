//! The steps of `cargo xtask check` and their order (docs/spec/checks.md 4). Every step that can
//! run does run after a failure; a step that needs another one is skipped when that one failed,
//! and says so.

pub mod cargo;
pub mod cross_build;
pub mod deps;
pub mod diag;
pub mod docs;
pub mod fixture;
pub mod generate;
pub mod gpus;
pub mod metadata;
pub mod projects;
pub mod test;
pub mod web;
pub mod web_pack;

use crate::cli::{CheckOptions, STEPS};
use crate::config;
use crate::files;
use crate::report::{Problem, StepResult, Verdict};
use crate::run::Env;
use metadata::Metadata;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Instant;

/// What every step reads, loaded and validated before any step runs, so a malformed
/// configuration file is exit code 2 (checks.md 11).
pub struct Prepared {
    pub listed: Vec<String>,
    pub graph: deps::Graph,
    pub projects: Vec<projects::Project>,
}

pub fn prepare(env: &Env) -> Result<Prepared, Problem> {
    let listed = files::list(&env.root)
        .map_err(|m| Problem::new("check.tool_missing", m, json!({"tool": "git"})))?;
    let graph = deps::load(&env.root)?;
    config::load::<generate::ClippySource>(&env.root, generate::CLIPPY_SOURCE)?;
    let projects = projects::discover(&env.root, &listed)?;
    Ok(Prepared {
        listed,
        graph,
        projects,
    })
}

/// The steps a step needs: it is skipped when one of them failed (checks.md 4).
fn needs(step: &str) -> &'static [&'static str] {
    match step {
        "test" | "gpus" | "types" | "determinism" | "fork" | "replay" | "reload" | "contract" => {
            &["build"]
        }
        "web" => &["build", "wasm", "determinism", "replay"],
        "perf" => &["build", "web"],
        _ => &[],
    }
}

struct Run<'a> {
    env: &'a Env,
    opts: &'a CheckOptions,
    prep: &'a Prepared,
    results: Vec<StepResult>,
    metadata: Option<Metadata>,
    amoris: Option<Result<PathBuf, Problem>>,
}

impl Run<'_> {
    fn failed_need(&self, step: &str) -> Option<String> {
        needs(step).iter().find_map(|n| {
            self.results
                .iter()
                .find(|r| r.name == *n && r.verdict == Verdict::Fail)
                .map(|_| format!("{n} failed"))
        })
    }

    fn target_dir(&self) -> PathBuf {
        if let Some(m) = &self.metadata
            && !m.target_directory.is_empty()
        {
            return PathBuf::from(&m.target_directory);
        }
        std::env::var_os("CARGO_TARGET_DIR")
            .map_or_else(|| self.env.root.join("target"), PathBuf::from)
    }

    /// The workspace members, from the deps step's metadata or, when it did not run, from
    /// `cargo metadata --no-deps`.
    fn members(&mut self) -> (BTreeSet<String>, Option<bool>) {
        if self.metadata.is_none() {
            let cmd = self
                .env
                .cargo(["metadata", "--format-version", "1", "--no-deps"]);
            let log = self.env.log_path("metadata");
            if let Ok(out) = crate::run::run_stdout(cmd, &log) {
                self.metadata = serde_json::from_str(&out.text).ok();
            }
        }
        let Some(m) = &self.metadata else {
            return (BTreeSet::new(), None);
        };
        let members: BTreeSet<String> = m.members().iter().map(|p| p.name.clone()).collect();
        let web = m
            .members()
            .into_iter()
            .find(|p| p.name == "amoris-web")
            .map(|p| p.features.contains_key("editor"));
        (members, web)
    }

    /// The `amoris` binary with its `check` subcommand, or `check.command_failed` saying why
    /// there is none (no binary, or one that gave no report).
    fn amoris(&mut self) -> Result<PathBuf, Problem> {
        if self.amoris.is_none() {
            let found = projects::amoris_binary(&self.target_dir())
                .and_then(|p| projects::probe(&p, &self.env.root).map(|()| p));
            self.amoris = Some(found);
        }
        self.amoris.clone().expect("probed")
    }

    fn step(&mut self, name: &str) -> StepResult {
        let (env, prep, opts) = (self.env, self.prep, self.opts);
        match name {
            "deps" => {
                let log = env.new_log("deps");
                let (mut step, meta) = deps::run(env, &prep.graph, &log);
                step.log = Some(env.rel(&log));
                if meta.is_some() {
                    self.metadata = meta;
                }
                step
            }
            "fmt" => cargo::fmt(env),
            "docs" => docs::run(env, &prep.listed),
            "clippy" => {
                let (members, _) = self.members();
                let web = prep.graph.web_crates(&members, None);
                let dir = self.target_dir().to_string_lossy().into_owned();
                cargo::clippy(env, &web, Some(&dir))
            }
            "build" => cargo::build(env),
            "gen" => generate::check(&env.root, &prep.listed),
            "test" => test::run(env, opts.quick),
            "gpus" => {
                // `test` runs the default adapter's `auto` configuration, unless it was left out
                // or the environment chose another device for it.
                let test_ran = self
                    .results
                    .iter()
                    .any(|r| r.name == "test" && r.verdict != Verdict::Skipped);
                let set: Vec<&str> = gpus::DEVICE_VARS
                    .into_iter()
                    .filter(|k| std::env::var_os(k).is_some())
                    .collect();
                gpus::run(
                    env,
                    crate::run::python(),
                    gpus::default_auto(test_ran, &set),
                )
            }
            "wasm" => {
                let (members, web) = self.members();
                cargo::wasm(env, &prep.graph.web_crates(&members, Some("game")), web)
            }
            "types" | "determinism" | "fork" | "replay" | "reload" => {
                let amoris = self.amoris();
                let mut step =
                    projects::run(env, &amoris, &prep.projects, name, opts.quick, opts.jobs);
                // The cross-build variant (checks.md 8.2), outside --quick.
                if name == "determinism"
                    && !opts.quick
                    && let Ok(release) = &amoris
                {
                    cross_build::run(env, release, &prep.projects, &self.target_dir(), &mut step);
                    step.summary
                        .push_str("; cross build against a debug amoris");
                }
                step
            }
            "contract" => StepResult::skipped(
                "contract",
                "the conformance checks and the benchmark self-test (checks.md 8.7) are not built yet",
            ),
            "web" => {
                let amoris = self.amoris();
                web::run(env, &self.target_dir(), &amoris, &prep.projects)
            }
            "perf" => match self.amoris() {
                Err(why) => StepResult::skipped(
                    "perf",
                    format!(
                        "amoris-check not built yet: {}; `amoris bench` and bench/budgets/ come with it (budgets.md 7, 8)",
                        why.message
                    ),
                ),
                Ok(_) => StepResult::skipped(
                    "perf",
                    "xtask does not run the budgets of bench/budgets/ yet (budgets.md 8)",
                ),
            },
            other => StepResult::skipped(other, "unknown step"),
        }
    }
}

/// Runs every step in order, printing progress on stderr.
pub fn run(env: &Env, opts: &CheckOptions, prep: &Prepared) -> Vec<StepResult> {
    let mut run = Run {
        env,
        opts,
        prep,
        results: Vec::new(),
        metadata: None,
        amoris: None,
    };
    for (i, name) in STEPS.iter().enumerate() {
        let result = if let Some(why) = opts.wants(name) {
            StepResult::skipped(name, why)
        } else if let Some(why) = run.failed_need(name) {
            StepResult::skipped(name, why)
        } else {
            eprintln!("[{}/{}] {name}", i + 1, STEPS.len());
            let start = Instant::now();
            let mut r = run.step(name);
            r.duration_ms = start.elapsed().as_millis() as u64;
            eprintln!(
                "       {} in {}",
                r.verdict.label(),
                crate::report::seconds(r.duration_ms)
            );
            r
        };
        run.results.push(result);
    }
    run.results
}
