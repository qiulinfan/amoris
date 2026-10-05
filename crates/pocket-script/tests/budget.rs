//! Tests 3 and 4 of script-host.md 12 (steps; budget and transactions) and P1 of
//! script-sandbox.md 4.2: a call's steps depend on the code it runs alone, whatever ran before it;
//! an endless loop, in a system or in a promise job it queued, stops after exactly the configured
//! steps; a failed system leaves the world, the inbox, the RNG and the allocator as they were, and
//! no job behind it (P8).
#![cfg(feature = "transpile")]
mod common;

use pocket_script::{ScriptLimits, SystemOutcome};
use pocket_sim::{EntityAllocator, Sim};

const COMPONENTS: &str = r#"import { component, field } from "pocket";
export const Mover = component("Mover", {
    version: 1, doc: "Something that moves.",
    fields: { x: field.f64(0, "Position."), hits: field.u32(0, "Times hit.") },
});
"#;

/// A fixed workload whose steps are pinned, and a system that runs before it doing more or less
/// work by tick, so the counter's history differs from call to call.
const WORK: &str = r#"import { game, system } from "pocket";
import { Mover } from "./components";
const setup = system({
    name: "setup", phase: "update", doc: "Spawns movers.", when: "start",
    run(ctx) {
        for (let i = 0; i < 50; i++) ctx.world.spawn({ Mover: { x: i } });
    },
});
const noise = system({
    name: "noise", phase: "update", doc: "Work that varies by tick.",
    run(ctx) {
        let s = 0;
        for (let i = 0; i < (ctx.tick % 7) * 1000; i++) s += i;
        if (s < 0) ctx.emit("game.never");
    },
});
const fixed = system({
    name: "fixed", phase: "update", doc: "The same work every tick.",
    queries: { movers: { with: ["Mover"] } },
    run(ctx, { movers }) {
        const x = movers.cols.Mover.x;
        for (let r = 0; r < movers.len; r++) x[r] = x[r] + Math.sin(r) * 0.001;
    },
});
export default game({ components: [Mover], systems: [setup, noise, fixed] });
"#;

fn fixed_steps(sim: &Sim) -> u64 {
    pocket_script::scripts::last_steps(sim.world())
        .into_iter()
        .find(|(n, _)| n == "fixed")
        .map(|(_, s)| s)
        .unwrap()
}

/// The fixed system's steps on ticks 2 to 9: pinned, so a change to the prelude, the host's
/// calls or QuickJS-ng shows here (and in the web report, tests/web.rs).
const FIXED_STEPS: u64 = 215;

#[test]
fn steps_depend_on_the_call_alone() {
    let set = common::compiled(&[
        ("scripts/components.ts", COMPONENTS),
        ("scripts/main.ts", WORK),
    ]);
    let mut first = Vec::new();
    for run in 0..2 {
        let mut sim = common::sim_with(&set, ScriptLimits::default(), 3);
        common::step(&mut sim);
        let mut steps = Vec::new();
        for t in 2..10 {
            common::step(&mut sim);
            steps.push(fixed_steps(&sim));
            if t == 5 {
                // A reload changes nothing a call counts.
                pocket_script::swap(sim.world_mut(), &set, true).unwrap();
            }
        }
        assert!(
            steps.iter().all(|s| *s == FIXED_STEPS),
            "run {run}: {steps:?}"
        );
        if run == 0 {
            first = steps;
        } else {
            assert_eq!(first, steps);
        }
    }
}

const SPIN: &str = r#"import { game, system } from "pocket";
const spin = system({
    name: "spin", phase: "update", doc: "Never ends.",
    run(ctx) {
        for (let i = 0; ; i++) {
            if (i % 1000 === 0) console.log(i);
        }
    },
});
export default game({ systems: [spin] });
"#;

const JOB: &str = r#"import { game, system } from "pocket";
const queue = system({
    name: "queue", phase: "update", doc: "Leaves an endless job.",
    run(ctx) {
        (async () => {})().then(() => {
            for (let i = 0; ; i++) {
                if (i % 1000 === 0) console.log(i);
            }
        });
    },
});
export default game({ systems: [queue] });
"#;

fn spin_once(files: &[(&str, &str)], limits: ScriptLimits, warm: u32) -> (u64, String) {
    let set = common::compiled_lint_off(files);
    let mut sim = common::sim_with(&set, limits, 1);
    // `warm` failed calls run first, so the measured call follows a different history.
    let mut last = None;
    for _ in 0..=warm {
        let r = common::step(&mut sim);
        assert_eq!(r.errors.len(), 1, "{:#?}", r.errors);
        let outcomes = pocket_script::scripts::last_tick(sim.world());
        let SystemOutcome::Failed { error, stats } = &outcomes[0].1 else {
            panic!("{outcomes:#?}")
        };
        assert_eq!(error.code, "script.budget_exceeded", "{error:#?}");
        let log = sim
            .world()
            .get_non_send::<pocket_script::Scripts>()
            .unwrap()
            .host
            .take_log();
        last = Some((
            stats.steps,
            log.last().map(|l| l.text.clone()).unwrap_or_default(),
        ));
    }
    last.unwrap()
}

#[test]
fn an_endless_loop_stops_at_the_same_step_every_run() {
    let limits = ScriptLimits {
        steps_per_system: 50_000,
        log_lines_per_tick: 1000,
        ..ScriptLimits::default()
    };
    let a = spin_once(&[("scripts/main.ts", SPIN)], limits, 0);
    let b = spin_once(&[("scripts/main.ts", SPIN)], limits, 3);
    assert_eq!(a.0, 50_000);
    assert_eq!(a, b);
    // The tick's budget caps the system's.
    let tight = ScriptLimits {
        steps_per_tick: 30_000,
        ..limits
    };
    let c = spin_once(&[("scripts/main.ts", SPIN)], tight, 0);
    assert_eq!(c.0, 30_000);
    // An endless loop in a promise job fails the call that queued it, under the same budget.
    let d = spin_once(&[("scripts/main.ts", JOB)], limits, 2);
    assert_eq!(d.0, 50_000);
    println!("stopped after logging {} (system) and {} (job)", a.1, d.1);
}

const TRANSACTION: &str = r#"import { game, system } from "pocket";
import { Mover } from "./components";
const setup = system({
    name: "setup", phase: "update", doc: "Spawns movers.", when: "start",
    run(ctx) {
        for (let i = 0; i < 3; i++) ctx.world.spawn({ Mover: { x: i } });
    },
});
const busy = system({
    name: "busy", phase: "update", doc: "Does everything, then maybe throws.", when: { every: 2 },
    queries: { movers: { with: ["Mover"] } },
    run(ctx, { movers }) {
        movers.cols.Mover.x[0] = 100;
        ctx.world.set(movers.id(1), "Mover", { hits: 5 });
        const e = ctx.world.spawn({ Mover: { x: 9 } });
        ctx.emit("game.busy", { spawned: e });
        const r = ctx.rng.next() + Math.random();
        ctx.world.set(movers.id(0), "Mover", { x: 200 });
        if (ctx.tick % 4 === 2) throw new Error("after all that, " + r);
    },
});
export default game({ components: [Mover], systems: [setup, busy] });
"#;

#[test]
fn a_failed_system_changes_nothing_and_a_good_one_applies_in_order() {
    let set = common::compiled(&[
        ("scripts/components.ts", COMPONENTS),
        ("scripts/main.ts", TRANSACTION),
    ]);
    let mut sim = common::sim_with(&set, ScriptLimits::default(), 5);
    common::step(&mut sim); // tick 1
    let movers = |sim: &Sim| -> Vec<serde_json::Value> {
        (1..=3)
            .map(|i| {
                let e = pocket_sim::EntityId::from_f64(f64::from(i)).unwrap();
                common::component_json(sim.world(), e, "Mover").unwrap()
            })
            .collect()
    };
    let before = movers(&sim);
    let next = sim.world().resource::<EntityAllocator>().next();
    let r = common::step(&mut sim); // tick 2: busy throws
    assert_eq!(r.errors.len(), 1);
    // Only the clock moved and the failure's event was appended.
    assert_eq!(sim.world().resource::<EntityAllocator>().next(), next);
    let kinds: Vec<&str> = r.events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["script.failed"]);
    // No column write, set or spawn reached the world.
    assert_eq!(movers(&sim), before);
    common::step(&mut sim); // tick 3: nothing runs
    let r = common::step(&mut sim); // tick 4: busy succeeds
    assert!(r.errors.is_empty(), "{:#?}", r.errors);
    // Columns first, then commands in call order: the set of x to 200 wins over the column's 100.
    let world = sim.world();
    let comp = pocket_script::scripts::last_tick(world);
    assert!(matches!(comp[0].1, SystemOutcome::Ok(_)));
    let busy: Vec<&str> = r.events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(busy, ["game.busy"]);
    assert_eq!(
        r.events[0].data.get("spawned").unwrap(),
        &pocket_sim::PlainData::Number(next as f64)
    );
    assert_eq!(world.resource::<EntityAllocator>().next(), next + 1);
    let now = movers(&sim);
    assert_eq!(now[0]["x"], serde_json::json!(200.0));
    assert_eq!(now[1]["hits"], serde_json::json!(5));
}

const LOOT: &str = r#"import { game, system } from "pocket";
const greedy = system({
    name: "greedy", phase: "update", doc: "Draws from a shared stream, then fails.",
    run(ctx) {
        ctx.rngTimeless("loot").next();
        ctx.rngTimeless("loot").next();
        throw new Error("no loot for you");
    },
});
const fair = system({
    name: "fair", phase: "update", doc: "Draws from the shared stream.",
    run(ctx) {
        ctx.emit("game.loot", { v: ctx.rngTimeless("loot").next() });
    },
});
export default game({ systems: [SYSTEMS] });
"#;

#[test]
fn rng_draws_of_a_failed_call_are_undone() {
    // A timeless stream is shared by the systems of a tick: the failed system's draws must not
    // move the next system's.
    let with = common::compiled(&[("scripts/main.ts", &LOOT.replace("SYSTEMS", "greedy, fair"))]);
    let without = common::compiled(&[("scripts/main.ts", &LOOT.replace("SYSTEMS", "fair"))]);
    let mut a = common::sim_with(&with, ScriptLimits::default(), 9);
    let mut b = common::sim_with(&without, ScriptLimits::default(), 9);
    for t in 1..=3 {
        let ra = common::step(&mut a);
        let rb = common::step(&mut b);
        assert_eq!(ra.errors.len(), 1, "tick {t}");
        let loot = |r: &pocket_sim::StepReport| {
            r.events
                .iter()
                .find(|e| e.kind.as_str() == "game.loot")
                .unwrap()
                .data
                .clone()
        };
        assert_eq!(loot(&ra), loot(&rb), "tick {t}");
    }
}

/// A system that queues a job and then fails, before or after a system with fixed work; `QUEUE`
/// and `FAIL` are filled in. Async code needs the lint off, which is how a job could be queued at
/// all once Array.fromAsync and AsyncDisposableStack are gone.
const LEAK: &str = r#"import { game, system, component, field } from "pocket";
export const Tally = component("Tally", {
    version: 1, doc: "A count.", fields: { n: field.u32(0, "Count.") },
});
const setup = system({
    name: "setup", phase: "update", doc: "Spawns the tally.", when: "start",
    run(ctx) { ctx.world.spawn({ Tally: { n: 0 } }); },
});
const leaky = system({
    name: "leaky", phase: "update", doc: "Queues a job, then fails.",
    queries: { t: { with: ["Tally"] } },
    run(ctx, { t }) {
        if (t.len === 0) return;
        const e = t.id(0);
        QUEUE
        FAIL
    },
});
const counter = system({
    name: "counter", phase: "update", doc: "Fixed work.",
    run(ctx) {
        let s = 0;
        for (let i = 0; i < 5; i++) s += i;
        if (s < 0) ctx.emit("game.never");
    },
});
export default game({ components: [Tally], systems: [ORDER] });
"#;

/// The counter's steps and the world's digest at each of 6 ticks, with the world given a new host
/// after tick `rehost_at`.
fn leak_run(order: &str, queue: bool, fail: &str, rehost_at: Option<u64>) -> Vec<(u64, u64)> {
    let job = "(async () => {})().then(() => ctx.world.set(e, \"Tally\", { n: 999 }));";
    let src = LEAK
        .replace("ORDER", order)
        .replace("QUEUE", if queue { job } else { "void e;" })
        .replace("FAIL", fail);
    let set = common::compiled_lint_off(&[("scripts/main.ts", &src)]);
    let limits = ScriptLimits {
        steps_per_system: 20_000,
        ..ScriptLimits::default()
    };
    let mut sim = common::sim_with(&set, limits, 3);
    let mut out = Vec::new();
    for t in 1..=6 {
        common::step(&mut sim);
        let steps = pocket_script::scripts::last_steps(sim.world())
            .into_iter()
            .find(|(n, _)| n == "counter")
            .map_or(0, |(_, s)| s);
        out.push((steps, common::hash(sim.world())));
        if rehost_at == Some(t) {
            pocket_script::scripts::rehost(sim.world_mut(), limits).unwrap();
        }
    }
    out
}

#[test]
fn a_failed_call_leaves_no_job_behind() {
    for fail in ["throw new Error(\"after queueing\");", "for (;;) {}"] {
        // The job would run in the next system's call, or across the tick boundary in the first
        // system of the next tick; a new host would drop it, so a fork would diverge.
        for order in ["setup, leaky, counter", "setup, counter, leaky"] {
            let reference = leak_run(order, false, fail, None);
            assert_eq!(
                leak_run(order, true, fail, None),
                reference,
                "{order}; {fail}"
            );
            assert_eq!(
                leak_run(order, true, fail, Some(3)),
                reference,
                "{order}; {fail}; rehosted"
            );
        }
    }
}
