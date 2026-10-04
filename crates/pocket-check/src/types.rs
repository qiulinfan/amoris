//! The `types` check (docs/spec/checks.md 6.4): the stateless-script lint, which compiling with the
//! lint on runs, each finding under its own code with its location; then, when the caller hands a
//! type checker over ([`crate::TypeCheck`], `pocket-app`'s `tsc`), the project's declarations
//! (`scripts.types`, script-host.md 7.4) and TypeScript's own check over them, each diagnostic a
//! `types.error {project, path, line, column, code, message}`.

use std::path::Path;

use crate::TypeCheck;
use crate::report::StepResult;

/// The lint and compile findings of the project's scripts, then `tsc`'s.
#[cfg(feature = "native")]
pub fn check(dir: &Path, name: &str, typecheck: Option<&TypeCheck>) -> StepResult {
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
            step.summary = "the scripts do not compile; tsc not run".into();
            return step;
        }
    };
    step.measure("modules", modules as f64, "count");
    let Some(typecheck) = typecheck else {
        step.warn(Problem::new(
            "check.not_implemented",
            "This build runs no TypeScript type check (tsc); only the lint ran.",
            detail([("what", json!("tsc"))]),
        ));
        step.summary = format!("{modules} modules linted; no tsc in this build");
        return step;
    };
    if let Err(mut p) = declarations(dir) {
        p.detail.insert("project".into(), json!(name));
        step.error(p);
        step.summary = format!("{modules} modules linted; their declarations failed");
        return step;
    }
    let r = typecheck(dir);
    if let Some(ms) = r["tsc_ms"].as_f64() {
        step.measure("tsc", ms, "ms");
    }
    let diagnostics = r["diagnostics"].as_array().cloned().unwrap_or_default();
    match r["typecheck"].as_str().unwrap_or("") {
        "ok" | "failed" => {
            for d in &diagnostics {
                let message = d["message"].as_str().unwrap_or("");
                step.error(Problem::new(
                    "types.error",
                    fit(message),
                    detail([
                        ("project", json!(name)),
                        ("path", d["file"].clone()),
                        ("line", d["line"].clone()),
                        ("column", d["column"].clone()),
                        ("code", d["code"].clone()),
                        ("tsc_message", json!(message)),
                    ]),
                ));
            }
            if r["typecheck"] == "failed" && diagnostics.is_empty() {
                step.error(Problem::new(
                    "types.error",
                    "tsc failed without a diagnostic.",
                    detail([("project", json!(name))]),
                ));
            }
            step.summary = format!(
                "{modules} modules linted; tsc: {} errors",
                diagnostics.len()
            );
        }
        "timeout" => {
            step.error(Problem::new(
                "types.timeout",
                "tsc did not finish in its time limit.",
                detail([("project", json!(name))]),
            ));
            step.summary = format!("{modules} modules linted; tsc timed out");
        }
        _ => {
            let reason = r["reason"].as_str().unwrap_or("tsc not found").to_owned();
            step.warn(Problem::new(
                "check.tool_missing",
                format!("The type check did not run: {reason}."),
                detail([("tool", json!("tsc")), ("reason", json!(reason))]),
            ));
            step.summary = format!("{modules} modules linted; tsc not found");
        }
    }
    step
}

/// `tsc`'s message within a problem's message limit, its last `Did you mean ...?` kept: a plain cut
/// would drop the suggestion after a long list of the names a type allows. The whole message is
/// the problem's `tsc_message`.
#[cfg(feature = "native")]
fn fit(message: &str) -> String {
    use pocket_contract::render::MAX_MESSAGE_BYTES;
    let Some(at) = message
        .rfind("Did you mean")
        .filter(|_| message.len() > MAX_MESSAGE_BYTES)
    else {
        return message.to_owned();
    };
    let tail = message[at..].lines().next().unwrap_or("");
    let mut cut = MAX_MESSAGE_BYTES.saturating_sub(tail.len() + 5).min(at);
    while !message.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{} ... {tail}", message[..cut].trim_end())
}

/// Writes the project's declarations (`.pocket/types/`, and a `tsconfig.json` when it has none),
/// as `scripts.types` does on a running host: the scripts compiled (the lint passed) and loaded
/// into a game with no entities.
#[cfg(feature = "native")]
fn declarations(dir: &Path) -> Result<serde_json::Value, pocket_contract::Problem> {
    use std::sync::Arc;

    let setup = Arc::new(pocket_runtime::Project::load(dir)?.setup(false)?);
    let set = setup.scripts.clone();
    let game = pocket_runtime::GameBuilder::new(setup)
        .project(dir)
        .build_empty(&set)?;
    game.script_types(&pocket_runtime::types::ScriptsTypesParams::default())
}

/// Without the compiler there is nothing to lint.
#[cfg(not(feature = "native"))]
pub fn check(_dir: &Path, _name: &str, _typecheck: Option<&TypeCheck>) -> StepResult {
    StepResult::skipped("types", "this build has no TypeScript compiler")
}
