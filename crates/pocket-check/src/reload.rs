//! Reload equivalence (docs/spec/checks.md 8.5; charter 3.7; hot-update.md 9): reloading the
//! project's unchanged scripts mid-run changes no world hash. Run A has no reloads; run B gets
//! `scripts.apply {force: true}` at each boundary of `[reload] at`, the command an edit uses, which
//! the runtime turns into a `scripts.swap` Host write. The two go through spec-persist's lockstep,
//! hashes compared at every tick; B's recording must show the swap at every reload point, since an
//! unforced reload of an unchanged bundle swaps nothing and would make the check vacuous.

use pocket_contract::{Problem, detail};
use pocket_link::Source;
use pocket_persist::replay::{
    MemorySink, Record, RecordOptions, RecordedWrite, Replay, canonical_json,
};
use pocket_persist::{LockstepOptions, Side, lockstep};
use pocket_sim::Tick;
use serde_json::json;

use crate::report::StepResult;
use crate::runs::{Subject, divergence_json, stopped};

fn one(subject: &Subject, seed: u64, at: &[u64], step: &mut StepResult) -> Result<(), Problem> {
    let mut a = subject.game(seed)?;
    let mut b = subject.game(seed)?;
    let sink = MemorySink::new();
    b.record(
        Box::new(sink.clone()),
        RecordOptions {
            keyframe_every: 0,
            ..RecordOptions::default()
        },
    )?;
    let inputs = &subject.inputs;
    let mut seq = 0u64;
    let mut writes = |t: Tick, side: Side| {
        let mut v = Vec::new();
        if side == Side::Actual && at.contains(&(t.0 - 1)) {
            seq += 1;
            v.push(RecordedWrite {
                source: Source::Developer(0),
                seq,
                name: "scripts.apply".to_owned(),
                params: canonical_json(&json!({"force": true})),
            });
        }
        v.extend(inputs.at(t.0).iter().map(|i| i.recorded()));
        v
    };
    let report = lockstep(
        &mut a,
        &mut b,
        &mut writes,
        LockstepOptions::until(Tick(subject.ticks())),
    );
    if let Some(d) = report.divergence {
        let tick = d.at.tick.0;
        let reload_at = at.iter().copied().filter(|r| *r < tick).max();
        step.error(Problem::new(
            "reload.diverged",
            format!(
                "{} seed {seed}: reloading the unchanged scripts at {reload_at:?} changed the world at tick {tick}{}; a script keeps state outside the world",
                subject.name,
                d.sections.first().map_or(String::new(), |s| format!(" in {s}"))
            ),
            detail([
                ("project", json!(subject.name)),
                ("seed", json!(seed)),
                ("tick", json!(tick)),
                ("reload_at", json!(reload_at)),
                ("divergence", divergence_json(&d)),
            ]),
        ));
        return Ok(());
    }
    b.finish_recording()?;
    let replay = Replay::read(&sink.bytes())?;
    for &r in at.iter().filter(|r| **r < subject.ticks()) {
        let swapped = replay.records().iter().any(|rec| {
            matches!(rec, Record::Tick { tick, writes, .. }
                if tick.0 == r + 1 && writes.iter().any(|w| w.name == "scripts.swap"))
        });
        if !swapped {
            step.error(Problem::new(
                "reload.not_performed",
                format!(
                    "{} seed {seed}: no scripts.swap was recorded at boundary {r}, so the reload there proves nothing",
                    subject.name
                ),
                detail([("project", json!(subject.name)), ("seed", json!(seed)), ("at", json!(r))]),
            ));
        }
    }
    Ok(())
}

/// The reload check over the seeds.
pub fn check(subject: &Subject, seeds: &[u64]) -> StepResult {
    let Some(cfg) = subject.config.reload.clone() else {
        return StepResult::skipped("reload", "check.toml has no [reload]");
    };
    let mut step = StepResult::new("reload");
    for &seed in seeds {
        if let Err(p) = one(subject, seed, &cfg.at, &mut step) {
            step.error(stopped("reload", subject, seed, p));
        }
    }
    step.measure("reloads", (cfg.at.len() * seeds.len()) as f64, "count");
    step.summary = format!(
        "{} seeds x {} ticks, unchanged scripts reloaded at {:?}",
        seeds.len(),
        subject.ticks(),
        cfg.at
    );
    step
}
