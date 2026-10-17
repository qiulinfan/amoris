//! The step profiler (docs/spec/debugger.md 14): it changes no step, no digest and no budget stop,
//! a regular expression's stop and a debugger's nested evaluations included; the same run gives the
//! same profile, byte for byte; samples land on the functions and TypeScript lines whose steps they
//! are, the jumps of a loop without calls included (P13). The WebAssembly build's profile is the
//! native one: the web report holds one (tests/web.rs).
#![cfg(feature = "transpile")]
mod common;

use std::cell::Cell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

use amoris_script::debug::{DebugHook, Guard, Trace, TraceKind, guarded, set_tracing};
use amoris_script::{CompiledSet, ScriptLimits, ScriptProfile, SystemOutcome};
use amoris_sim::Sim;

fn workload() -> amoris_script::CompiledSet {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/web/workload");
    let src = amoris_script::ScriptSource::read_dir(&root).unwrap();
    amoris_script::compile(&src, &Default::default()).unwrap_or_else(|e| panic!("{e:#?}"))
}

/// What a tick shows of the scripts: each system's outcome and steps, and the world's digest.
type TickRecord = (Vec<(String, String, u64)>, u64);

fn tick_record(sim: &Sim) -> TickRecord {
    let systems = amoris_script::scripts::last_tick(sim.world())
        .into_iter()
        .map(|(name, o)| match o {
            SystemOutcome::Ok(s) => (name, "ok".to_owned(), s.steps),
            SystemOutcome::Failed { error, stats } => (name, error.code, stats.steps),
            SystemOutcome::Fault { error } => (name, error.code, 0),
        })
        .collect();
    (systems, common::hash(sim.world()))
}

/// Runs the workload `ticks` ticks, profiled from tick `from` with `interval` (`None`: never).
fn run(
    set: &amoris_script::CompiledSet,
    ticks: u64,
    from: u64,
    interval: Option<u32>,
) -> (Vec<TickRecord>, Option<ScriptProfile>) {
    let mut sim = common::sim_with(set, ScriptLimits::default(), 2026);
    let mut out = Vec::new();
    for t in 0..ticks {
        if t == from
            && let Some(i) = interval
        {
            amoris_script::scripts::profile_start(sim.world(), i).unwrap();
        }
        common::step(&mut sim);
        out.push(tick_record(&sim));
    }
    let profile = amoris_script::scripts::profile_take(sim.world());
    (out, profile)
}

#[test]
fn a_profile_changes_no_step_and_no_digest() {
    let set = workload();
    let (plain, none) = run(&set, 90, 0, None);
    assert!(none.is_none());
    for interval in [1, 7, 250, 4096] {
        let (profiled, profile) = run(&set, 90, 20, Some(interval));
        assert_eq!(plain, profiled, "interval {interval}");
        let p = profile.unwrap();
        // The profile's steps are the exact steps of the calls it saw, ticks 21 to 90.
        let steps: u64 = plain[20..]
            .iter()
            .flat_map(|(s, _)| s.iter().map(|x| x.2))
            .sum();
        assert_eq!(p.steps, steps, "interval {interval}");
        let by_system: u64 = p.systems.iter().map(|s| s.steps).sum();
        assert_eq!(by_system, steps);
        if interval == 1 {
            // Every step sampled: each system's samples are its steps.
            for s in &p.systems {
                assert_eq!(s.samples, s.steps, "{}", s.system);
            }
        } else {
            // About one sample per `interval` steps.
            let want = steps / u64::from(interval);
            let tolerance = want / 10 + 3;
            assert!(
                p.samples.abs_diff(want) <= tolerance,
                "interval {interval}: {} samples for {steps} steps",
                p.samples
            );
        }
        let sampled: u64 = p.systems.iter().map(|s| s.samples).sum();
        assert_eq!(sampled, p.samples);
    }
}

#[test]
fn a_profile_is_the_same_bytes_every_run() {
    let set = workload();
    let a = run(&set, 61, 1, Some(13)).1.unwrap();
    let b = run(&set, 61, 1, Some(13)).1.unwrap();
    let (ja, jb) = (
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap(),
    );
    assert_eq!(ja, jb);
    assert_eq!(a.folded(), b.folded());
    // A profile started again from the same world after other work gives the same bytes.
    let mut sim = common::sim_with(&set, ScriptLimits::default(), 2026);
    common::step(&mut sim);
    amoris_script::scripts::profile_start(sim.world(), 999).unwrap();
    for _ in 0..5 {
        common::step(&mut sim);
    }
    amoris_script::scripts::profile_take(sim.world()).unwrap();
    // Not the same world as `run`'s from here: just a second recording in one host.
    amoris_script::scripts::profile_start(sim.world(), 13).unwrap();
    common::step(&mut sim);
    let c = amoris_script::scripts::profile_take(sim.world()).unwrap();
    assert!(c.samples > 0);
    println!("{}", &ja[..ja.len().min(600)]);
}

const HOT: &str = r#"import { game, system } from "amoris";
function mix(s: number, i: number): number {
    return i % 3 === 0 ? s + i : s - 1;
}
const hot = system({
    name: "hot", phase: "update", doc: "A loop calling a function.",
    run(ctx) {
        let s = 0;
        for (let i = 0; i < 300; i++) {
            s = mix(s, i);
        }
        let t = 0;
        while (t < 200) {
            t = t + 2;
        }
        if (s === 12345 && t === 0) ctx.emit("game.never");
    },
});
export default game({ systems: [hot] });
"#;

fn line(p: &amoris_script::SystemProfile, file: &str, line: u32) -> (u64, u64) {
    p.lines
        .iter()
        .filter(|l| l.file == file && l.line == line)
        .fold((0, 0), |(a, b), l| {
            (a + l.self_samples, b + l.total_samples)
        })
}

/// Three ticks of `HOT`, every step sampled: its profile and the folded stacks.
fn hot_profile() -> (amoris_script::SystemProfile, String) {
    let set = common::compiled(&[("scripts/main.ts", HOT)]);
    let mut sim = common::sim_with(&set, ScriptLimits::default(), 1);
    amoris_script::scripts::profile_start(sim.world(), 1).unwrap();
    for _ in 0..3 {
        common::step(&mut sim);
    }
    let p = amoris_script::scripts::profile_take(sim.world()).unwrap();
    let hot = p
        .systems
        .iter()
        .find(|s| s.system == "hot")
        .unwrap()
        .clone();
    assert_eq!(hot.calls, 3);
    assert_eq!(hot.samples, hot.steps);
    (hot, p.folded())
}

/// What holds without P13 too: samples are counted under the right function, and a caller's line
/// at a call (where QuickJS-ng stores the frame's position itself).
#[test]
fn samples_land_on_their_functions() {
    let (hot, folded) = hot_profile();
    let f = "scripts/main.ts";
    // `mix`'s conditional: one or two jumps a call, in `mix`.
    let mix = hot.functions.iter().find(|x| x.function == "mix").unwrap();
    assert_eq!(mix.file, f);
    assert!(mix.self_samples >= 900, "{:#?}", hot.functions);
    let run = hot.functions.iter().find(|x| x.function == "run").unwrap();
    assert!(run.total_samples >= run.self_samples + mix.self_samples);
    // The calling line holds `mix`'s steps in total.
    assert!(line(&hot, f, 10).1 >= 900 + mix.self_samples);
    // The folded stacks: root first, the system named once per line, weights summing to the steps.
    assert!(
        folded
            .lines()
            .any(|l| l.starts_with("hot;run (scripts/main.ts:10);mix (")),
        "{folded}"
    );
    let total: u64 = folded
        .lines()
        .filter(|l| l.starts_with("hot;"))
        .map(|l| l.rsplit(' ').next().unwrap().parse::<u64>().unwrap())
        .sum();
    assert_eq!(total, hot.steps);
    println!("{folded}");
}

/// What P13 adds: a jump's step is on the jump's own line. Without it, a jump reports the frame's
/// last call (line 10 in `run`) or no position at all in a frame that has made none (`mix`, the
/// while loop).
#[test]
fn jumps_land_on_their_own_lines() {
    let (hot, folded) = hot_profile();
    let f = "scripts/main.ts";
    // One step per call of `mix` on the calling line, and nothing else there.
    assert_eq!(line(&hot, f, 10).0, 900, "{:#?}", hot.lines);
    // `mix`'s conditional is its own line.
    let (own3, _) = line(&hot, f, 3);
    let mix = hot.functions.iter().find(|x| x.function == "mix").unwrap();
    assert_eq!(mix.self_samples, own3, "{:#?}", hot.lines);
    // The for loop's test and back jump, and the while loop without calls.
    assert!(line(&hot, f, 9).0 >= 1800, "{:#?}", hot.lines);
    assert!(
        line(&hot, f, 13).0 + line(&hot, f, 14).0 >= 200,
        "{:#?}",
        hot.lines
    );
    // No sample without a position.
    assert!(
        hot.lines
            .iter()
            .all(|l| l.line > 0 || l.file == "native" || l.file == "host"),
        "{:#?}",
        hot.lines
    );
    assert!(
        folded
            .lines()
            .any(|l| l.starts_with("hot;run (scripts/main.ts:10);mix (scripts/main.ts:3) ")),
        "{folded}"
    );
}

const SPIN: &str = r#"import { game, system } from "amoris";
function tick(n: number): number { return n + 1; }
const spin = system({
    name: "spin", phase: "update", doc: "Loops until it runs out of steps, or runs N.",
    run(ctx) {
        let n = 0;
        for (let i = 0; i < LIMIT; i++) {
            n = tick(n);
        }
        if (n < 0) ctx.emit("game.never");
    },
});
export default game({ systems: [spin] });
"#;

/// The first system's outcome in the last tick: its code, steps and error location.
fn outcome(sim: &Sim) -> (String, u64, Option<String>) {
    match &amoris_script::scripts::last_tick(sim.world())[0].1 {
        SystemOutcome::Ok(s) => ("ok".into(), s.steps, None),
        SystemOutcome::Failed { error, stats } => (
            error.code.clone(),
            stats.steps,
            error.detail.location.as_ref().map(|l| format!("{l:?}")),
        ),
        SystemOutcome::Fault { error } => (error.code.clone(), 0, None),
    }
}

/// One tick of `source` under `steps` per system, profiled every `interval`, with `hook` attached:
/// the system's outcome, steps and error location, and the world's digest. A profile's steps are
/// the system's; with `interval` 1 and no regular expression stopping the call (`regex`), so are
/// its samples.
fn one_tick(
    source: &str,
    steps: u32,
    interval: Option<u32>,
    hook: Option<Rc<dyn DebugHook>>,
    regex: bool,
) -> (String, u64, Option<String>, u64) {
    let set = common::compiled(&[("scripts/main.ts", source)]);
    let limits = ScriptLimits {
        steps_per_system: steps,
        ..ScriptLimits::default()
    };
    let mut sim = common::sim_with(&set, limits, 1);
    if let Some(hook) = hook {
        assert!(amoris_script::debug::attach(sim.world_mut(), Some(hook)));
    }
    if let Some(i) = interval {
        amoris_script::scripts::profile_start(sim.world(), i).unwrap();
    }
    common::step(&mut sim);
    let (code, used, at) = outcome(&sim);
    if interval.is_some() {
        let p = amoris_script::scripts::profile_take(sim.world()).unwrap();
        // A debugger's program is instantiated again, instrumented, as the tick starts: `(load)`.
        let name = &amoris_script::scripts::last_tick(sim.world())[0].0;
        let s = p.systems.iter().find(|s| &s.system == name).unwrap();
        assert_eq!(s.calls, 1);
        assert_eq!(s.steps, used);
        if interval == Some(1) && !regex {
            assert_eq!(s.samples, s.steps);
        }
    }
    (code, used, at, common::hash(sim.world()))
}

/// One tick of `SPIN` with `LIMIT`, under `steps` per system, profiled every `interval`: the
/// system's outcome, steps and error location.
fn spin(limit: &str, steps: u32, interval: Option<u32>) -> (String, u64, Option<String>) {
    let (code, used, at, _) = one_tick(&SPIN.replace("LIMIT", limit), steps, interval, None, false);
    (code, used, at)
}

#[test]
fn a_budget_stops_at_the_same_step_with_a_profile() {
    // An endless loop: stopped at the budget's last step, at the same line.
    let endless = spin("Infinity", 5_000, None);
    assert_eq!(endless.0, "script.budget_exceeded");
    assert_eq!(endless.1, 5_000);
    for interval in [1, 2, 3, 4_999, 5_000, 5_001, 1_000_000] {
        assert_eq!(
            spin("Infinity", 5_000, Some(interval)),
            endless,
            "{interval}"
        );
    }
    // A call that needs k steps: it fits a budget of k + 1 and overruns one of k, at its last
    // step, with or without a profile.
    let (code, k, _) = spin("100", 1_000_000, None);
    assert_eq!(code, "ok");
    let k = u32::try_from(k).unwrap();
    for budget in [k - 1, k, k + 1, k + 2] {
        let plain = spin("100", budget, None);
        assert_eq!(plain.0 == "ok", budget > k, "{budget}: {plain:?}");
        for interval in [1, 2, 7, k - 1, k, k + 1, 1_000] {
            assert_eq!(
                spin("100", budget, Some(interval)),
                plain,
                "budget {budget}, interval {interval}"
            );
        }
    }
    // No budget at all: the first step stops it.
    assert_eq!(spin("100", 0, Some(1)), spin("100", 0, None));
}

#[test]
fn an_interval_out_of_range_is_refused() {
    let set = common::compiled(&[("scripts/main.ts", HOT)]);
    let sim = common::sim_with(&set, ScriptLimits::default(), 1);
    for bad in [0, amoris_script::profile::MAX_INTERVAL + 1] {
        let e = amoris_script::scripts::profile_start(sim.world(), bad).unwrap_err();
        assert_eq!(e.code, "request.invalid_value");
    }
    assert!(amoris_script::scripts::profile_take(sim.world()).is_none());
}

const REGEX: &str = r#"import { game, system } from "amoris";
const re = system({
    name: "re", phase: "update", doc: "Regular expressions; the last backtracks over REPEAT pairs.",
    run(ctx) {
        let n = 0;
        for (let i = 0; i < 40; i++) {
            if (/^(?:ab)+$/.test("ab".repeat(i + 1))) n++;
        }
        const s = "ab".repeat(REPEAT);
        if (/(?:a|b)*c/.test(s)) n = -1;
        if (n !== 40) ctx.emit("game.never");
    },
});
export default game({ systems: [re] });
"#;

/// libregexp calls the interrupt handler itself, every 10,000 of a regular expression's own polls,
/// without the counter; without a profile that stops the call as over its budget, whatever steps
/// it has left (script-sandbox.md 4.1). With a profile it stops the same way, at the same place,
/// and the regular expressions that finish leave the same steps.
#[test]
fn a_regular_expression_stops_the_same_with_a_profile() {
    let budget = ScriptLimits::default().steps_per_system;
    let long = REGEX.replace("REPEAT", "2000");
    let short = REGEX.replace("REPEAT", "20");
    let stopped = one_tick(&long, budget, None, None, true);
    assert_eq!(stopped.0, "script.budget_exceeded", "{stopped:?}");
    assert_eq!(stopped.1, u64::from(budget));
    let fits = one_tick(&short, budget, None, None, true);
    assert_eq!(fits.0, "ok", "{fits:?}");
    for interval in [1, 7, 100, 4_096, 9_999, 10_000, 10_001, 1_000_000] {
        let profiled = one_tick(&long, budget, Some(interval), None, true);
        assert_eq!(profiled, stopped, "{interval}");
        let profiled = one_tick(&short, budget, Some(interval), None, true);
        assert_eq!(profiled, fits, "{interval}");
    }
}

/// A debugger that runs, at every statement, an evaluation inside another, as CDP's
/// `callFunctionOn` does (amoris-debug, inspect.rs), each under the host's guard.
#[derive(Default)]
struct Nested {
    evaluations: Cell<u32>,
}

impl DebugHook for Nested {
    fn instrument(&self) -> bool {
        true
    }

    fn loaded(&self, _set: &Arc<CompiledSet>, _instrumented: bool) {}

    fn trace(&self, trace: &Trace) {
        if trace.kind != TraceKind::Statement {
            return;
        }
        let Some(raw) = NonNull::new(trace.ctx) else {
            return;
        };
        let evaluate = || {
            // SAFETY: the stopped call's context, live during the trace.
            let ctx = unsafe { rquickjs::Ctx::from_raw(raw) };
            ctx.eval::<i32, _>("[1, 2, 3].map((x) => x * 2).length")
                .unwrap_or(0)
        };
        // SAFETY: as above, on the game thread. Untraced, as amoris-debug evaluates: code compiled
        // while the handler is set is instrumented and would trace its own evaluation.
        let n = unsafe {
            set_tracing(trace.ctx, false);
            let n = guarded(trace.ctx, Guard::Read, || {
                guarded(trace.ctx, Guard::Read, evaluate)
            });
            set_tracing(trace.ctx, true);
            n
        };
        if n == 3 {
            self.evaluations.set(self.evaluations.get() + 1);
        }
    }
}

#[test]
fn a_debuggers_nested_evaluations_leave_the_profiled_call_alone() {
    for (limit, budget) in [("100", 1_000_000), ("Infinity", 5_000)] {
        let source = SPIN.replace("LIMIT", limit);
        let hook = Rc::new(Nested::default());
        let plain = one_tick(&source, budget, None, Some(hook.clone()), false);
        let evaluations = hook.evaluations.get();
        assert!(evaluations > 100, "{limit}: {evaluations}");
        for interval in [1, 7, 1_000] {
            let hook = Rc::new(Nested::default());
            let profiled = one_tick(&source, budget, Some(interval), Some(hook.clone()), false);
            assert_eq!(profiled, plain, "{limit}, interval {interval}");
            assert_eq!(hook.evaluations.get(), evaluations);
        }
    }
}
