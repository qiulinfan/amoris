//! `scripts.profile` (docs/spec/debugger.md 14; docs/spec/server.md): where the scripts' steps go.
//! It forks the world, runs the fork some ticks with the script host's step profiler and answers
//! the profile; the world itself does not move and nothing is recorded. The fork runs the same
//! bundle from the same state, so its ticks are the ones the world would run next without new
//! commands, and the same state always gives the same answer. Without an `interval`, a first run
//! on another fork counts the steps of those ticks and the interval aims at [`TARGET_SAMPLES`].

use pocket_contract::codes::{Range, out_of_range};
use pocket_contract::{Pointer, Problem};
use pocket_script::{ScriptProfile, SystemProfile};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Game;

/// The ticks a profile runs when `ticks` is not given.
pub const DEFAULT_PROFILE_TICKS: u64 = 60;
/// The most ticks one profile runs.
pub const MAX_PROFILE_TICKS: u64 = 3600;
/// The samples a profile aims at when `interval` is not given: the interval is the run's steps
/// over this, at least 1, counted by a first run of the same ticks on another fork.
pub const TARGET_SAMPLES: u64 = 20_000;
/// The rows of each list when `top` is not given.
pub const DEFAULT_PROFILE_TOP: u32 = 10;

/// `scripts.profile`'s parameters.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScriptsProfileParams {
    /// Ticks to run on the fork (default 60, at most 3600).
    #[serde(default)]
    pub ticks: Option<u64>,
    /// Mean steps between two samples; 1 samples every step, exactly. By default the run's steps
    /// over 20,000, at least 1, from a first run of the same ticks on another fork.
    #[serde(default)]
    pub interval: Option<u32>,
    /// Rows per list: functions, lines and stacks of each system (default 10).
    #[serde(default)]
    pub top: Option<u32>,
    /// Also answer every stack as folded text (`system;frame;...;frame steps` per line), which
    /// speedscope and other flame graph tools read.
    #[serde(default)]
    pub folded: bool,
}

fn check(p: &ScriptsProfileParams) -> Result<(u64, Option<u32>, usize), Problem> {
    let ticks = p.ticks.unwrap_or(DEFAULT_PROFILE_TICKS);
    if !(1..=MAX_PROFILE_TICKS).contains(&ticks) {
        return Err(out_of_range(
            &Pointer::root().key("ticks"),
            &json!(ticks),
            &Range::inclusive(1, MAX_PROFILE_TICKS),
            None,
        ));
    }
    let max = pocket_script::profile::MAX_INTERVAL;
    if let Some(interval) = p.interval
        && !(1..=max).contains(&interval)
    {
        return Err(out_of_range(
            &Pointer::root().key("interval"),
            &json!(interval),
            &Range::inclusive(1, u64::from(max)),
            None,
        ));
    }
    let top = p.top.unwrap_or(DEFAULT_PROFILE_TOP);
    Ok((
        ticks,
        p.interval,
        usize::try_from(top).unwrap_or(usize::MAX),
    ))
}

/// The steps the scripts take over the next `ticks` ticks of `game`, run on a fork (to a fault, if
/// one stops it).
fn steps_ahead(game: &Game, ticks: u64) -> Result<u64, Problem> {
    let mut fork = game.fork()?;
    let mut steps = 0;
    for _ in 0..ticks {
        if fork.step().is_err() {
            break;
        }
        steps += pocket_script::scripts::last_steps(fork.world())
            .iter()
            .map(|(_, s)| s)
            .sum::<u64>();
    }
    Ok(steps)
}

/// Runs `scripts.profile` on a fork of `game`.
pub fn profile(game: &Game, p: &ScriptsProfileParams) -> Result<Value, Problem> {
    let (ticks, interval, top) = check(p)?;
    if let Some(poison) = game.sim().poisoned() {
        return Err(pocket_sim::sim::world_poisoned(poison.tick));
    }
    let interval = match interval {
        Some(i) => i,
        None => {
            let steps = steps_ahead(game, ticks)?;
            let i = steps.div_ceil(TARGET_SAMPLES).max(1);
            u32::try_from(i)
                .unwrap_or(u32::MAX)
                .min(pocket_script::profile::MAX_INTERVAL)
        }
    };
    let mut fork = game.fork()?;
    pocket_script::scripts::profile_start(fork.world(), interval).map_err(|e| e.to_problem())?;
    let from = fork.tick().0;
    let mut stopped = None;
    for _ in 0..ticks {
        if let Err(p) = fork.step() {
            stopped = Some(p);
            break;
        }
    }
    let to = fork.tick().0;
    let profile = pocket_script::scripts::profile_take(fork.world()).unwrap_or(ScriptProfile {
        interval,
        steps: 0,
        samples: 0,
        systems: Vec::new(),
    });
    let mut out = to_json(&profile, from, to, top);
    if let Some(problem) = stopped {
        out["stopped_by"] = json!({"reason": "fault", "problem": problem});
    }
    if p.folded {
        out["folded"] = json!(profile.folded());
    }
    Ok(out)
}

/// The answer: the run, and per system (the most steps first) its exact steps and its top
/// functions, lines and stacks, their self and total in steps estimated from samples.
fn to_json(p: &ScriptProfile, from: u64, to: u64, top: usize) -> Value {
    let mut systems: Vec<&SystemProfile> = p.systems.iter().collect();
    systems.sort_by(|a, b| b.steps.cmp(&a.steps).then_with(|| a.system.cmp(&b.system)));
    let steps = |samples: u64| samples * u64::from(p.interval);
    let systems: Vec<Value> = systems
        .into_iter()
        .map(|s| {
            let functions: Vec<Value> = s
                .functions
                .iter()
                .take(top)
                .map(|f| {
                    json!({"function": f.function, "file": f.file,
                           "self": steps(f.self_samples), "total": steps(f.total_samples)})
                })
                .collect();
            let lines: Vec<Value> = s
                .lines
                .iter()
                .take(top)
                .map(|l| {
                    json!({"at": format!("{}:{}", l.file, l.line), "function": l.function,
                           "self": steps(l.self_samples), "total": steps(l.total_samples)})
                })
                .collect();
            let stacks: Vec<Value> = s
                .stacks
                .iter()
                .take(top)
                .map(|st| json!({"stack": st.frames.join(";"), "steps": steps(st.samples)}))
                .collect();
            let mut v = json!({
                "system": s.system,
                "calls": s.calls,
                "steps": s.steps,
                "samples": s.samples,
                "functions": functions,
                "lines": lines,
                "stacks": stacks,
            });
            let more = |n: usize| n.saturating_sub(top);
            let left = (
                more(s.functions.len()),
                more(s.lines.len()),
                more(s.stacks.len()),
            );
            if left != (0, 0, 0) {
                v["more"] = json!({"functions": left.0, "lines": left.1, "stacks": left.2});
            }
            v
        })
        .collect();
    json!({
        "from_tick": from,
        "to_tick": to,
        "ticks": to - from,
        "interval": p.interval,
        "steps": p.steps,
        "samples": p.samples,
        "estimated": p.interval > 1,
        "systems": systems,
    })
}
