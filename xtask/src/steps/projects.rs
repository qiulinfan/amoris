//! The per-project checks (docs/spec/checks.md 6.4 and 8): `types`, `determinism`, `fork`,
//! `replay` and `reload`, each run through the built binary as
//! `pocket check <project> --json --only <check> [--seeds <first>]`, whose stdout is a `Report`
//! (checks.md 10.1) with one step named after the check. xtask merges the projects' rows into its
//! own step of that name and judges the negative controls (`[expect] fail`, checks.md 8.6).

use crate::config;
use crate::files;
use crate::report::{Problem, Report, StepResult, Verdict};
use crate::run::{self, Env};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

// The shape of `check.toml` (checks.md 8.1). xtask parses it only to refuse a malformed or
// misspelt file before any step runs (exit code 2); `pocket check` reads the values.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct CheckToml {
    pub run: Run,
    pub fork: Option<Fork>,
    pub reload: Option<Reload>,
    pub web: Option<Web>,
    pub expect: Option<Expect>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct Run {
    pub seeds: Vec<u64>,
    pub ticks: u64,
    pub inputs: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct Fork {
    pub at: Vec<u64>,
    pub ticks: u64,
    pub branch_inputs: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct Reload {
    pub at: Vec<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
pub struct Web {
    pub workload: bool,
}

/// A negative control: the code it must fail with, and whether its scripts compile with the lint
/// off (`lint = false`, so a defect the lint refuses reaches the check that is its backstop;
/// checks.md 14, Slice 1).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Expect {
    pub fail: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub lint: Option<bool>,
}

pub struct Project {
    pub dir: String,
    pub config: CheckToml,
}

impl Project {
    /// The step whose check a control's expected code belongs to, if the project is a control.
    pub fn control_step(&self) -> Option<&'static str> {
        self.config.expect.as_ref().and_then(|e| family(&e.fail))
    }
}

/// The step a problem code belongs to: `reload.diverged` to `reload`, the lint's and the type
/// check's codes to `types`, `web.*` to the web page.
pub fn family(code: &str) -> Option<&'static str> {
    match code.split('.').next()? {
        "determinism" => Some("determinism"),
        "fork" => Some("fork"),
        "replay" => Some("replay"),
        "reload" => Some("reload"),
        "types" | "lint" => Some("types"),
        "web" => Some("web"),
        _ => None,
    }
}

/// Every project with a `check.toml` under `samples/` or `tests/fixtures/`, parsed (checks.md 8.1).
pub fn discover(root: &Path, listed: &[String]) -> Result<Vec<Project>, Problem> {
    let mut projects = Vec::new();
    for path in listed {
        if !(files::glob("samples/*/check.toml", path)
            || files::glob("tests/fixtures/**/check.toml", path))
        {
            continue;
        }
        let config: CheckToml = config::load(root, path)?;
        if let Some(e) = &config.expect
            && family(&e.fail).is_none()
        {
            return Err(config::invalid(
                path,
                None,
                &format!("[expect] fail = \"{}\" names no check's code", e.fail),
            ));
        }
        if config.run.seeds.is_empty() {
            return Err(config::invalid(path, None, "[run] seeds is empty"));
        }
        let dir = path.strip_suffix("/check.toml").unwrap_or(path).to_string();
        projects.push(Project { dir, config });
    }
    Ok(projects)
}

/// The built `pocket` binary, or `check.command_failed` naming where the release build left none.
pub fn pocket_binary(target_dir: &Path) -> Result<PathBuf, Problem> {
    let release = target_dir.join("release");
    let p = release.join(format!("pocket{}", std::env::consts::EXE_SUFFIX));
    if p.is_file() {
        return Ok(p);
    }
    Err(Problem::new(
        "check.command_failed",
        format!("no pocket binary in {}", release.display()),
        json!({"command": format!("{} check --json", p.display()), "status": null, "tail": ""}),
    ))
}

/// The last `n` lines of a process's output, for a problem's `tail`.
fn tail(bytes: &[u8], n: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Whether the binary has `pocket check`: asked for a report with no project, it must answer
/// with one (a `check.usage` report), since both commands print the same shape (checks.md 2). When
/// it does not, `check.command_failed {command, status, tail}` with its exit status and the end of
/// its stderr.
pub fn probe(pocket: &Path, root: &Path) -> Result<(), Problem> {
    let mut cmd = Command::new(pocket);
    cmd.args(["check", "--json"])
        .current_dir(root)
        .stdin(std::process::Stdio::null());
    let line = run::describe(&cmd);
    let failed = |message: String, status: Option<i32>, tail: String| {
        Problem::new(
            "check.command_failed",
            message,
            json!({"command": line, "status": status, "tail": tail}),
        )
    };
    let out = cmd
        .output()
        .map_err(|e| failed(format!("{line} could not start: {e}"), None, String::new()))?;
    match serde_json::from_slice::<Report>(&out.stdout) {
        Ok(r) if r.format == 1 => Ok(()),
        _ => Err(failed(
            format!("{line} gave no report (exit {:?})", out.status.code()),
            out.status.code(),
            tail(&out.stderr, 20),
        )),
    }
}

/// The projects that take part in `check`: every project but the controls of other checks.
fn participants<'a>(projects: &'a [Project], check: &str) -> Vec<&'a Project> {
    projects
        .iter()
        .filter(|p| p.control_step().is_none_or(|s| s == check))
        .collect()
}

/// The step when `pocket check` cannot run. While no project takes part, `Skipped` (the slice 1
/// state, checks.md 14.8); once one does, the probe's failure fails the step, so a check or a
/// control that must never pass cannot vanish with exit 0.
pub fn unchecked(check: &str, mine: &[&Project], mut problem: Problem) -> StepResult {
    if mine.is_empty() {
        return StepResult::skipped(
            check,
            format!("pocket-check not built yet: {}", problem.message),
        );
    }
    let dirs: Vec<&str> = mine.iter().map(|p| p.dir.as_str()).collect();
    problem.message = format!(
        "{} projects take part in {check}, but `pocket check` cannot run: {}",
        dirs.len(),
        problem.message
    );
    problem.detail.insert("projects".into(), json!(dirs));
    let mut step = StepResult::new(check);
    step.error(problem);
    step.summary = format!("{} projects not checked", dirs.len());
    step
}

/// Merges one project's row for `check` into the step, judging a control by its expected code.
pub fn merge(step: &mut StepResult, project: &Project, check: &str, row: &StepResult) {
    let tag = |mut p: Problem| {
        p.detail
            .insert("project".into(), Value::String(project.dir.clone()));
        p
    };
    if let Some(expected) = project.config.expect.as_ref().map(|e| e.fail.as_str()) {
        let control = project
            .dir
            .rsplit('/')
            .next()
            .unwrap_or(&project.dir)
            .to_string();
        let got: Vec<&str> = row.errors.iter().map(|p| p.code.as_str()).collect();
        if got.contains(&expected) {
            step.measure(&format!("{}/control_caught", project.dir), 1.0, "count");
        } else if row.verdict == Verdict::Pass {
            step.error(tag(Problem::new(
                "check.control_passed",
                format!("the control {control} passed {check}; it must fail with {expected}"),
                json!({"control": control, "expected": expected}),
            )));
        } else {
            step.error(tag(Problem::new(
                "check.control_wrong_failure",
                format!("the control {control} failed {check} with {got:?}, not {expected}"),
                json!({"control": control, "expected": expected, "got": got}),
            )));
        }
        return;
    }
    for p in row.errors.iter().cloned() {
        match row.verdict {
            Verdict::Inconclusive => step.inconclusive(tag(p)),
            _ => step.error(tag(p)),
        }
    }
    step.more_errors += row.more_errors;
    // The least report (checks.md 14.7) may give a verdict without a problem: it still counts.
    if row.errors.is_empty() && matches!(row.verdict, Verdict::Fail | Verdict::Inconclusive) {
        let verdict = format!("{:?}", row.verdict);
        let p = tag(Problem::new(
            "check.command_failed",
            format!(
                "pocket check reported {check} as {verdict} for {} without a problem",
                project.dir
            ),
            json!({"verdict": verdict}),
        ));
        if row.verdict == Verdict::Fail {
            step.error(p);
        } else {
            step.inconclusive(p);
        }
    }
    for p in row.warnings.iter().cloned() {
        step.warn(tag(p));
    }
    for mut m in row.measurements.iter().cloned() {
        m.name = format!("{}/{}", project.dir, m.name);
        step.measurements.push(m);
    }
}

/// Runs one check over every project that takes part in it, `jobs` processes at a time; `pocket`
/// is the probed binary or why it cannot run.
pub fn run(
    env: &Env,
    pocket: &Result<PathBuf, Problem>,
    projects: &[Project],
    check: &str,
    quick: bool,
    jobs: usize,
) -> StepResult {
    let mine = participants(projects, check);
    let pocket = match pocket {
        Ok(p) => p,
        Err(p) => return unchecked(check, &mine, p.clone()),
    };
    if mine.is_empty() {
        return StepResult::skipped(
            check,
            format!("no project under samples/ or tests/fixtures/ has a check.toml for {check}"),
        );
    }
    let mut step = StepResult::new(check);
    let queue = Mutex::new(mine.iter().copied().enumerate().collect::<Vec<_>>());
    let results: Mutex<Vec<(usize, Result<StepResult, Problem>)>> = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| {
                loop {
                    let Some((i, project)) = queue.lock().expect("queue").pop() else {
                        break;
                    };
                    let r = run_one(env, pocket, project, check, quick);
                    results.lock().expect("results").push((i, r));
                }
            });
        }
    });
    let mut results = results.into_inner().expect("results");
    results.sort_by_key(|(i, _)| *i);
    for (i, r) in results {
        match r {
            Ok(row) => merge(&mut step, mine[i], check, &row),
            Err(p) => step.error(p),
        }
    }
    step.summary = format!(
        "{} projects{}",
        mine.len(),
        if quick { ", first seed only" } else { "" }
    );
    step
}

/// The row for `check` in what `pocket check` printed, or why there is none: no report, no step
/// of that name, or an exit status other than the one checks.md 11 gives the row's verdict (a
/// failing row that exits 0, a passing one that exits 1).
pub fn read_row(stdout: &str, status: Option<i32>, check: &str) -> Result<StepResult, String> {
    let report: Report = serde_json::from_str(stdout)
        .map_err(|e| format!("printed no report (exit {status:?}): {e}"))?;
    let row = report
        .steps
        .into_iter()
        .find(|s| s.name == check)
        .ok_or_else(|| format!("its report has no step '{check}' (exit {status:?})"))?;
    let expected = i32::from(crate::report::exit_code(row.verdict));
    if status != Some(expected) {
        return Err(format!(
            "exited {status:?}, but its report gives {check} the verdict {:?}, whose exit code is {expected}",
            row.verdict
        ));
    }
    Ok(row)
}

fn run_one(
    env: &Env,
    pocket: &Path,
    project: &Project,
    check: &str,
    quick: bool,
) -> Result<StepResult, Problem> {
    let mut cmd = Command::new(pocket);
    cmd.args(["check", &project.dir, "--json", "--only", check])
        .current_dir(&env.root);
    if quick {
        cmd.args(["--seeds", &project.config.run.seeds[0].to_string()]);
    }
    let log = env.new_log(&format!("{check}-{}", project.dir.replace('/', "-")));
    let line = run::describe(&cmd);
    let failed = |message: String, status: Option<i32>| {
        let logged = std::fs::read(&log).unwrap_or_default();
        Problem::new(
            "check.command_failed",
            message,
            json!({"command": line, "status": status, "tail": tail(&logged, 20), "project": project.dir, "log": env.rel(&log)}),
        )
    };
    let out = run::run_stdout(cmd, &log)
        .map_err(|e| failed(format!("{line} could not start: {e}"), None))?;
    read_row(&out.text, out.code, check).map_err(|why| failed(format!("{line} {why}"), out.code))
}

#[cfg(test)]
#[path = "projects_tests.rs"]
mod tests;
