//! `web`: the web build checked in headless Chrome (docs/spec/checks.md 7). The module is built
//! once for wasm32 with the `web` profile, bound by `wasm-bindgen --target web` (and once more
//! through `wasm-opt -O3`), put beside the page of `web/` with the workloads' packages, inputs,
//! native hash chains and replays (`web_pack`), and the page is loaded with `?check` through
//! `tools/webcheck.py`: cross-origin isolated and not (checks.md 7.3), and the `-O3` variant once.
//!
//! The page reports through its title: `DONE <json>` or `FAIL <json>`, where the JSON may carry
//! `errors` (problems in the error protocol's shape, with the codes of checks.md 7.2), `warnings`,
//! `measurements` (checks.md 10.1) and `controls`, the outcome of each negative control among the
//! workloads (`{project, expected, got, caught, errors}`), which this step judges as the project
//! steps judge theirs (`projects::merge`: `check.control_passed`, `check.control_wrong_failure`);
//! any other JSON is reported as one `web.test_failed`.

use super::projects::{self, Project};
use super::web_pack;
use crate::report::{Measurement, Problem, StepResult};
use crate::run::{self, Env};
use crate::serve::Server;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The page: the shipped page and its worker (`index.html`, `pocket.js`, `game.js`, `form.js`)
/// and the check's driver (`check.js`), which the query `?check` turns on (checks.md 7.1;
/// checks-slice1.md 22).
pub const PAGE_DIR: &str = "web";

/// What one load of a page said.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    Done(Value),
    Fail(Value),
    Timeout,
}

/// Reads the last `DONE`/`FAIL` line of `tools/webcheck.py`'s output.
pub fn verdict(output: &str) -> Verdict {
    for line in output.lines().rev() {
        let parse = |s: &str| {
            serde_json::from_str(s.trim()).unwrap_or_else(|_| Value::String(s.trim().into()))
        };
        if let Some(rest) = line.strip_prefix("DONE ") {
            return Verdict::Done(parse(rest));
        }
        if let Some(rest) = line.strip_prefix("FAIL ") {
            return Verdict::Fail(parse(rest));
        }
    }
    Verdict::Timeout
}

/// Loads a page through `tools/webcheck.py` with this crate's server (`isolation` sets COOP and
/// COEP), returning the verdict and the output.
pub fn load(
    env: &Env,
    dir: &Path,
    page: &str,
    isolation: bool,
    timeout: u32,
    log: &Path,
) -> Result<(Verdict, String), Problem> {
    let python = run::python().ok_or_else(|| {
        Problem::new(
            "check.tool_missing",
            "no Python 3 to run tools/webcheck.py",
            json!({"tool": "python"}),
        )
    })?;
    let server = Server::start(dir, isolation).map_err(|e| {
        Problem::new(
            "check.command_failed",
            format!("cannot serve {}: {e}", dir.display()),
            json!({}),
        )
    })?;
    let mut cmd = Command::new(python);
    cmd.arg("tools/webcheck.py")
        .arg(server.url(page))
        .args(["--timeout", &timeout.to_string()])
        .current_dir(&env.root)
        .env("PYTHONUTF8", "1");
    let out = run::run_merged(cmd, log, &mut |l| eprintln!("    {l}")).map_err(|e| {
        Problem::new(
            "check.command_failed",
            format!("tools/webcheck.py could not start: {e}"),
            json!({}),
        )
    })?;
    drop(server);
    if out.text.contains("No module named 'websocket'") {
        return Err(Problem::new(
            "check.tool_missing",
            "tools/webcheck.py needs websocket-client (pip install websocket-client)",
            json!({"tool": "websocket-client"}),
        ));
    }
    if out.text.contains("no Chrome or Edge found") {
        return Err(Problem::new(
            "check.tool_missing",
            "no Chrome or Edge for tools/webcheck.py",
            json!({"tool": "chrome"}),
        ));
    }
    Ok((verdict(&out.text), out.text))
}

/// The problems a failed load gives.
pub fn failures(v: &Verdict, output: &str, isolation: bool) -> Vec<Problem> {
    let console: Vec<&str> = output
        .lines()
        .filter(|l| !l.starts_with("DONE ") && !l.starts_with("FAIL "))
        .collect();
    let console = console[console.len().saturating_sub(20)..].join("\n");
    let mode = if isolation {
        "isolated"
    } else {
        "not isolated"
    };
    match v {
        Verdict::Done(_) => Vec::new(),
        Verdict::Fail(json) => {
            let listed: Vec<Problem> = json
                .get("errors")
                .and_then(|e| serde_json::from_value::<Vec<Problem>>(e.clone()).ok())
                .unwrap_or_default();
            // A control the page did not catch fails the page; `judge_controls` reports it.
            let control_failed = json
                .get("controls")
                .and_then(Value::as_array)
                .is_some_and(|l| l.iter().any(|c| c["caught"] != true));
            if listed.is_empty() && !control_failed {
                vec![Problem::new(
                    "web.test_failed",
                    format!("the check page failed ({mode})"),
                    json!({"test": "page", "message": json, "console": console, "isolation": isolation}),
                )]
            } else {
                listed
                    .into_iter()
                    .map(|mut p| {
                        p.detail.insert("isolation".into(), Value::Bool(isolation));
                        p
                    })
                    .collect()
            }
        }
        Verdict::Timeout => vec![Problem::new(
            "web.test_failed",
            format!("the check page never set its title to DONE or FAIL ({mode})"),
            json!({"test": "page", "message": "timeout", "console": console, "isolation": isolation}),
        )],
    }
}

/// Judges the negative controls among the workloads from one load's `controls`
/// (`projects::merge`): a control the page caught passes; one it did not catch is
/// `check.control_passed`, one that failed with other codes `check.control_wrong_failure`, and one
/// the page did not report `web.test_failed {test: "controls"}`. The caught controls'
/// `<dir>/control_caught` measurements are kept when `measure`.
pub fn judge_controls(
    step: &mut StepResult,
    v: &Verdict,
    controls: &[&Project],
    isolation: bool,
    measure: bool,
) {
    let (Verdict::Done(json) | Verdict::Fail(json)) = v else {
        return; // a timeout is already the step's error (`failures`)
    };
    let listed = json.get("controls").and_then(Value::as_array);
    for project in controls {
        let mut row = StepResult::new("web");
        let entry = listed.and_then(|l| l.iter().find(|c| c["project"] == project.dir.as_str()));
        let Some(entry) = entry else {
            step.error(Problem::new(
                "web.test_failed",
                format!(
                    "the check page reported nothing for the control {}",
                    project.dir
                ),
                json!({"test": "controls", "project": project.dir, "isolation": isolation}),
            ));
            continue;
        };
        let errors: Vec<Problem> = entry
            .get("errors")
            .and_then(|e| serde_json::from_value(e.clone()).ok())
            .unwrap_or_default();
        for p in errors {
            row.error(p);
        }
        let mut judged = StepResult::new("web");
        projects::merge(&mut judged, project, "web", &row);
        for mut p in judged.errors {
            p.detail.insert("isolation".into(), Value::Bool(isolation));
            step.error(p);
        }
        if measure {
            step.measurements.extend(judged.measurements);
        }
    }
}

/// The loads of the check page: the plain module isolated and not (checks.md 7.3), then the
/// `-O3` variant once, isolated, whose measurements carry the suffix `.O3`. Timings come from the
/// isolated loads only: without isolation Chrome coarsens `performance.now()` to 0.1 ms.
fn loads(out: &Path, opt: Option<&Path>) -> Vec<(PathBuf, bool, &'static str)> {
    let mut v = vec![
        (out.to_path_buf(), true, ""),
        (out.to_path_buf(), false, ""),
    ];
    if let Some(o) = opt {
        v.push((o.to_path_buf(), true, ".O3"));
    }
    v
}

/// The bytes the shipped page fetches before its project: the page, its scripts, the glue and the
/// module (`web.size.page`, budgets.md 5.4; `check.js` is the check's, not the page's).
fn page_size(dir: &Path) -> u64 {
    [
        "index.html",
        "pocket.js",
        "game.js",
        "form.js",
        "pkg/pocket_web.js",
        "pkg/pocket_web_bg.wasm",
    ]
    .iter()
    .map(|f| std::fs::metadata(dir.join(f)).map_or(0, |m| m.len()))
    .sum()
}

pub fn run(
    env: &Env,
    target_dir: &Path,
    pocket: &Result<PathBuf, Problem>,
    projects: &[Project],
) -> StepResult {
    let page = env.root.join(PAGE_DIR);
    if !page.join("index.html").is_file() {
        return StepResult::skipped(
            "web",
            format!("pocket-web has no page yet ({PAGE_DIR}/index.html, checks.md 7)"),
        );
    }
    let workloads = web_pack::workloads(projects);
    if workloads.is_empty() {
        return StepResult::skipped(
            "web",
            "no project's check.toml has [web] workload = true (checks.md 7.1)",
        );
    }
    let mut step = StepResult::new("web");
    let log = env.new_log("web");
    step.log = Some(env.rel(&log));
    let pocket = match pocket {
        Ok(p) => p,
        Err(p) => {
            step.error(p.clone());
            return step;
        }
    };
    let (out, opt) = (env.out.join("web"), env.out.join("web-opt"));
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_dir_all(&opt);
    let Some(opt_built) = web_pack::module(env, &mut step, target_dir, &out, &opt, &log) else {
        return step;
    };
    let opt = opt_built.then_some(opt.as_path());
    for dir in std::iter::once(out.as_path()).chain(opt) {
        if let Err(e) = web_pack::copy_dir(&page, dir) {
            step.error(Problem::new(
                "check.command_failed",
                format!("cannot copy {PAGE_DIR}: {e}"),
                json!({}),
            ));
            return step;
        }
    }
    if !web_pack::workload_files(env, &mut step, pocket, &workloads, &out, &log) {
        return step;
    }
    if let Some(o) = opt {
        let copied = web_pack::copy_dir(&out.join("projects"), &o.join("projects"))
            .and_then(|()| std::fs::copy(out.join("expected.json"), o.join("expected.json")));
        if let Err(e) = copied {
            step.error(Problem::new(
                "check.command_failed",
                format!("cannot copy the workloads beside the -O3 page: {e}"),
                json!({}),
            ));
            return step;
        }
    }
    step.measure("web.size.page", page_size(&out) as f64 / 1024.0, "KiB");
    let controls: Vec<&Project> = projects
        .iter()
        .filter(|p| {
            workloads
                .iter()
                .any(|w| w.dir == p.dir && w.expect.is_some())
        })
        .collect();
    let mut passed = Vec::new();
    for (i, (dir, isolation, suffix)) in loads(&out, opt).into_iter().enumerate() {
        eprintln!(
            "  loading the check page ({}{})",
            if isolation {
                "isolated"
            } else {
                "not isolated"
            },
            if suffix.is_empty() {
                ""
            } else {
                ", wasm-opt -O3"
            }
        );
        match load(env, &dir, "index.html?check", isolation, 300, &log) {
            Ok((v, text)) => {
                if let Verdict::Done(json) | Verdict::Fail(json) = &v
                    && let Some(ms) = json
                        .get("measurements")
                        .and_then(|m| serde_json::from_value::<Vec<Measurement>>(m.clone()).ok())
                {
                    // The plain module's numbers from its first (isolated) load only.
                    if !suffix.is_empty() || isolation {
                        step.measurements.extend(ms.into_iter().map(|mut m| {
                            m.name.push_str(suffix);
                            m
                        }));
                    }
                }
                if suffix.is_empty() {
                    passed.push(matches!(v, Verdict::Done(_)));
                }
                let mut load_step = StepResult::new("web");
                for p in failures(&v, &text, isolation) {
                    load_step.error(p);
                }
                judge_controls(&mut load_step, &v, &controls, isolation, i == 0);
                step.measurements.extend(load_step.measurements);
                for mut p in load_step.errors {
                    if !suffix.is_empty() {
                        p.detail.insert("module".into(), json!("wasm-opt -O3"));
                    }
                    step.error(p);
                }
            }
            Err(p) => {
                step.inconclusive(p);
                return step;
            }
        }
    }
    if passed == [true, false] {
        step.error(Problem::new(
            "web.isolation_required",
            "the check page passes only cross-origin isolated; the Worker form must work on a shared link without COOP and COEP (charter 5.1)",
            json!({}),
        ));
    }
    let names: Vec<String> = workloads
        .iter()
        .map(|w| match &w.expect {
            Some(code) => format!("{} (control, must fail with {code})", w.dir),
            None => w.dir.clone(),
        })
        .collect();
    step.summary = format!(
        "{} against native pocket hashes in headless Chrome, isolated and not{}: {passed:?}",
        names.join(", "),
        if opt.is_some() {
            ", and with wasm-opt -O3"
        } else {
            ""
        }
    );
    step
}

/// `cargo xtask webcheck <page> --serve DIR [--isolation on|off] [--timeout S]`: one load of a page
/// through `tools/webcheck.py`, served by xtask, so a page can be tried without isolation too.
pub fn main(env: &Env, args: &[String]) -> Result<bool, Problem> {
    let usage = || {
        Problem::new(
            "check.usage",
            "cargo xtask webcheck <page> --serve DIR [--isolation on|off] [--timeout S]",
            json!({}),
        )
    };
    let (mut page, mut dir, mut isolation, mut timeout) = (None, None, true, 60u32);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--serve" => dir = it.next().cloned(),
            "--isolation" => isolation = it.next().map(String::as_str) != Some("off"),
            "--timeout" => timeout = it.next().and_then(|t| t.parse().ok()).ok_or_else(usage)?,
            s if !s.starts_with("--") && page.is_none() => page = Some(s.to_string()),
            _ => return Err(usage()),
        }
    }
    let (page, dir) = (page.ok_or_else(usage)?, dir.ok_or_else(usage)?);
    let log = env.new_log("webcheck");
    // The console went to stderr as it came; stdout carries the verdict line alone.
    let (v, text) = load(env, &PathBuf::from(dir), &page, isolation, timeout, &log)?;
    let last = text
        .lines()
        .rev()
        .find(|l| l.starts_with("DONE ") || l.starts_with("FAIL "));
    println!("{}", last.unwrap_or("TIMEOUT"));
    Ok(matches!(v, Verdict::Done(_)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdicts() {
        assert_eq!(
            verdict("[log] a\nDONE {\"ok\": 1}\n"),
            Verdict::Done(json!({"ok": 1}))
        );
        assert_eq!(
            verdict("FAIL {\"errors\": []}"),
            Verdict::Fail(json!({"errors": []}))
        );
        assert_eq!(
            verdict("TIMEOUT: the page did not set its title"),
            Verdict::Timeout
        );
    }

    #[test]
    fn page_errors_keep_their_codes() {
        let v = Verdict::Fail(
            json!({"errors": [{"code": "web.cross_target_diverged", "message": "m", "detail": {"tick": 7}}]}),
        );
        let ps = failures(&v, "[log] x\nFAIL {...}", false);
        assert_eq!(ps[0].code, "web.cross_target_diverged");
        assert_eq!(ps[0].detail["isolation"], json!(false));
        let ps = failures(&Verdict::Fail(json!("boom")), "[exception] e", true);
        assert_eq!(ps[0].code, "web.test_failed");
        assert_eq!(ps[0].detail["console"], json!("[exception] e"));
    }

    fn control(dir: &str) -> Project {
        let text = "[run]
seeds = [1]
ticks = 60
[web]
workload = true
                    [expect]
fail = \"web.cross_target_diverged\"
";
        Project {
            dir: dir.into(),
            config: crate::config::parse("check.toml", text).unwrap(),
        }
    }

    #[test]
    fn web_controls_are_workloads_and_other_controls_are_not() {
        let mut reload = control("tests/fixtures/controls/reload-one");
        reload.config.expect.as_mut().unwrap().fail = "reload.diverged".into();
        let list = [
            control("tests/fixtures/controls/physics-platform-math"),
            reload,
        ];
        let w = web_pack::workloads(&list);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].expect.as_deref(), Some("web.cross_target_diverged"));
    }

    #[test]
    fn web_controls_are_judged_by_their_code() {
        let p = control("tests/fixtures/controls/physics-platform-math");
        let page = |entry: Value| Verdict::Fail(json!({"errors": [], "controls": [entry]}));
        let entry = |codes: &[&str]| {
            let errors: Vec<Value> = codes
                .iter()
                .map(|c| json!({"code": c, "message": "m", "detail": {}}))
                .collect();
            json!({"project": p.dir, "expected": "web.cross_target_diverged",
                   "got": codes, "caught": codes.contains(&"web.cross_target_diverged"),
                   "errors": errors})
        };
        let judge = |v: &Verdict| {
            let mut step = StepResult::new("web");
            for e in failures(v, "", true) {
                step.error(e);
            }
            judge_controls(&mut step, v, &[&p], true, true);
            step
        };
        // Caught: the page passes and the step records the catch.
        let caught = Verdict::Done(json!({"errors": [],
            "controls": [entry(&["web.cross_target_diverged"])]}));
        let step = judge(&caught);
        assert_eq!(
            step.verdict,
            crate::report::Verdict::Pass,
            "{:?}",
            step.errors
        );
        assert_eq!(
            step.measurements[0].name,
            "tests/fixtures/controls/physics-platform-math/control_caught"
        );
        // Not caught: the hashes agreed.
        let step = judge(&page(entry(&[])));
        let codes: Vec<&str> = step.errors.iter().map(|e| e.code.as_str()).collect();
        assert_eq!(codes, ["check.control_passed"]);
        assert_eq!(
            step.errors[0].detail["control"],
            json!("physics-platform-math")
        );
        assert_eq!(step.errors[0].detail["isolation"], json!(true));
        // Failed otherwise.
        let step = judge(&page(entry(&["web.test_failed"])));
        assert_eq!(step.errors[0].code, "check.control_wrong_failure");
        assert_eq!(step.errors[0].detail["got"], json!(["web.test_failed"]));
        // Not reported at all.
        let step = judge(&Verdict::Done(json!({"errors": [], "controls": []})));
        assert_eq!(step.errors[0].code, "web.test_failed");
        assert_eq!(step.errors[0].detail["test"], json!("controls"));
    }
}
