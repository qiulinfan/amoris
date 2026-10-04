//! `docs`: documentation is prose wrapped at 100 columns (AGENTS.md rule 6), checked with
//! `tools/wrap_docs.py --check` over the Markdown under `docs/` and `shared/` (checks.md 14, Slice 1).

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

pub fn run(env: &Env, listed: &[String]) -> StepResult {
    let mut step = StepResult::new("docs");
    let log = env.new_log("docs");
    step.log = Some(env.rel(&log));
    let docs: Vec<&String> = listed
        .iter()
        .filter(|p| (p.starts_with("docs/") || p.starts_with("shared/")) && p.ends_with(".md"))
        .collect();
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
            format!("{path} is not wrapped at 100 columns; run python tools/wrap_docs.py {path}"),
            json!({"path": path}),
        ));
    }
    step.measure("docs.files", docs.len() as f64, "count");
    step.summary = format!(
        "{} Markdown files under docs/ and shared/, {} need wrapping",
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
}
