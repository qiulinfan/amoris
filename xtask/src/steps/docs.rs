//! `docs`: documentation is prose hard-wrapped at 100 display columns (AGENTS.md rule 7), checked
//! with `tools/wrap_docs.py --check` over the Markdown under `docs/`, except the recorded evidence
//! under `docs/evidence/` (checks-slice1.md 9). `shared/` is the contract's text, which changes
//! only through its own record (`gen.shared_modified`), so this step does not reformat it.

use crate::report::{Problem, StepResult};
use crate::run::{self, Env};
use serde_json::json;
use std::process::Command;

/// The paths `wrap_docs.py --check` lists after its "would change N of M" line.
pub fn parse(output: &str) -> Vec<String> {
    let mut lines = output
        .lines()
        .skip_while(|l| !l.starts_with("would change"));
    lines.next();
    lines
        .filter(|l| l.starts_with("  "))
        .map(|l| l.trim().replace('\\', "/"))
        .collect()
}

/// Whether the step reads a file: Markdown under `docs/`, but not `docs/evidence/`, whose
/// transcripts and logs are kept as they were recorded.
pub fn checked(path: &str) -> bool {
    path.starts_with("docs/") && !path.starts_with("docs/evidence/") && path.ends_with(".md")
}

pub fn run(env: &Env, listed: &[String]) -> StepResult {
    let mut step = StepResult::new("docs");
    let log = env.new_log("docs");
    step.log = Some(env.rel(&log));
    let docs: Vec<&String> = listed.iter().filter(|p| checked(p)).collect();
    let Some(python) = run::python() else {
        step.inconclusive(Problem::new(
            "check.tool_missing",
            "no Python 3 to run tools/wrap_docs.py (python or python3 on PATH, or POCKET_PYTHON)",
            json!({"tool": "python"}),
        ));
        step.summary = "python missing".into();
        return step;
    };
    let mut cmd = Command::new(python);
    cmd.arg("tools/wrap_docs.py")
        .arg("--check")
        .args(&docs)
        .current_dir(&env.root)
        .env("PYTHONUTF8", "1");
    let line = run::describe(&cmd);
    let out = match run::run_merged(cmd, &log, &mut |_| {}) {
        Ok(o) => o,
        Err(e) => {
            step.problem(run::start_failed(&line, &e));
            return step;
        }
    };
    if !out.ok() || !out.text.contains("would change") {
        step.error(Problem::new(
            "check.command_failed",
            format!("tools/wrap_docs.py --check failed (exit {:?})", out.code),
            json!({"command": "python tools/wrap_docs.py --check <docs>", "status": out.code, "tail": out.tail(20)}),
        ));
        return step;
    }
    let unwrapped = parse(&out.text);
    for path in &unwrapped {
        step.error(Problem::new(
            "docs.unwrapped",
            format!(
                "{path} has prose wider than 100 columns; run python tools/wrap_docs.py {path}"
            ),
            json!({"path": path}),
        ));
    }
    step.measure("docs.files", docs.len() as f64, "count");
    step.summary = format!(
        "{} Markdown files under docs/ (not docs/evidence/), {} need wrapping",
        docs.len(),
        unwrapped.len()
    );
    step
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listed_files_are_read() {
        let out = "would change 2 of 30\n  docs/a.md\n  shared/contract/b.md\n";
        assert_eq!(parse(out), ["docs/a.md", "shared/contract/b.md"]);
        assert!(parse("would change 0 of 30\n").is_empty());
    }

    #[test]
    fn evidence_and_shared_are_not_checked() {
        assert!(checked("docs/spec/checks.md"));
        assert!(!checked("shared/contract/mcp.md"));
        assert!(!checked(
            "docs/evidence/debug-eval/steer-sign/cli-1/transcript.md"
        ));
        assert!(!checked("docs/spec/checks.json"));
        assert!(!checked("README.md"));
    }
}
