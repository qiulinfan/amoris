//! `gpus`: the renderer's GPU tests on every GPU adapter and native backend of this machine (charter
//! 4.6, the owner's decision of 2026-10-16; docs/spec/checks.md 4, docs/spec/checks-slice1.md 24,
//! docs/bench/gpu-matrix.md 11). `tools/gpu_matrix.py --configs machine --workspace` lists the
//! adapters, runs pocket-render's test binaries, built as `test` built them, once per configuration
//! (each adapter on each native backend, `auto` on every adapter but the default one, whose run is
//! `test`'s, and WebGPU's baseline device on the default adapter) and writes `out/check/gpus.json`,
//! which this step reads: a test that failed, hung or crashed, a test binary that exited with an
//! error or listed no tests, and a configuration that found no GPU are errors, and every test that
//! skipped work for want of a capability is a warning, since a skip is never a pass. When `test`
//! did not run that default configuration in this check, the step runs it too (`default-auto`).
//! With no Python, or no adapter but software ones, the step is skipped and says why.

use super::cargo::command_failed;
use crate::report::{MAX_PROBLEMS, Problem, StepResult, Verdict};
use crate::run::{self, Env, Outcome};
use serde_json::{Value, json};
use std::process::Command;

/// `tools/gpu_matrix.py`'s exit status when it found no adapter to run on.
const NOTHING_TO_RUN: i32 = 3;

/// The variables that choose the device a test opens. `test` inherits them; the tool sets its own.
pub const DEVICE_VARS: [&str; 3] = ["POCKET_ADAPTER", "POCKET_BACKEND", "POCKET_GPU_MINIMAL"];

/// Why the step must run the default adapter's `auto` configuration itself, the one `test` runs:
/// `test` did not run in this check (`--only gpus`, `--skip test`), or `set` (the [`DEVICE_VARS`]
/// in the environment) made it open another device. None when `test` ran it.
pub fn default_auto(test_ran: bool, set: &[&str]) -> Option<String> {
    if !test_ran {
        Some("the test step did not run in this check".into())
    } else if !set.is_empty() {
        Some(format!(
            "{} set, so the test step ran another device",
            set.join(" and ")
        ))
    } else {
        None
    }
}

/// `python` is [`run::python`]'s answer; `default_auto` is [`default_auto`]'s.
pub fn run(env: &Env, python: Option<String>, default_auto: Option<String>) -> StepResult {
    let Some(python) = python else {
        return StepResult::skipped(
            "gpus",
            "no Python 3 to run tools/gpu_matrix.py (POCKET_PYTHON, python or python3)",
        );
    };
    let mut step = StepResult::new("gpus");
    let log = env.new_log("gpus");
    step.log = Some(env.rel(&log));
    let report_path = env.out.join("gpus.json");
    let _ = std::fs::remove_file(&report_path);
    let mut cmd = Command::new(python);
    cmd.arg("tools/gpu_matrix.py")
        .args(["--configs", "machine", "--workspace", "--json"])
        .arg(&report_path)
        .arg("--out")
        .arg(env.out.join("gpus"))
        .current_dir(&env.root)
        .env("PYTHONUTF8", "1");
    if let Some(why) = &default_auto {
        eprintln!("    the default adapter's auto configuration too: {why}");
        cmd.arg("--default-auto");
    }
    // The C compilers of every other cargo command here: with the same environment the tool's
    // `cargo test --workspace --no-run` finds `test`'s binaries up to date.
    for (k, v) in env.c_env() {
        cmd.env(k, v);
    }
    let line = run::describe(&cmd);
    let mut progress = |l: &str| {
        if l.starts_with("configurations:") || l.contains(": exit ") || l.starts_with("error:") {
            eprintln!("    {l}");
        }
    };
    let out = match run::run_merged(cmd, &log, &mut progress) {
        Ok(o) => o,
        Err(e) => {
            step.problem(run::start_failed(&line, &e));
            return step;
        }
    };
    let report: Option<Value> = std::fs::read_to_string(&report_path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());
    let paths = Paths {
        logs: env.rel(&env.out.join("gpus")),
        report: env.rel(&report_path),
    };
    let mut step = judge(step, &line, &out, report.as_ref(), &paths);
    if let Some(why) = default_auto
        && step.verdict != Verdict::Skipped
    {
        step.summary
            .push_str(&format!("; default-auto because {why}"));
    }
    step
}

/// Where the tool wrote its per-binary logs and its report, relative to the root.
pub struct Paths {
    pub logs: String,
    pub report: String,
}

/// The step's verdict from what the tool gave: its exit status, which is authoritative, and its
/// report (None when it wrote none). Exit 3 (no adapter but software ones) skips the step; no
/// report is `check.command_failed`, and so is a failing exit the report does not explain.
fn judge(
    mut step: StepResult,
    line: &str,
    out: &Outcome,
    report: Option<&Value>,
    paths: &Paths,
) -> StepResult {
    if out.code == Some(NOTHING_TO_RUN) {
        let mut skipped = StepResult::skipped("gpus", nothing_to_run(report));
        skipped.log = step.log;
        return skipped;
    }
    let Some(report) = report else {
        command_failed(&mut step, line, out);
        return step;
    };
    read(report, &mut step, paths);
    if !out.ok() && step.errors.is_empty() {
        command_failed(&mut step, line, out);
    }
    step
}

/// Why nothing ran: the adapters the tool found and left out.
fn nothing_to_run(report: Option<&Value>) -> String {
    let left = report
        .map(|r| strings(&r["machine"]["left_out"]))
        .unwrap_or_default();
    if left.is_empty() {
        "no GPU adapter on this machine".into()
    } else {
        format!(
            "no GPU adapter on this machine; left out: {}",
            left.join("; ")
        )
    }
}

fn strings(v: &Value) -> Vec<&str> {
    v.as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// Reads the tool's report (its `configs`, `built` and `problems`) into the step: an error per
/// failed, hung or crashed test, per test binary that exited with an error none of its tests
/// explains or that listed no tests (every binary in `built` runs in every configuration, since
/// the step passes neither `--bins` nor `--filter`), and per configuration that found no GPU; a
/// warning per skip, the last one counting those past the report's limit; the counts and a summary
/// naming every configuration.
pub fn read(report: &Value, step: &mut StepResult, paths: &Paths) {
    let empty = serde_json::Map::new();
    let configs = report["configs"].as_object().unwrap_or(&empty);
    // The tool's order (adapter by adapter) where it gave one; JSON objects come back sorted.
    let mut names = strings(&report["machine"]["configs"]);
    names.retain(|n| configs.contains_key(*n));
    for n in configs.keys() {
        if !names.contains(&n.as_str()) {
            names.push(n);
        }
    }
    let built: Vec<&str> = report["built"]
        .as_object()
        .map(|o| o.keys().map(String::as_str).collect())
        .unwrap_or_default();
    let (mut passed, mut skipped, mut failed, mut bad_binaries) = (0u64, 0u64, 0u64, 0u64);
    let mut skips = Vec::new();
    let mut skips_per_config = Vec::new();
    for &name in &names {
        let c = &configs[name];
        let adapter = c["adapter"].as_str().unwrap_or("not reported");
        let tests = c["tests"].as_object().unwrap_or(&empty);
        let mut config_skips = 0;
        for (test, cell) in tests {
            let status = cell["status"].as_str().unwrap_or("");
            let notes = strings(&cell["notes"]);
            match status {
                "pass" => passed += 1,
                "skip" => {
                    skipped += 1;
                    config_skips += 1;
                    skips.push(Problem::new(
                        "gpus.skipped",
                        format!("{name} {test}: {}", notes.join("; ")),
                        json!({"config": name, "adapter": adapter, "test": test, "notes": notes}),
                    ));
                }
                "FAIL" | "hang" | "crash" => {
                    failed += 1;
                    let binary = test.split(' ').next().unwrap_or("");
                    let message = cell["message"].as_str().unwrap_or("");
                    step.error(Problem::new(
                        "gpus.test_failed",
                        format!("{name} {test}: {status}{}", tail(message)),
                        json!({"config": name, "adapter": adapter, "test": test,
                               "status": status, "message": message,
                               "log": format!("{}/{name}/{binary}.log", paths.logs)}),
                    ));
                }
                _ => {}
            }
        }
        // A binary's exit status counts as `cargo test`'s does in `test`: a crash after all its
        // tests reported ok, or a listing that timed out, fails the step too.
        let ran = c["binaries"].as_object().unwrap_or(&empty);
        for (binary, info) in ran {
            if info["exit"].as_i64() == Some(0) {
                continue;
            }
            let explained = tests.iter().any(|(t, cell)| {
                t.split(' ').next() == Some(binary.as_str())
                    && matches!(cell["status"].as_str(), Some("FAIL" | "hang" | "crash"))
            });
            if explained {
                continue;
            }
            let exit = info["exit"]
                .as_str()
                .map_or_else(|| info["exit"].to_string(), str::to_string);
            let what = if exit == "timeout" && info["tests"].as_u64() == Some(0) {
                "listing its tests timed out".to_string()
            } else {
                format!("exit {exit} although none of its tests failed")
            };
            bad_binaries += 1;
            step.error(Problem::new(
                "gpus.binary_failed",
                format!("{name} {binary}: {what}"),
                json!({"config": name, "adapter": adapter, "binary": binary, "exit": exit,
                       "log": format!("{}/{name}/{binary}.log", paths.logs)}),
            ));
        }
        for binary in built.iter().filter(|b| !ran.contains_key(**b)) {
            bad_binaries += 1;
            step.error(Problem::new(
                "gpus.binary_failed",
                format!("{name} {binary}: listed no tests"),
                json!({"config": name, "adapter": adapter, "binary": binary}),
            ));
        }
        if config_skips > 0 {
            skips_per_config.push(format!("{name} {config_skips}"));
        }
    }
    // Every skip while they fit; past the report's limit the last warning counts the rest.
    let room = MAX_PROBLEMS.saturating_sub(step.warnings.len());
    let shown = if skips.len() > room {
        room.saturating_sub(1)
    } else {
        skips.len()
    };
    let more = skips.len() - shown;
    for p in skips.into_iter().take(shown) {
        step.warn(p);
    }
    if more > 0 {
        step.warn(Problem::new(
            "gpus.more_skipped",
            format!(
                "{more} more skipped tests: {} lists every one",
                paths.report
            ),
            json!({"count": more, "report": paths.report}),
        ));
    }
    for f in report["problems"].as_array().into_iter().flatten() {
        let name = f["config"].as_str().unwrap_or("");
        let tests = strings(&f["tests"]);
        let message = match f["kind"].as_str() {
            Some("no_adapter") => format!("{name}: no adapter reported"),
            Some("no_gpu") => format!("{name}: {} tests found no GPU", tests.len()),
            _ => continue, // `failed`: one error per test above
        };
        step.error(Problem::new(
            "gpus.no_gpu",
            message,
            json!({"config": name, "env": configs.get(name).map(|c| c["env"].clone()),
                   "tests": tests}),
        ));
    }
    step.measure("gpus.configs", configs.len() as f64, "count");
    step.measure("gpus.passed", passed as f64, "count");
    step.measure("gpus.skipped", skipped as f64, "count");
    step.measure("gpus.failed", failed as f64, "count");
    step.summary = format!(
        "{}: {passed} passed, {skipped} skipped{}, {failed} failed{}",
        names.join(", "),
        if skips_per_config.is_empty() {
            String::new()
        } else {
            format!(" ({})", skips_per_config.join(", "))
        },
        if bad_binaries > 0 {
            format!(", {bad_binaries} test binaries failed")
        } else {
            String::new()
        },
    );
}

fn tail(message: &str) -> String {
    if message.is_empty() {
        String::new()
    } else {
        format!(": {message}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn cell(status: &str, notes: &[&str], message: &str) -> Value {
        json!({"status": status, "notes": notes, "message": message})
    }

    fn paths() -> Paths {
        Paths {
            logs: "out/check/gpus".into(),
            report: "out/check/gpus.json".into(),
        }
    }

    fn outcome(code: Option<i32>) -> Outcome {
        Outcome {
            code,
            text: "error: something the step does not read\n".into(),
        }
    }

    #[test]
    fn skips_warn_and_failures_fail() {
        let report = json!({
            "configs": {
                "5060-dx12": {"adapter": "Direct3D 12 on NVIDIA", "env": {}, "tests": {
                    "lib a": cell("pass", &[], ""),
                    "taa b": cell("FAIL", &[], "off by 28"),
                }},
                "5060-dx12-minimal": {"adapter": "Direct3D 12 on NVIDIA", "env": {}, "tests": {
                    "lib a": cell("skip", &["no GPU timestamps: skipped"], ""),
                    "taa b": cell("hang", &[], "no result after a minute or more"),
                }},
                "780n-vulkan": {"adapter": null, "env": {"POCKET_ADAPTER": "780n"}, "tests": {
                    "lib a": cell("skip", &["no GPU: skipped"], ""),
                }},
            },
            "problems": [
                {"config": "5060-dx12", "kind": "failed", "tests": ["taa b"]},
                {"config": "780n-vulkan", "kind": "no_adapter", "tests": []},
                {"config": "780n-vulkan", "kind": "no_gpu", "tests": ["lib a"]},
            ],
        });
        let mut step = StepResult::new("gpus");
        read(&report, &mut step, &paths());
        assert_eq!(step.verdict, Verdict::Fail);
        let codes: Vec<&str> = step.errors.iter().map(|p| p.code.as_str()).collect();
        assert_eq!(
            codes,
            [
                "gpus.test_failed",
                "gpus.test_failed",
                "gpus.no_gpu",
                "gpus.no_gpu"
            ]
        );
        assert_eq!(step.errors[0].message, "5060-dx12 taa b: FAIL: off by 28");
        assert_eq!(
            step.errors[0].detail["log"],
            json!("out/check/gpus/5060-dx12/taa.log")
        );
        assert_eq!(step.warnings.len(), 2, "a skip is never a pass");
        assert_eq!(
            step.warnings[0].message,
            "5060-dx12-minimal lib a: no GPU timestamps: skipped"
        );
        assert_eq!(
            step.summary,
            "5060-dx12, 5060-dx12-minimal, 780n-vulkan: 1 passed, 2 skipped \
             (5060-dx12-minimal 1, 780n-vulkan 1), 2 failed"
        );
    }

    #[test]
    fn a_clean_run_passes_with_its_skips_shown() {
        let report = json!({
            "built": {"lib": "2026-10-16 06:00:00"},
            "configs": {"a-dx12-minimal": {"adapter": "A", "env": {},
                "binaries": {"lib": {"exit": 0, "seconds": 1.0, "tests": 2}},
                "tests": {
                    "lib t": cell("skip", &["no GPU timestamps: placement not checked"], ""),
                    "lib u": cell("pass", &[], ""),
                }}},
            "problems": [],
        });
        let mut step = StepResult::new("gpus");
        read(&report, &mut step, &paths());
        assert_eq!(step.verdict, Verdict::Pass);
        assert_eq!(step.warnings[0].code, "gpus.skipped");
        assert_eq!(
            step.summary,
            "a-dx12-minimal: 1 passed, 1 skipped (a-dx12-minimal 1), 0 failed"
        );
    }

    #[test]
    fn a_binary_that_exits_badly_or_lists_nothing_fails() {
        // `taa` crashed after its tests reported ok; `lod` failed a test, which explains its exit;
        // `neural`'s listing timed out; `hiz` was built but listed no tests.
        let report = json!({
            "built": {"lib": "", "taa": "", "lod": "", "neural": "", "hiz": ""},
            "configs": {"a-vulkan": {"adapter": "A", "env": {},
                "binaries": {
                    "lib": {"exit": 0, "seconds": 1.0, "tests": 1},
                    "taa": {"exit": 3221225477u64, "seconds": 1.0, "tests": 1},
                    "lod": {"exit": 101, "seconds": 1.0, "tests": 1},
                    "neural": {"exit": "timeout", "seconds": 300, "tests": 0},
                },
                "tests": {
                    "lib a": cell("pass", &[], ""),
                    "taa b": cell("pass", &[], ""),
                    "lod c": cell("FAIL", &[], "off by 1"),
                }}},
            "problems": [{"config": "a-vulkan", "kind": "failed", "tests": ["lod c"]}],
        });
        let mut step = StepResult::new("gpus");
        read(&report, &mut step, &paths());
        assert_eq!(step.verdict, Verdict::Fail);
        let messages: Vec<(&str, &str)> = step
            .errors
            .iter()
            .map(|p| (p.code.as_str(), p.message.as_str()))
            .collect();
        assert_eq!(
            messages,
            [
                ("gpus.test_failed", "a-vulkan lod c: FAIL: off by 1"),
                (
                    "gpus.binary_failed",
                    "a-vulkan neural: listing its tests timed out"
                ),
                (
                    "gpus.binary_failed",
                    "a-vulkan taa: exit 3221225477 although none of its tests failed"
                ),
                ("gpus.binary_failed", "a-vulkan hiz: listed no tests"),
            ]
        );
        assert_eq!(
            step.errors[2].detail["log"],
            json!("out/check/gpus/a-vulkan/taa.log")
        );
        assert_eq!(
            step.summary,
            "a-vulkan: 2 passed, 0 skipped, 1 failed, 3 test binaries failed"
        );
    }

    #[test]
    fn skips_past_the_limit_are_counted() {
        let tests: serde_json::Map<String, Value> = (0..MAX_PROBLEMS + 5)
            .map(|i| {
                (
                    format!("lib t{i:02}"),
                    cell("skip", &["no ray queries: skipped"], ""),
                )
            })
            .collect();
        let report = json!({"configs": {"a-dx12-minimal": {"adapter": "A", "env": {},
            "tests": tests}}, "problems": []});
        let mut step = StepResult::new("gpus");
        read(&report, &mut step, &paths());
        assert_eq!(step.verdict, Verdict::Pass);
        assert_eq!(step.warnings.len(), MAX_PROBLEMS);
        assert_eq!(step.warnings[MAX_PROBLEMS - 2].code, "gpus.skipped");
        let last = &step.warnings[MAX_PROBLEMS - 1];
        assert_eq!(last.code, "gpus.more_skipped");
        assert_eq!(
            last.message,
            "6 more skipped tests: out/check/gpus.json lists every one"
        );
        assert_eq!(
            step.summary,
            "a-dx12-minimal: 0 passed, 25 skipped (a-dx12-minimal 25), 0 failed"
        );
    }

    #[test]
    fn the_exit_status_decides_with_the_report() {
        let line = "python tools/gpu_matrix.py --configs machine";
        let fresh = || {
            let mut s = StepResult::new("gpus");
            s.log = Some("out/check/gpus.log".into());
            s
        };
        // Exit 3: no adapter but software ones, so the step is skipped and keeps its log.
        let left = json!({"machine": {"left_out": ["WARP (dx12): a software adapter"]}});
        let s = judge(fresh(), line, &outcome(Some(3)), Some(&left), &paths());
        assert_eq!(s.verdict, Verdict::Skipped);
        assert_eq!(
            s.skipped_because.as_deref(),
            Some("no GPU adapter on this machine; left out: WARP (dx12): a software adapter")
        );
        assert_eq!(s.log.as_deref(), Some("out/check/gpus.log"));
        // No report at all.
        let s = judge(fresh(), line, &outcome(Some(1)), None, &paths());
        assert_eq!(s.verdict, Verdict::Fail);
        assert_eq!(s.errors[0].code, "check.command_failed");
        // A failing exit the report does not explain still fails.
        let clean = json!({"configs": {"a-dx12": {"adapter": "A", "env": {},
            "tests": {"lib a": cell("pass", &[], "")}}}, "problems": []});
        let s = judge(fresh(), line, &outcome(Some(1)), Some(&clean), &paths());
        assert_eq!(s.verdict, Verdict::Fail);
        let codes: Vec<&str> = s.errors.iter().map(|p| p.code.as_str()).collect();
        assert_eq!(codes, ["check.command_failed"]);
        // A failing exit the report explains is reported once, by the report.
        let failed = json!({"configs": {"a-dx12": {"adapter": "A", "env": {},
            "tests": {"lib a": cell("FAIL", &[], "off")}}},
            "problems": [{"config": "a-dx12", "kind": "failed", "tests": ["lib a"]}]});
        let s = judge(fresh(), line, &outcome(Some(1)), Some(&failed), &paths());
        let codes: Vec<&str> = s.errors.iter().map(|p| p.code.as_str()).collect();
        assert_eq!(codes, ["gpus.test_failed"]);
        // A clean report and exit 0 pass.
        let s = judge(fresh(), line, &outcome(Some(0)), Some(&clean), &paths());
        assert_eq!(s.verdict, Verdict::Pass);
    }

    #[test]
    fn without_python_the_step_is_skipped() {
        let root = PathBuf::from("nowhere");
        let env = Env {
            out: root.join("out").join("check"),
            root,
            cargo: "cargo".into(),
            host: "x86_64-pc-windows-msvc".into(),
            llvm: PathBuf::new(),
        };
        let s = run(&env, None, None);
        assert_eq!(s.verdict, Verdict::Skipped);
        assert_eq!(
            s.skipped_because.as_deref(),
            Some("no Python 3 to run tools/gpu_matrix.py (POCKET_PYTHON, python or python3)")
        );
    }

    #[test]
    fn the_default_auto_configuration_runs_when_test_did_not_run_it() {
        assert_eq!(default_auto(true, &[]), None);
        assert_eq!(
            default_auto(false, &[]).as_deref(),
            Some("the test step did not run in this check")
        );
        assert_eq!(
            default_auto(true, &["POCKET_ADAPTER", "POCKET_BACKEND"]).as_deref(),
            Some("POCKET_ADAPTER and POCKET_BACKEND set, so the test step ran another device")
        );
    }

    #[test]
    fn nothing_to_run_says_what_was_left_out() {
        assert_eq!(nothing_to_run(None), "no GPU adapter on this machine");
        let report = json!({"machine": {"left_out": [
            "Microsoft Basic Render Driver (dx12): a software adapter"]}});
        assert_eq!(
            nothing_to_run(Some(&report)),
            "no GPU adapter on this machine; left out: Microsoft Basic Render Driver (dx12): a \
             software adapter"
        );
    }
}
