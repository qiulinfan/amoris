//! Fork consistency (docs/spec/checks.md 8.3; charter 3.7): a fork is identical to its source, the
//! same inputs on the original and the fork give the same hashes, and acting only on a fork leaves
//! the original exactly as if it had never been forked.
//!
//! For each seed and fork point F, with K = `[fork] ticks`: R and A run to F, and R is never
//! forked; B and C are forks of A. Then, tick by tick, R, A, B and C get the run's inputs, C also
//! the branch's own, and they step one after the other: B must equal A at every tick, A must equal
//! R at every tick, and C must differ from A by the end.

use pocket_contract::{Problem, detail};
use pocket_persist::WorldHash;
use serde_json::json;

use crate::report::StepResult;
use crate::runs::{self, Subject, divergence_json, stopped};

fn hash_problem(
    code: &str,
    subject: &Subject,
    seed: u64,
    fork_at: u64,
    message: String,
    d: Option<&pocket_persist::Divergence>,
    extra: Option<(WorldHash, WorldHash)>,
) -> Problem {
    let mut p = Problem::new(
        code,
        message,
        detail([
            ("project", json!(subject.name)),
            ("seed", json!(seed)),
            ("fork_at", json!(fork_at)),
        ]),
    );
    if let Some(d) = d {
        p.detail.insert("tick".into(), json!(d.at.tick.0));
        p.detail.insert("divergence".into(), divergence_json(d));
    }
    if let Some((a, b)) = extra {
        p.detail.insert("hash_a".into(), json!(a.to_string()));
        p.detail.insert("hash_b".into(), json!(b.to_string()));
    }
    p
}

/// One fork point of one seed.
fn one(subject: &Subject, seed: u64, f: u64, k: u64, step: &mut StepResult) -> Result<(), Problem> {
    let mut r = runs::run_to(subject, seed, f)?;
    let mut a = runs::run_to(subject, seed, f)?;
    let mut b = a.fork()?;
    let mut c = a.fork()?;
    let (ha, hb) = (a.world_hash()?, b.world_hash()?);
    if ha != hb {
        step.error(hash_problem(
            "fork.not_identical",
            subject,
            seed,
            f,
            format!(
                "{} seed {seed}: the fork at tick {f} does not hash as its source",
                subject.name
            ),
            None,
            Some((ha, hb)),
        ));
        return Ok(());
    }
    let branch = subject.branch.shifted(f);
    let (mut branch_differs, mut original_changed) = (false, false);
    for t in f + 1..=f + k {
        runs::apply_inputs(&mut r, &subject.inputs, t)?;
        runs::apply_inputs(&mut a, &subject.inputs, t)?;
        runs::apply_inputs(&mut b, &subject.inputs, t)?;
        runs::apply_inputs(&mut c, &subject.inputs, t)?;
        runs::apply_inputs(&mut c, &branch, t)?;
        r.step()?;
        a.step()?;
        b.step()?;
        c.step()?;
        let (ha, hb) = (a.world_hash()?, b.world_hash()?);
        if ha != hb && !branch_differs {
            branch_differs = true;
            let d = runs::diverged(&a, &b, t, ha, hb);
            step.error(hash_problem(
                "fork.branch_differs",
                subject,
                seed,
                f,
                format!("{} seed {seed}: the same inputs on the original and the fork forked at {f} differ at tick {t}", subject.name),
                Some(&d),
                None,
            ));
        }
        let hr = r.world_hash()?;
        if ha != hr && !original_changed {
            original_changed = true;
            let d = runs::diverged(&r, &a, t, hr, ha);
            step.error(hash_problem(
                "fork.original_changed",
                subject,
                seed,
                f,
                format!("{} seed {seed}: forking at {f} and acting on the fork changed the original at tick {t}", subject.name),
                Some(&d),
                None,
            ));
        }
        if branch_differs && original_changed {
            break;
        }
    }
    if !branch.is_empty() && a.world_hash()? == c.world_hash()? {
        step.error(hash_problem(
            "fork.branch_inputs_ineffective",
            subject,
            seed,
            f,
            format!("{} seed {seed}: the branch's own inputs left the fork at {f} equal to the original after {k} ticks, which proves nothing", subject.name),
            None,
            None,
        ));
    }
    Ok(())
}

/// The fork check over the seeds and fork points.
pub fn check(subject: &Subject, seeds: &[u64]) -> StepResult {
    let Some(cfg) = subject.config.fork.clone() else {
        return StepResult::skipped("fork", "check.toml has no [fork]");
    };
    let mut step = StepResult::new("fork");
    let mut forks = 0u32;
    for &seed in seeds {
        for &f in &cfg.at {
            match one(subject, seed, f, cfg.ticks, &mut step) {
                Ok(()) => forks += 1,
                Err(p) => step.error(stopped("fork", subject, seed, p)),
            }
        }
    }
    step.measure("forks", f64::from(forks), "count");
    step.summary = format!(
        "{} seeds x {} fork points, {} ticks each{}",
        seeds.len(),
        cfg.at.len(),
        cfg.ticks,
        if subject.branch.is_empty() {
            ""
        } else {
            ", with branch inputs"
        }
    );
    step
}
