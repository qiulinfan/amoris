//! The determinism check's cross-build variant (docs/spec/checks.md 8.2): the sailing workload's
//! hash chain from a debug build of `pocket` against the release build's, which catches
//! optimizer-dependent results and debug-only code paths. Full check only: `--quick` leaves it out,
//! since it needs a second build. xtask links no engine crate, so a divergence names its tick and
//! both hashes, and the commands that give each build's snapshot there for `diff` (checks.md 14).

use crate::report::{Problem, StepResult};
use crate::run::{self, Env};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::projects::Project;

/// The workload the variant runs (checks.md 8.2).
pub const PROJECT: &str = "samples/sailing";

fn failed(command: &str, status: Option<i32>, error: Value, tail: String, why: &str) -> Problem {
    Problem::new(
        "check.command_failed",
        format!("{command} {why}"),
        json!({"command": command, "status": status, "error": error, "tail": tail, "project": PROJECT}),
    )
}

/// One build's chain: `[tick, hash]` per tick, read strictly.
fn chain(env: &Env, pocket: &Path, seed: u64, log: &Path) -> Result<Vec<(u64, String)>, Problem> {
    let mut cmd = Command::new(pocket);
    cmd.args([
        "check",
        PROJECT,
        "--chain-only",
        "--seeds",
        &seed.to_string(),
        "--json",
    ])
    .current_dir(&env.root);
    let line = run::describe(&cmd);
    let out = run::run_stdout(cmd, log).map_err(|e| run::start_failed(&line, &e))?;
    let tail = || {
        let logged = std::fs::read(log).unwrap_or_default();
        let text = String::from_utf8_lossy(&logged);
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(20)..].join("\n")
    };
    let v: Value = serde_json::from_str(&out.text).map_err(|e| {
        failed(
            &line,
            out.code,
            Value::Null,
            tail(),
            &format!("printed no JSON: {e}"),
        )
    })?;
    if let Some(error) = v.get("error") {
        return Err(failed(&line, out.code, error.clone(), tail(), "failed"));
    }
    if !out.ok() {
        return Err(failed(
            &line,
            out.code,
            Value::Null,
            tail(),
            "exited with a failure",
        ));
    }
    let items = v["chain"]
        .as_array()
        .ok_or_else(|| failed(&line, out.code, Value::Null, tail(), "printed no chain"))?;
    items
        .iter()
        .map(|p| match (p[0].as_u64(), p[1].as_str()) {
            (Some(t), Some(h)) => Ok((t, h.to_owned())),
            _ => Err(failed(
                &line,
                out.code,
                Value::Null,
                tail(),
                &format!("printed {p}"),
            )),
        })
        .collect()
}

/// The first tick where the chains differ, with each side's hash (`None` past its end).
pub fn first_difference(
    release: &[(u64, String)],
    debug: &[(u64, String)],
) -> Option<(u64, Option<String>, Option<String>)> {
    let n = release.len().max(debug.len());
    (0..n).find_map(|i| {
        let (a, b) = (release.get(i), debug.get(i));
        if a == b {
            return None;
        }
        let tick = a.or(b).map_or(0, |(t, _)| *t);
        Some((tick, a.map(|(_, h)| h.clone()), b.map(|(_, h)| h.clone())))
    })
}

/// Builds the debug `pocket`, runs the sailing chain of the first seed with both builds, and adds
/// the comparison to `step`: `determinism.diverged {project, seed, variant: "cross_build", ...}`
/// or the ticks compared.
pub fn run(
    env: &Env,
    release: &Path,
    projects: &[Project],
    target_dir: &Path,
    step: &mut StepResult,
) {
    let Some(project) = projects.iter().find(|p| p.dir == PROJECT) else {
        step.error(failed(
            "cross build",
            None,
            Value::Null,
            String::new(),
            &format!("has no workload: {PROJECT} has no check.toml"),
        ));
        return;
    };
    let seed = project.config.run.seeds[0];
    let log = env.new_log("determinism-cross-build");
    eprintln!("  determinism: building the debug pocket for the cross-build variant");
    let build = env.cargo(["build", "-p", "pocket-app", "--bin", "pocket", "--locked"]);
    let line = run::describe(&build);
    let out = match run::run_merged(build, &log, &mut |_| {}) {
        Ok(o) => o,
        Err(e) => return step.error(run::start_failed(&line, &e)),
    };
    if !out.ok() {
        step.error(failed(&line, out.code, Value::Null, out.tail(20), "failed"));
        return;
    }
    let debug: PathBuf = target_dir
        .join("debug")
        .join(format!("pocket{}", std::env::consts::EXE_SUFFIX));
    eprintln!("  determinism: the sailing chain of seed {seed}, release and debug");
    let chains =
        chain(env, release, seed, &log).and_then(|r| Ok((r, chain(env, &debug, seed, &log)?)));
    let (r, d) = match chains {
        Ok(c) => c,
        Err(p) => return step.error(p),
    };
    match first_difference(&r, &d) {
        None => step.measure("cross_build_ticks_compared", r.len() as f64, "count"),
        Some((tick, release_hash, debug_hash)) => {
            let at = |pocket: &Path| {
                format!(
                    "{} check {PROJECT} --chain-only --seeds {seed} --snapshot-at {tick} --json",
                    pocket.display()
                )
            };
            step.error(Problem::new(
                "determinism.diverged",
                format!(
                    "{PROJECT} with seed {seed} diverged at tick {tick} between the release and the debug build (cross_build)"
                ),
                json!({
                    "project": PROJECT,
                    "seed": seed,
                    "variant": "cross_build",
                    "divergence": {"at": {"segment": 0, "tick": tick}, "kind": "Hash",
                                   "expected": release_hash, "actual": debug_hash},
                    "snapshots": [at(release), at(&debug)],
                }),
            ));
        }
    }
    if step.log.is_none() {
        step.log = Some(env.rel(&log));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(items: &[(u64, &str)]) -> Vec<(u64, String)> {
        items.iter().map(|(t, h)| (*t, (*h).to_owned())).collect()
    }

    #[test]
    fn chains_differ_at_their_first_unequal_tick() {
        let a = c(&[(0, "a"), (1, "b"), (2, "c")]);
        assert_eq!(first_difference(&a, &a), None);
        assert_eq!(
            first_difference(&a, &c(&[(0, "a"), (1, "x"), (2, "c")])),
            Some((1, Some("b".into()), Some("x".into())))
        );
        assert_eq!(
            first_difference(&a, &c(&[(0, "a"), (1, "b")])),
            Some((2, Some("c".into()), None))
        );
    }
}
