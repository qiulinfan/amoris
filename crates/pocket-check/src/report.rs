//! The report `pocket check` prints (docs/spec/checks.md 10.1): the same shape as
//! `cargo xtask check`'s, so `xtask` merges a project's rows into its own steps, and the exit codes
//! of checks.md 11.

use pocket_contract::Problem;
use serde::{Deserialize, Serialize};

/// At most this many errors and warnings per step; the rest are counted.
pub const MAX_PROBLEMS: usize = 20;

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

/// One number a step measured.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    pub name: String,
    pub value: f64,
    pub unit: String,
    pub stat: String,
    pub samples: u32,
    pub limit: Option<f64>,
    pub limit_kind: Option<String>,
}

/// One check's result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepResult {
    pub name: String,
    pub verdict: Verdict,
    pub duration_ms: u64,
    pub summary: String,
    pub errors: Vec<Problem>,
    pub more_errors: u32,
    pub warnings: Vec<Problem>,
    pub measurements: Vec<Measurement>,
    pub skipped_because: Option<String>,
    pub log: Option<String>,
}

impl StepResult {
    pub fn new(name: &str) -> StepResult {
        StepResult {
            name: name.to_owned(),
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

    pub fn skipped(name: &str, because: &str) -> StepResult {
        let mut s = StepResult::new(name);
        s.verdict = Verdict::Skipped;
        s.summary = because.to_owned();
        s.skipped_because = Some(because.to_owned());
        s
    }

    /// A failure: the step fails, the problem is kept (at most [`MAX_PROBLEMS`]).
    pub fn error(&mut self, p: Problem) {
        self.verdict = Verdict::Fail;
        if self.errors.len() < MAX_PROBLEMS {
            self.errors.push(p);
        } else {
            self.more_errors += 1;
        }
    }

    /// A problem that leaves the step without a verdict (a tool missing), unless it already failed.
    pub fn inconclusive(&mut self, p: Problem) {
        if self.verdict != Verdict::Fail {
            self.verdict = Verdict::Inconclusive;
        }
        if self.errors.len() < MAX_PROBLEMS {
            self.errors.push(p);
        } else {
            self.more_errors += 1;
        }
    }

    /// A finding that leaves the verdict alone.
    pub fn warn(&mut self, p: Problem) {
        if self.warnings.len() < MAX_PROBLEMS {
            self.warnings.push(p);
        }
    }

    /// A count or a value, without a limit.
    pub fn measure(&mut self, name: &str, value: f64, unit: &str) {
        self.measurements.push(Measurement {
            name: name.to_owned(),
            value,
            unit: unit.to_owned(),
            stat: "value".to_owned(),
            samples: 1,
            limit: None,
            limit_kind: None,
        });
    }
}

/// The machine's fingerprint (budgets.md 3); `pocket check` fills what it knows cheaply.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Machine {
    pub id: String,
    pub cpu: String,
    pub logical_cores: u32,
    pub os: String,
    pub rustc: String,
}

/// The whole report.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub format: u32,
    pub command: String,
    pub commit: Option<String>,
    pub machine: Machine,
    pub started_utc: String,
    pub duration_ms: u64,
    pub verdict: Verdict,
    pub steps: Vec<StepResult>,
}

impl Report {
    /// A report of `steps`, its verdict theirs: a failure fails it, else an inconclusive step makes
    /// it inconclusive, else it passes (or is skipped when every step was).
    pub fn of(command: &str, steps: Vec<StepResult>, duration_ms: u64) -> Report {
        let verdict = if steps.iter().any(|s| s.verdict == Verdict::Fail) {
            Verdict::Fail
        } else if steps.iter().any(|s| s.verdict == Verdict::Inconclusive) {
            Verdict::Inconclusive
        } else if !steps.is_empty() && steps.iter().all(|s| s.verdict == Verdict::Skipped) {
            Verdict::Skipped
        } else {
            Verdict::Pass
        };
        Report {
            format: 1,
            command: command.to_owned(),
            commit: None,
            machine: Machine {
                os: std::env::consts::OS.to_owned(),
                ..Machine::default()
            },
            started_utc: String::new(),
            duration_ms,
            verdict,
            steps,
        }
    }

    /// A refused invocation (checks.md 11, exit code 2): one step named `check` with the problem.
    pub fn usage(command: &str, problem: Problem) -> Report {
        let mut step = StepResult::new("check");
        step.summary = problem.message.clone();
        step.error(problem);
        Report::of(command, vec![step], 0)
    }

    /// The exit code of checks.md 11 for this report (2 is the caller's, for a refused invocation).
    pub fn exit_code(&self) -> i32 {
        exit_code(self.verdict)
    }

    /// The human form (checks.md 10.2): one line per step, each failure, the verdict.
    pub fn human(&self) -> String {
        let mut out = String::new();
        for s in &self.steps {
            out.push_str(&format!(
                "{:<5} {:<12} {:>6.1}s  {}\n",
                s.verdict.label(),
                s.name,
                s.duration_ms as f64 / 1000.0,
                s.summary
            ));
        }
        for s in &self.steps {
            for e in &s.errors {
                let detail = serde_json::to_string(&e.detail).unwrap_or_default();
                out.push_str(&format!("{}: {} {detail}\n", e.code, e.message));
            }
        }
        out.push_str(&format!("{}\n", self.verdict.label()));
        out
    }
}

/// The exit code a verdict gives (checks.md 11): 0 pass or skipped, 1 fail, 3 inconclusive.
pub fn exit_code(v: Verdict) -> i32 {
    match v {
        Verdict::Pass | Verdict::Skipped => 0,
        Verdict::Fail => 1,
        Verdict::Inconclusive => 3,
    }
}
