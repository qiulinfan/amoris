//! The steps that run cargo over the workspace: `fmt`, `clippy` (natively, for the web target and
//! over the lint fixture, `fixture.rs`), `build` and `wasm` (docs/spec/checks.md 5.3, 6.1, 6.3).

use super::deps::WEB_TARGET;
use super::diag::{self, Diag};
use crate::report::{Problem, StepResult};
use crate::run::{self, Env, Outcome};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) fn go(step: &mut StepResult, cmd: Command, log: &Path) -> Option<Outcome> {
    let line = run::describe(&cmd);
    match run::run_merged(cmd, log, &mut |_| {}) {
        Ok(out) => Some(out),
        Err(e) => {
            step.problem(run::start_failed(&line, &e));
            None
        }
    }
}

/// A command that failed without a finding the step could parse (a stale `Cargo.lock` under
/// `--locked`, a build script that failed): never shown as a pass or as an empty list.
pub(super) fn command_failed(step: &mut StepResult, line: &str, out: &Outcome) {
    step.error(Problem::new(
        "check.command_failed",
        format!(
            "{line} failed (exit {:?}) without a diagnostic the step reads",
            out.code
        ),
        json!({"command": line, "status": out.code, "tail": diag::plain_tail(&out.text, 20)}),
    ));
}

pub fn fmt(env: &Env) -> StepResult {
    let mut step = StepResult::new("fmt");
    let log = env.new_log("fmt");
    step.log = Some(env.rel(&log));
    let cmd = env.cargo(["fmt", "--all", "--check", "--", "-l"]);
    let line = run::describe(&cmd);
    let Some(out) = go(&mut step, cmd, &log) else {
        return step;
    };
    let root = std::fs::canonicalize(&env.root).unwrap_or_else(|_| env.root.clone());
    let files: Vec<String> = out
        .text
        .lines()
        .filter(|l| l.ends_with(".rs"))
        .map(|l| {
            let p = PathBuf::from(l.trim());
            let rel = p
                .strip_prefix(&root)
                .or_else(|_| p.strip_prefix(&env.root))
                .unwrap_or(&p);
            rel.to_string_lossy().replace('\\', "/")
        })
        .collect();
    if !files.is_empty() {
        step.error(Problem::new(
            "fmt.unformatted",
            format!(
                "{} files are not formatted; run `cargo fmt -p <crate>` on your own crates",
                files.len()
            ),
            json!({"files": files}),
        ));
    } else if !out.ok() {
        command_failed(&mut step, &line, &out);
    }
    step.summary = if step.errors.is_empty() {
        "formatted".into()
    } else {
        format!("{} unformatted files", files.len())
    };
    step
}

fn clippy_problems(step: &mut StepResult, target: &str, diags: &[Diag]) {
    for d in diags {
        step.error(Problem::new(
            "clippy.warning",
            format!("{}: {}", d.at(), d.message),
            json!({"path": d.path, "line": d.line, "lint": d.lint, "message": d.message, "crate": d.package, "target": target}),
        ));
    }
}

pub fn clippy(env: &Env, web_crates: &[String], target_dir: Option<&str>) -> StepResult {
    let mut step = StepResult::new("clippy");
    let log = env.new_log("clippy");
    step.log = Some(env.rel(&log));
    let mut counts = Vec::new();
    let host = env.cargo([
        "clippy",
        "--workspace",
        "--all-targets",
        "--release",
        "--locked",
        "--keep-going",
        "--message-format=json",
        "--",
        "-D",
        "warnings",
    ]);
    let mut runs = vec![(env.host.clone(), host)];
    if !web_crates.is_empty() {
        let mut args: Vec<String> = [
            "clippy",
            "--target",
            WEB_TARGET,
            "--no-default-features",
            "--profile",
            "web",
            "--locked",
            "--keep-going",
            "--message-format=json",
        ]
        .map(String::from)
        .into();
        for c in web_crates {
            args.extend(["-p".into(), c.clone()]);
        }
        args.extend(["--".into(), "-D".into(), "warnings".into()]);
        runs.push((WEB_TARGET.to_string(), env.cargo(args)));
    }
    for (target, cmd) in runs {
        let line = run::describe(&cmd);
        eprintln!("  clippy for {target}");
        let Some(out) = go(&mut step, cmd, &log) else {
            continue;
        };
        let diags = diag::all(&out.text);
        counts.push(format!("{} on {target}", diags.len()));
        clippy_problems(&mut step, &target, &diags);
        if !out.ok() && diags.is_empty() {
            command_failed(&mut step, &line, &out);
        }
    }
    let fixture = super::fixture::lint(env, &mut step, &log, target_dir);
    step.summary = format!(
        "diagnostics: {}; lint fixture: {fixture}",
        counts.join(", ")
    );
    step
}

fn grouped_errors(diags: &[Diag]) -> BTreeMap<String, Vec<String>> {
    let mut by_crate: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for d in diags.iter().filter(|d| d.level == "error") {
        by_crate
            .entry(d.package.clone())
            .or_default()
            .push(format!("{}: {}", d.at(), d.message));
    }
    by_crate
}

pub fn build(env: &Env) -> StepResult {
    let mut step = StepResult::new("build");
    let log = env.new_log("build");
    step.log = Some(env.rel(&log));
    let cmd = env.cargo([
        "build",
        "--workspace",
        "--release",
        "--locked",
        "--keep-going",
        "--message-format=json",
    ]);
    let line = run::describe(&cmd);
    let Some(out) = go(&mut step, cmd, &log) else {
        return step;
    };
    let by_crate = grouped_errors(&diag::all(&out.text));
    for (krate, errors) in &by_crate {
        step.error(Problem::new(
            "build.failed",
            format!(
                "{krate} does not build: {} errors, the first: {}",
                errors.len(),
                errors[0]
            ),
            json!({"crate": krate, "errors": errors.iter().take(10).collect::<Vec<_>>()}),
        ));
    }
    if !out.ok() && by_crate.is_empty() {
        command_failed(&mut step, &line, &out);
    }
    step.summary = if out.ok() {
        "native release build".into()
    } else {
        format!("{} crates failed", by_crate.len())
    };
    step
}

/// `cargo check` for the web target of every game crate marked `web`, then of `pocket-web` with
/// the shipped feature set and with `editor` when it has that feature (checks.md 6.3).
pub fn wasm(env: &Env, game_crates: &[String], web_has_editor: Option<bool>) -> StepResult {
    let mut step = StepResult::new("wasm");
    let log = env.new_log("wasm");
    step.log = Some(env.rel(&log));
    if env.clang_for_web().is_none() {
        step.inconclusive(Problem::new(
            "check.tool_missing",
            format!(
                "no clang in {} for the web target's C code; set POCKET_LLVM",
                env.llvm.display()
            ),
            json!({"tool": "clang"}),
        ));
        step.summary = "clang missing".into();
        return step;
    }
    let base = [
        "check",
        "--target",
        WEB_TARGET,
        "--profile",
        "web",
        "--locked",
        "--keep-going",
        "--no-default-features",
        "--message-format=json",
    ];
    let mut runs: Vec<(String, Vec<String>)> = Vec::new();
    if !game_crates.is_empty() {
        let mut args: Vec<String> = base.map(String::from).into();
        for c in game_crates {
            args.extend(["-p".into(), c.clone()]);
        }
        runs.push(("game crates".into(), args));
    }
    if let Some(editor) = web_has_editor {
        let mut args: Vec<String> = base.map(String::from).into();
        args.extend(["-p".into(), "pocket-web".into()]);
        runs.push(("pocket-web".into(), args.clone()));
        if editor {
            args.extend(["--features".into(), "editor".into()]);
            runs.push(("pocket-web with editor".into(), args));
        }
    }
    let mut checked = Vec::new();
    for (what, args) in runs {
        eprintln!("  wasm check of {what}");
        let cmd = env.cargo(args);
        let line = run::describe(&cmd);
        let Some(out) = go(&mut step, cmd, &log) else {
            continue;
        };
        let by_crate = grouped_errors(&diag::all(&out.text));
        for (krate, errors) in &by_crate {
            step.error(Problem::new(
                "wasm.check_failed",
                format!(
                    "{krate} does not build for {WEB_TARGET} ({what}): {}",
                    errors[0]
                ),
                json!({"crate": krate, "errors": errors.iter().take(10).collect::<Vec<_>>()}),
            ));
        }
        if !out.ok() && by_crate.is_empty() {
            command_failed(&mut step, &line, &out);
        }
        checked.push(what);
    }
    step.summary = format!("checked for {WEB_TARGET}: {}", checked.join(", "));
    step
}
