//! Replay (docs/spec/checks.md 8.4; charter 3.3 and 3.7): a recorded run replays identically tick by
//! tick in a fresh process, and a perturbed copy of the recording names the first tick where it
//! diverges, never an earlier one.
//!
//! Two perturbations, both of the first recorded write at or after tick `ticks / 2` (an input of
//! tick T): dropped (checks.md 8.4, 3) and moved one tick later. Either must diverge, at T or later
//! for the dropped one and at T exactly for the moved one; a copy that verifies identically is
//! `replay.perturbation_undetected` (checks.md 14, Slice 1).

use std::path::Path;

use pocket_contract::{Problem, detail};
use pocket_persist::Divergence;
use pocket_persist::replay::{
    MemorySink, Record, RecordOptions, Replay, ReplayMode, ReplayOutcome, VerifyOptions, verify,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::report::StepResult;
use crate::runs::{self, Subject, divergence_json, stopped};
use crate::{Child, ChildAnswer, ChildRequest};

/// What verifying a replay found, as `pocket replay --verify --json` prints it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VerifySummary {
    pub ticks_run: u64,
    pub identical: bool,
    pub divergence: Option<Divergence>,
    pub stopped: Option<Problem>,
}

/// Verifies a replay's bytes against a fresh game (replay.md 3.4, `Verify`).
pub fn verify_bytes(bytes: &[u8]) -> Result<VerifySummary, Problem> {
    let replay = Replay::read(bytes)?;
    let mut game = pocket_runtime::Game::for_replay(&replay)?;
    let report = verify(&replay, &mut game, VerifyOptions::mode(ReplayMode::Verify));
    Ok(match report.outcome {
        ReplayOutcome::Identical => VerifySummary {
            ticks_run: report.ticks_run,
            identical: true,
            divergence: None,
            stopped: None,
        },
        ReplayOutcome::Diverged(d) => VerifySummary {
            ticks_run: report.ticks_run,
            identical: false,
            divergence: Some(*d),
            stopped: None,
        },
        ReplayOutcome::Stopped(_, p) => VerifySummary {
            ticks_run: report.ticks_run,
            identical: false,
            divergence: None,
            stopped: Some(p),
        },
    })
}

/// Records a run of the seed with the inputs: the replay's bytes.
pub fn record(subject: &Subject, seed: u64) -> Result<Vec<u8>, Problem> {
    let mut g = subject.game(seed)?;
    let sink = MemorySink::new();
    g.record(Box::new(sink.clone()), RecordOptions::default())?;
    for _ in 0..subject.ticks() {
        runs::advance(&mut g, &subject.inputs)?;
    }
    g.finish_recording()?;
    Ok(sink.bytes())
}

/// How a copy of the recording is perturbed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Perturb {
    /// The write is left out.
    Drop,
    /// The write is applied one boundary later.
    Delay,
}

/// A copy of the recording with the first write at or after tick `from` perturbed: its bytes and
/// T, the tick the write was an input of; `None` when no write is recorded from there on.
pub fn perturbed(bytes: &[u8], from: u64, how: Perturb) -> Result<Option<(Vec<u8>, u64)>, Problem> {
    let (header, mut records, complete) = Replay::read(bytes)?.into_parts();
    let found = records.iter().position(
        |r| matches!(r, Record::Tick { tick, writes, .. } if tick.0 >= from && !writes.is_empty()),
    );
    let Some(i) = found else {
        return Ok(None);
    };
    let (t, w) = match &mut records[i] {
        Record::Tick { tick, writes, .. } => (tick.0, writes.remove(0)),
        _ => return Ok(None),
    };
    if how == Perturb::Delay {
        let next = records[i + 1..]
            .iter_mut()
            .find(|r| matches!(r, Record::Tick { tick, .. } if tick.0 == t + 1));
        match next {
            Some(Record::Tick { writes, .. }) => writes.insert(0, w),
            _ => return Ok(None),
        }
    }
    let copy = Replay::from_records(header, records, complete)?.to_bytes()?;
    Ok(Some((copy, t)))
}

fn slug(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

fn verify_file(
    subject: &Subject,
    path: &Path,
    bytes: &[u8],
    child: Option<&Child>,
) -> Result<VerifySummary, Problem> {
    match child {
        Some(c) => match c(
            subject,
            &ChildRequest::Verify {
                replay: path.to_path_buf(),
            },
        )? {
            ChildAnswer::Verify(s) => Ok(*s),
            _ => Err(Problem::new(
                "check.command_failed",
                "the child process answered a verify request with something else",
                detail([]),
            )),
        },
        None => verify_bytes(bytes),
    }
}

/// `replay.diverged {project, seed, divergence}` or the problem that stopped the replay.
fn not_identical(subject: &Subject, seed: u64, s: &VerifySummary) -> Problem {
    match (&s.divergence, &s.stopped) {
        (Some(d), _) => Problem::new(
            "replay.diverged",
            format!(
                "{} seed {seed}: the recording diverged from its replay at tick {}",
                subject.name, d.at.tick.0
            ),
            detail([
                ("project", json!(subject.name)),
                ("seed", json!(seed)),
                ("divergence", divergence_json(d)),
            ]),
        ),
        (None, Some(p)) => {
            let mut p = p.clone();
            p.detail.insert("project".into(), json!(subject.name));
            p.detail.insert("seed".into(), json!(seed));
            p
        }
        (None, None) => Problem::new(
            "replay.diverged",
            format!("{} seed {seed}: the replay was not identical", subject.name),
            detail([("project", json!(subject.name)), ("seed", json!(seed))]),
        ),
    }
}

fn localize(
    subject: &Subject,
    seed: u64,
    bytes: &[u8],
    dir: &Path,
    child: Option<&Child>,
    how: Perturb,
    step: &mut StepResult,
) -> Result<(), Problem> {
    let from = subject.ticks() / 2;
    let Some((copy, t)) = perturbed(bytes, from, how)? else {
        step.warn(Problem::new(
            "replay.nothing_to_perturb",
            format!(
                "{} seed {seed}: no write is recorded at or after tick {from}, so the replay's localization was not tried",
                subject.name
            ),
            detail([("project", json!(subject.name)), ("seed", json!(seed))]),
        ));
        return Ok(());
    };
    let label = if how == Perturb::Drop {
        "dropped"
    } else {
        "delayed"
    };
    let path = dir.join(format!("{}-{seed}-{label}.p3dreplay", slug(&subject.name)));
    let _ = std::fs::write(&path, &copy);
    let s = verify_file(subject, &path, &copy, child)?;
    let reported = s.divergence.as_ref().map(|d| d.at.tick.0);
    let early = match how {
        Perturb::Drop => reported.is_some_and(|r| r < t),
        Perturb::Delay => reported.is_some_and(|r| r != t),
    };
    match reported {
        Some(r) if early => step.error(Problem::new(
            "replay.divergence_too_early",
            format!(
                "{} seed {seed}: the copy with the write of tick {t} {label} was reported diverging at tick {r}, and nothing differed before {t}",
                subject.name
            ),
            detail([
                ("project", json!(subject.name)),
                ("seed", json!(seed)),
                ("reported", json!(r)),
                ("expected_at_least", json!(t)),
                ("perturbation", json!(label)),
            ]),
        )),
        Some(r) => step.measure(&format!("seed{seed}.{label}_write_found_at"), r as f64, "tick"),
        None => step.error(Problem::new(
            "replay.perturbation_undetected",
            format!(
                "{} seed {seed}: the copy with the write of tick {t} {label} replayed {}",
                subject.name,
                if s.identical { "identically" } else { "without naming a tick" }
            ),
            detail([
                ("project", json!(subject.name)),
                ("seed", json!(seed)),
                ("tick", json!(t)),
                ("perturbation", json!(label)),
                ("stopped", serde_json::to_value(&s.stopped).unwrap_or_default()),
            ]),
        )),
    }
    Ok(())
}

fn one(
    subject: &Subject,
    seed: u64,
    dir: &Path,
    child: Option<&Child>,
    step: &mut StepResult,
) -> Result<(), Problem> {
    let bytes = record(subject, seed)?;
    let path = dir.join(format!("{}-{seed}.p3dreplay", slug(&subject.name)));
    std::fs::write(&path, &bytes).map_err(|e| {
        Problem::new(
            "check.tool_missing",
            format!("{} cannot be written: {e}", path.display()),
            detail([("path", json!(path.display().to_string()))]),
        )
    })?;
    step.measure(
        &format!("seed{seed}.replay_bytes"),
        bytes.len() as f64,
        "bytes",
    );
    let s = verify_file(subject, &path, &bytes, child)?;
    if !s.identical {
        step.error(not_identical(subject, seed, &s));
        return Ok(());
    }
    localize(subject, seed, &bytes, dir, child, Perturb::Drop, step)?;
    localize(subject, seed, &bytes, dir, child, Perturb::Delay, step)
}

/// The replay check over the seeds; replay files go to `dir`.
pub fn check(subject: &Subject, seeds: &[u64], dir: &Path, child: Option<&Child>) -> StepResult {
    let mut step = StepResult::new("replay");
    if let Err(e) = std::fs::create_dir_all(dir) {
        step.error(Problem::new(
            "check.tool_missing",
            format!("{} cannot be made: {e}", dir.display()),
            detail([("path", json!(dir.display().to_string()))]),
        ));
        return step;
    }
    for &seed in seeds {
        if let Err(p) = one(subject, seed, dir, child, &mut step) {
            step.error(stopped("replay", subject, seed, p));
        }
    }
    step.summary = format!(
        "{} seeds x {} ticks recorded and verified{}; files in {}",
        seeds.len(),
        subject.ticks(),
        if child.is_some() {
            " in a fresh process"
        } else {
            ""
        },
        dir.display()
    );
    step
}
