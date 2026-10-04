//! The report every check prints (docs/spec/checks.md 10): the same shape for `cargo xtask check`
//! and `pocket check`, its human form, the exit codes (11) and the commit trailer (12).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// At most this many errors and warnings per step; the rest are counted (checks.md 10.1).
pub const MAX_PROBLEMS: usize = 20;

/// What `pocket check --json` prints as well (checks.md 2). Reading one, the fields a minimal
/// implementation may leave out (the fingerprint, the timings, empty lists) take their defaults.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub format: u32,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub machine: Machine,
    #[serde(default)]
    pub started_utc: String,
    #[serde(default)]
    pub duration_ms: u64,
    pub verdict: Verdict,
    pub steps: Vec<StepResult>,
}

/// The fingerprint of budgets.md 3. `gpus` stays empty until `pocket` reports what wgpu sees.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Machine {
    pub id: String,
    pub cpu: String,
    pub logical_cores: u32,
    pub gpus: Vec<GpuInfo>,
    pub os: String,
    pub on_ac_power: Option<bool>,
    pub power_plan: Option<String>,
    pub chrome: Option<String>,
    pub rustc: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub driver: Option<String>,
    pub backend: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Pass,
    Fail,
    Inconclusive,
    Skipped,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Inconclusive => "INCO",
            Verdict::Skipped => "SKIP",
        }
    }
}

/// The error protocol's `{code, message, detail}` (shared/contract/errors.md). xtask keeps its own
/// copy of the shape instead of linking `pocket-contract`, so a compile error there is a finding of
/// the check rather than a checker that does not build (checks.md 2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub detail: Map<String, Value>,
}

impl Problem {
    pub fn new(code: &str, message: impl Into<String>, detail: Value) -> Problem {
        let detail = match detail {
            Value::Object(map) => map,
            Value::Null => Map::new(),
            other => {
                let mut map = Map::new();
                map.insert("value".into(), other);
                map
            }
        };
        Problem {
            code: code.into(),
            message: message.into(),
            detail,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepResult {
    pub name: String,
    pub verdict: Verdict,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub errors: Vec<Problem>,
    #[serde(default)]
    pub more_errors: u32,
    #[serde(default)]
    pub warnings: Vec<Problem>,
    #[serde(default)]
    pub measurements: Vec<Measurement>,
    #[serde(default)]
    pub skipped_because: Option<String>,
    #[serde(default)]
    pub log: Option<String>,
}

impl StepResult {
    pub fn new(name: &str) -> StepResult {
        StepResult {
            name: name.into(),
            verdict: Verdict::Pass,
            duration_ms: 0,
            summary: String::new(),
            errors: Vec::new(),
            more_errors: 0,
            warnings: Vec::new(),
            measurements: Vec::new(),
            skipped_because: None,
            log: None,
        }
    }

    pub fn skipped(name: &str, because: impl Into<String>) -> StepResult {
        let because = because.into();
        let mut step = StepResult::new(name);
        step.verdict = Verdict::Skipped;
        step.summary = format!("skipped: {because}");
        step.skipped_because = Some(because);
        step
    }

    /// Records an error and makes the step fail; past `MAX_PROBLEMS` only the count grows.
    pub fn error(&mut self, problem: Problem) {
        self.verdict = Verdict::Fail;
        if self.errors.len() < MAX_PROBLEMS {
            self.errors.push(problem);
        } else {
            self.more_errors += 1;
        }
    }

    /// An error that makes the step inconclusive (a tool missing), unless it already failed.
    pub fn inconclusive(&mut self, problem: Problem) {
        if self.verdict != Verdict::Fail {
            self.verdict = Verdict::Inconclusive;
        }
        if self.errors.len() < MAX_PROBLEMS {
            self.errors.push(problem);
        } else {
            self.more_errors += 1;
        }
    }

    /// An error, or an inconclusive finding when it is a missing tool.
    pub fn problem(&mut self, problem: Problem) {
        if problem.code == "check.tool_missing" {
            self.inconclusive(problem);
        } else {
            self.error(problem);
        }
    }

    pub fn warn(&mut self, problem: Problem) {
        if self.warnings.len() < MAX_PROBLEMS {
            self.warnings.push(problem);
        }
    }

    pub fn measure(&mut self, name: &str, value: f64, unit: &str) {
        self.measurements.push(Measurement {
            name: name.into(),
            value,
            unit: unit.into(),
            stat: "value".into(),
            samples: 1,
            limit: None,
            limit_kind: None,
        });
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Measurement {
    pub name: String,
    pub value: f64,
    pub unit: String,
    #[serde(default)]
    pub stat: String,
    #[serde(default)]
    pub samples: u32,
    #[serde(default)]
    pub limit: Option<f64>,
    #[serde(default)]
    pub limit_kind: Option<String>,
}

/// The overall verdict: a failed step fails the run; otherwise an inconclusive one makes it
/// inconclusive; skipped steps (by request or because something is not built yet) do neither.
pub fn overall(steps: &[StepResult]) -> Verdict {
    if steps.iter().any(|s| s.verdict == Verdict::Fail) {
        Verdict::Fail
    } else if steps.iter().any(|s| s.verdict == Verdict::Inconclusive) {
        Verdict::Inconclusive
    } else if steps.iter().all(|s| s.verdict == Verdict::Skipped) && !steps.is_empty() {
        Verdict::Skipped
    } else {
        Verdict::Pass
    }
}

/// checks.md 11: 1 if a step failed, else 3 if one was inconclusive, else 0. Exit code 2 (usage
/// and configuration) is decided before any step runs, in `main`.
pub fn exit_code(verdict: Verdict) -> u8 {
    match verdict {
        Verdict::Fail => 1,
        Verdict::Inconclusive => 3,
        Verdict::Pass | Verdict::Skipped => 0,
    }
}

pub fn seconds(ms: u64) -> String {
    if ms >= 60_000 {
        format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1000)
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

/// The human form (checks.md 10.2): a line per step, each failure as `code: message` with its
/// detail on one line, then the verdict.
pub fn human(report: &Report, report_path: &str) -> String {
    let mut out = String::new();
    for step in &report.steps {
        out.push_str(&format!(
            "{}  {:<12} {:>7}  {}\n",
            step.verdict.label(),
            step.name,
            seconds(step.duration_ms),
            step.summary
        ));
    }
    for step in &report.steps {
        if step.errors.is_empty() && step.warnings.is_empty() {
            continue;
        }
        out.push('\n');
        out.push_str(&format!("{}:\n", step.name));
        for p in &step.errors {
            out.push_str(&format!("  {}: {}\n", p.code, p.message));
            if !p.detail.is_empty() {
                out.push_str(&format!("    {}\n", Value::Object(p.detail.clone())));
            }
        }
        if step.more_errors > 0 {
            out.push_str(&format!("  ... and {} more\n", step.more_errors));
        }
        for p in &step.warnings {
            out.push_str(&format!("  warning {}: {}\n", p.code, p.message));
        }
        if let Some(log) = &step.log {
            out.push_str(&format!("  log: {log}\n"));
        }
    }
    let count = |v: Verdict| report.steps.iter().filter(|s| s.verdict == v).count();
    out.push_str(&format!(
        "\n{}  ({} failed, {} inconclusive, {} passed, {} skipped) in {}; report: {}\n",
        report.verdict.label(),
        count(Verdict::Fail),
        count(Verdict::Inconclusive),
        count(Verdict::Pass),
        count(Verdict::Skipped),
        seconds(report.duration_ms),
        report_path
    ));
    out
}

/// The `Checked:` trailer of checks.md 12, built from the steps' verdicts; `mode` is `full`,
/// `quick`, or `partial` when `--only` or `--skip` left steps out.
pub fn trailer(report: &Report, mode: &str) -> String {
    let verdict = |name: &str| {
        report
            .steps
            .iter()
            .find(|s| s.name == name)
            .map_or("not run".to_string(), |s| s.verdict.label().to_lowercase())
    };
    let deps = match verdict("deps").as_str() {
        "pass" => "ok".to_string(),
        other => other.to_string(),
    };
    format!(
        "Checked: {} {}; deps {}; perf {}",
        report.verdict.label().to_lowercase(),
        mode,
        deps,
        verdict("perf")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(name: &str, verdict: Verdict) -> StepResult {
        let mut s = StepResult::new(name);
        s.verdict = verdict;
        s
    }

    #[test]
    fn exit_codes_follow_the_order_of_checks_md_11() {
        let pass = [step("a", Verdict::Pass), step("b", Verdict::Skipped)];
        assert_eq!(exit_code(overall(&pass)), 0);
        let inconclusive = [step("a", Verdict::Pass), step("b", Verdict::Inconclusive)];
        assert_eq!(exit_code(overall(&inconclusive)), 3);
        let fail = [step("a", Verdict::Inconclusive), step("b", Verdict::Fail)];
        assert_eq!(exit_code(overall(&fail)), 1);
    }

    #[test]
    fn errors_are_capped_and_counted() {
        let mut s = StepResult::new("clippy");
        for i in 0..25 {
            s.error(Problem::new(
                "clippy.warning",
                format!("warning {i}"),
                json!({}),
            ));
        }
        assert_eq!(s.errors.len(), MAX_PROBLEMS);
        assert_eq!(s.more_errors, 5);
        assert_eq!(s.verdict, Verdict::Fail);
    }

    #[test]
    fn a_problem_serializes_in_the_error_protocol_shape() {
        let p = Problem::new("deps.crate_missing", "m", json!({"crate": "pocket-x"}));
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(
            v,
            json!({"code": "deps.crate_missing", "message": "m", "detail": {"crate": "pocket-x"}})
        );
    }

    #[test]
    fn the_trailer_records_deps_and_perf() {
        let report = Report {
            format: 1,
            command: "cargo xtask check".into(),
            commit: None,
            machine: Machine::default(),
            started_utc: String::new(),
            duration_ms: 0,
            verdict: Verdict::Pass,
            steps: vec![
                step("deps", Verdict::Pass),
                step("fmt", Verdict::Pass),
                step("perf", Verdict::Skipped),
            ],
        };
        assert_eq!(
            trailer(&report, "full"),
            "Checked: pass full; deps ok; perf skip"
        );
        let text = human(&report, "out/check/report.json");
        assert!(text.contains("PASS  deps"), "{text}");
        assert!(text.ends_with("(0 failed, 0 inconclusive, 2 passed, 1 skipped) in 0.0s; report: out/check/report.json\n"));
    }
}
