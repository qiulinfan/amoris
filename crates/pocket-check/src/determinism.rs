//! Determinism (docs/spec/checks.md 8.2; charter 3.7): the same seed and the same inputs give the
//! same world hash at every tick, in one process (two games stepped alternately: spec-persist's
//! lockstep, which catches state shared through process globals) and across processes (`pocket
//! check --chain-only` run again as a child, which catches hash-map seeds, addresses and thread
//! identities). The cross-build and cross-target variants are `cargo xtask check`'s.

use pocket_contract::{Problem, detail};
use pocket_persist::{LockstepOptions, first_divergence, lockstep};
use pocket_sim::Tick;
use serde_json::json;

use crate::report::StepResult;
use crate::runs::{self, Subject, divergence_json, stopped};
use crate::{Child, ChildAnswer, ChildRequest};

/// `determinism.diverged {project, seed, variant, divergence}`.
pub fn diverged(
    subject: &Subject,
    seed: u64,
    variant: &str,
    d: &pocket_persist::Divergence,
) -> Problem {
    Problem::new(
        "determinism.diverged",
        format!(
            "{} with seed {seed} diverged at tick {} ({variant}){}",
            subject.name,
            d.at.tick.0,
            d.sections
                .first()
                .map_or(String::new(), |s| format!(" in {s}"))
        ),
        detail([
            ("project", json!(subject.name)),
            ("seed", json!(seed)),
            ("variant", json!(variant)),
            ("divergence", divergence_json(d)),
        ]),
    )
}

/// The same-process variant: two games of the seed in lockstep.
pub fn same_process(
    subject: &Subject,
    seed: u64,
) -> Result<Option<pocket_persist::Divergence>, Problem> {
    let (mut a, mut b) = (subject.game(seed)?, subject.game(seed)?);
    let inputs = &subject.inputs;
    let mut writes = |t: Tick, _| inputs.at(t.0).iter().map(|i| i.recorded()).collect();
    let r = lockstep(
        &mut a,
        &mut b,
        &mut writes,
        LockstepOptions::until(Tick(subject.ticks())),
    );
    Ok(r.divergence)
}

/// The cross-process variant: the child's chain against this process's, and where they differ,
/// the child's snapshot at that tick diffed with this process's. A divergence comes with the
/// problem that kept its fields from being located, when the child could not give that snapshot.
pub fn cross_process(
    subject: &Subject,
    seed: u64,
    mine: &runs::Chain,
    child: &Child,
) -> Result<Option<(pocket_persist::Divergence, Option<Problem>)>, Problem> {
    let ChildAnswer::Chain(theirs) = child(subject, &ChildRequest::Chain { seed })? else {
        return Err(Problem::new(
            "check.command_failed",
            "the child process answered a chain request with something else",
            detail([]),
        ));
    };
    let Some(mut d) = first_divergence(mine, &theirs) else {
        return Ok(None);
    };
    let tick = d.at.tick.0;
    let snap = match child(subject, &ChildRequest::Snapshot { seed, tick }) {
        Ok(ChildAnswer::Snapshot(snap)) => snap,
        Ok(_) => {
            let p = Problem::new(
                "check.command_failed",
                "the child process answered a snapshot request with something else",
                detail([]),
            );
            return Ok(Some((d, Some(p))));
        }
        Err(p) => return Ok(Some((d, Some(p)))),
    };
    {
        let here = runs::run_to(subject, seed, tick)?;
        let mut other = here.fork()?;
        other.restore(&snap)?;
        let located = runs::diverged(
            &here,
            &other,
            tick,
            d.expected.unwrap_or(snap.world_hash()),
            snap.world_hash(),
        );
        d.sections = located.sections;
        d.fields = located.fields;
        d.fields_truncated = located.fields_truncated;
    }
    Ok(Some((d, None)))
}

/// The determinism check over the seeds.
pub fn check(subject: &Subject, seeds: &[u64], child: Option<&Child>) -> StepResult {
    let mut step = StepResult::new("determinism");
    let mut ticks = 0u64;
    for &seed in seeds {
        let mine = match runs::chain(subject, seed, subject.ticks()) {
            Ok(c) => c,
            Err(p) => {
                step.error(stopped("determinism", subject, seed, p));
                continue;
            }
        };
        ticks += subject.ticks();
        match same_process(subject, seed) {
            Ok(Some(d)) => step.error(diverged(subject, seed, "same_process", &d)),
            Ok(None) => {}
            Err(p) => step.error(stopped("determinism", subject, seed, p)),
        }
        if let Some(child) = child {
            match cross_process(subject, seed, &mine, child) {
                Ok(Some((d, unlocated))) => {
                    step.error(diverged(subject, seed, "cross_process", &d));
                    if let Some(p) = unlocated {
                        step.warn(stopped("determinism", subject, seed, p));
                    }
                }
                Ok(None) => {}
                Err(p) => step.error(stopped("determinism", subject, seed, p)),
            }
        }
    }
    step.measure("ticks_compared", ticks as f64, "count");
    step.summary = format!(
        "{} seeds x {} ticks, same process{}",
        seeds.len(),
        subject.ticks(),
        if child.is_some() {
            " and cross process"
        } else {
            ""
        }
    );
    step
}
