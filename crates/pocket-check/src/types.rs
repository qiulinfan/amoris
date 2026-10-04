//! The `types` check (docs/spec/checks.md 6.4): the stateless-script lint, which compiling with the
//! lint on runs, each finding under its own code with its location. The TypeScript type check
//! (`tsc`, script-host.md 7.4) is not built in slice 1, which the step says as a warning.

use std::path::Path;

use crate::report::StepResult;

/// The lint and compile findings of the project's scripts.
#[cfg(feature = "native")]
pub fn check(dir: &Path, name: &str) -> StepResult {
    use pocket_contract::{Problem, detail};
    use serde_json::json;

    let mut step = StepResult::new("types");
    let modules = match pocket_runtime::lint(dir) {
        Ok(n) => n,
        Err(problems) => {
            for mut p in problems {
                p.detail.insert("project".into(), json!(name));
                step.error(p);
            }
            0
        }
    };
    step.warn(Problem::new(
        "check.not_implemented",
        "The TypeScript type check (tsc) is not built yet; only the lint ran.",
        detail([("what", json!("tsc"))]),
    ));
    step.measure("modules", modules as f64, "count");
    step.summary = format!("{modules} modules linted; tsc not built yet");
    step
}

/// Without the compiler there is nothing to lint.
#[cfg(not(feature = "native"))]
pub fn check(_dir: &Path, _name: &str) -> StepResult {
    StepResult::skipped("types", "this build has no TypeScript compiler")
}
