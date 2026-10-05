//! The tick (docs/spec/simulation.md 12): time, run conditions, phases and hooks, the schedule
//! golden, events and their delivery, transactions, reports, decisions and boundary writes.

mod common;

use bevy_ecs::prelude::*;
use common::{Seen, id, seen, sim};
use pocket_sim::event::emit;
use pocket_sim::sim::{begin_invocation, commit_invocation, rollback_invocation, system_failed};
use pocket_sim::{
    DecisionRequest, Emit, EventInbox, EventKind, NewEvent, NoHooks, PlainData, RngTable,
    RunCondition, SimClock, StepHooks, SystemCtx, SystemKey, Tick, TickOutput, TickPhase, entity,
};

fn kind(k: &str) -> EventKind {
    EventKind::new(k).unwrap()
}

#[test]
fn time_is_the_tick_over_the_rate() {
    let mut s = sim(1);
    s.add_system(
        "test.clock",
        TickPhase::Update,
        RunCondition::Always,
        |clock: Res<SimClock>, mut seen: ResMut<Seen>| {
            seen.0.push(format!("{} {}", clock.tick.0, clock.time()));
        },
    )
    .unwrap();
    for _ in 0..3 {
        s.step(&mut NoHooks).unwrap();
    }
    // During tick n the clock reads n, and time() is the end of the step.
    assert_eq!(
        seen(&s),
        ["1 0.016666666666666666", "2 0.03333333333333333", "3 0.05"]
    );
    assert_eq!(s.clock().tick, Tick(3));
    assert_eq!(s.clock().dt(), 1.0 / 60.0);
}

#[test]
fn run_conditions_fire_on_the_ticks_they_name() {
    let mut s = sim(1);
    s.add_system(
        "test.every",
        TickPhase::Update,
        RunCondition::every(3, 1).unwrap(),
        |c: Res<SimClock>, mut seen: ResMut<Seen>| {
            seen.0.push(format!("every {}", c.tick.0));
        },
    )
    .unwrap();
    s.add_system(
        "test.start",
        TickPhase::Update,
        RunCondition::Start,
        |c: Res<SimClock>, mut seen: ResMut<Seen>| {
            seen.0.push(format!("start {}", c.tick.0));
        },
    )
    .unwrap();
    for _ in 0..7 {
        s.step(&mut NoHooks).unwrap();
    }
    assert_eq!(seen(&s), ["every 1", "start 1", "every 4", "every 7"]);
}

#[derive(Default)]
struct Recorder(Vec<String>);

impl StepHooks for Recorder {
    fn phase(&mut self, phase: TickPhase, begin: bool) {
        self.0
            .push(format!("{}{}", if begin { "+" } else { "-" }, phase.name()));
    }
    fn system(&mut self, key: &SystemKey, begin: bool) {
        self.0
            .push(format!("{}{key}", if begin { "+" } else { "-" }));
    }
}

#[test]
fn hooks_wrap_every_phase_and_system_in_order() {
    let mut s = sim(1);
    s.add_system(
        "physics.step",
        TickPhase::Physics,
        RunCondition::Always,
        || {},
    )
    .unwrap();
    s.add_exclusive(
        "script.update",
        TickPhase::Update,
        RunCondition::Always,
        |_: &mut World, ctx: &mut SystemCtx<'_>| {
            // script.update times each script system through the same hooks.
            let key = SystemKey::new("script:crates").unwrap();
            ctx.hooks().system(&key, true);
            ctx.hooks().system(&key, false);
        },
    )
    .unwrap();
    let mut r = Recorder::default();
    s.step(&mut r).unwrap();
    assert_eq!(
        r.0.join(" "),
        "+Begin +sim.begin -sim.begin -Begin +Control -Control +Update +script.update \
         +script:crates -script:crates -script.update -Update +Forces -Forces +Physics \
         +physics.step -physics.step -Physics +Finish +sim.finish -sim.finish -Finish"
    );
}

#[test]
fn schedule_golden() {
    let mut s = sim(1);
    // Registered out of order, as crates would: the table decides the order.
    for (key, phase) in [
        ("interface.turns", TickPhase::Finish),
        ("physics.contacts", TickPhase::Physics),
        ("physics.hull", TickPhase::Forces),
        ("interface.perception", TickPhase::Finish),
        ("physics.step", TickPhase::Physics),
        ("physics.sail", TickPhase::Forces),
        ("script.update", TickPhase::Update),
        ("physics.buoyancy", TickPhase::Forces),
        ("interface.intents", TickPhase::Control),
        ("physics.wind", TickPhase::Forces),
    ] {
        s.add_system(key, phase, RunCondition::Always, || {})
            .unwrap();
    }
    s.add_system(
        "test.late",
        TickPhase::Finish,
        RunCondition::every(10, 0).unwrap(),
        || {},
    )
    .unwrap();
    s.add_system("test.early", TickPhase::Begin, RunCondition::Start, || {})
        .unwrap();
    let golden = include_str!("golden/schedule.txt");
    assert_eq!(s.schedule_listing(), golden);
    // A duplicate, a listed key in another phase and a bad key are refused.
    let e = s
        .add_system(
            "physics.wind",
            TickPhase::Forces,
            RunCondition::Always,
            || {},
        )
        .unwrap_err();
    assert_eq!(e.code, "sim.system_duplicate");
    let e = sim(1)
        .add_system(
            "physics.step",
            TickPhase::Update,
            RunCondition::Always,
            || {},
        )
        .unwrap_err();
    assert_eq!(e.code, "sim.system_phase");
    let e = sim(1)
        .add_system("Bad Key", TickPhase::Update, RunCondition::Always, || {})
        .unwrap_err();
    assert_eq!(e.code, "sim.system_key_invalid");
}

/// Emits `test.ping` with the tick as data from the first system; a later system and a system of
/// the next phase record which pings they read.
fn ping_world() -> pocket_sim::Sim {
    let mut s = sim(1);
    s.add_system(
        "interface.intents",
        TickPhase::Control,
        RunCondition::Always,
        |inbox: Res<EventInbox>, c: Res<SimClock>, mut seen: ResMut<Seen>| {
            let got: Vec<String> = inbox
                .events()
                .iter()
                .map(|e| format!("{}:{}@{}", e.kind.as_str(), e.seq.0, e.tick.0))
                .collect();
            seen.0.push(format!(
                "tick {} control reads [{}]",
                c.tick.0,
                got.join(" ")
            ));
        },
    )
    .unwrap();
    s.add_system(
        "script.update",
        TickPhase::Update,
        RunCondition::Always,
        |mut emit: Emit, c: Res<SimClock>| {
            let data = PlainData::Number(c.tick.to_f64());
            emit.emit(NewEvent::new(kind("test.ping")).data(data));
        },
    )
    .unwrap();
    s.add_system(
        "physics.step",
        TickPhase::Physics,
        RunCondition::Always,
        |inbox: Res<EventInbox>, mut seen: ResMut<Seen>| {
            let got: Vec<u64> = inbox.of_kind("test.ping").map(|e| e.seq.0).collect();
            seen.0.push(format!("physics reads {got:?}"));
        },
    )
    .unwrap();
    s
}

#[test]
fn events_reach_every_system_of_the_next_tick_once() {
    let mut s = ping_world();
    let r1 = s.step(&mut NoHooks).unwrap();
    let seq = s
        .boundary()
        .emit(NewEvent::new(kind("test.boundary")).subject(id(1)));
    let r2 = s.step(&mut NoHooks).unwrap();
    s.step(&mut NoHooks).unwrap();
    assert_eq!(
        seen(&s),
        [
            "tick 1 control reads []",
            "physics reads []",
            // Tick 1's ping (seq 1) and the boundary write's event (seq 2, of tick 1).
            "tick 2 control reads [test.ping:1@1 test.boundary:2@1]",
            "physics reads [1]",
            // Tick 2's ping only: tick 1's events are gone.
            "tick 3 control reads [test.ping:3@2]",
            "physics reads [3]",
        ]
    );
    assert_eq!(seq.0, 2);
    // The report carries the tick's event record.
    assert_eq!(r1.events.len(), 1);
    assert_eq!(r1.events[0].data, PlainData::Number(1.0));
    assert_eq!(r2.events.iter().map(|e| e.seq.0).collect::<Vec<_>>(), [3]);
    let inbox = s.world().resource::<EventInbox>();
    assert_eq!(inbox.tick_events().len(), 1);
    assert!(inbox.boundary_events().is_empty());
}

/// A system that draws, spawns and emits, then fails (rolls back) on the ticks `fail` names; and a
/// system after it that draws and spawns, recording what it got.
fn transaction_world(failing: bool) -> pocket_sim::Sim {
    let mut s = sim(9);
    s.add_exclusive(
        "script.update",
        TickPhase::Update,
        RunCondition::Always,
        move |world: &mut World, ctx: &mut SystemCtx<'_>| {
            let key = SystemKey::new("script:loot").unwrap();
            let inv = begin_invocation(world, &key);
            let roll = world
                .resource_mut::<RngTable>()
                .system(key.as_str())
                .unwrap()
                .next_u32();
            let spawned = entity::spawn(world, common::Value(f64::from(roll))).unwrap();
            emit(world, NewEvent::new(kind("test.loot")).subject(spawned));
            world.resource_mut::<TickOutput>().decide(DecisionRequest {
                observer: spawned,
                reason: "test".into(),
                event: None,
            });
            if failing {
                rollback_invocation(world, inv);
                let cause =
                    pocket_contract::Problem::new("script.thrown", "thrown.", Default::default());
                let failed = system_failed(ctx.tick(), ctx.phase(), &key, None, &cause);
                world.resource_mut::<TickOutput>().report(failed);
            } else {
                commit_invocation(world, inv);
            }
        },
    )
    .unwrap();
    s.add_exclusive(
        "physics.wind",
        TickPhase::Forces,
        RunCondition::Always,
        |world: &mut World, _: &mut SystemCtx<'_>| {
            let roll = world
                .resource_mut::<RngTable>()
                .system("physics.wind")
                .unwrap()
                .next_u32();
            let other = world
                .resource_mut::<RngTable>()
                .system("script:loot")
                .unwrap()
                .next_u32();
            let e = entity::spawn(world, ()).unwrap();
            let line = format!("wind {roll} loot-stream {other} spawned {}", e.get());
            world.resource_mut::<Seen>().0.push(line);
        },
    )
    .unwrap();
    s
}

#[test]
fn a_failed_invocation_leaves_nothing_behind() {
    let mut failing = transaction_world(true);
    let report = failing.step(&mut NoHooks).unwrap();
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].code, "sim.system_failed");
    assert_eq!(
        report.errors[0].detail["system"],
        serde_json::json!("script:loot")
    );
    assert_eq!(
        report.errors[0].detail["cause"]["code"],
        serde_json::json!("script.thrown")
    );
    assert!(report.events.is_empty() && report.decisions.is_empty());
    // As if it had not run: the next system's draws, the stream it would have advanced and the
    // next spawn's id are those of a world without the failing system.
    let mut without = sim(9);
    without
        .add_exclusive(
            "physics.wind",
            TickPhase::Forces,
            RunCondition::Always,
            |world: &mut World, _: &mut SystemCtx<'_>| {
                let roll = world
                    .resource_mut::<RngTable>()
                    .system("physics.wind")
                    .unwrap()
                    .next_u32();
                let other = world
                    .resource_mut::<RngTable>()
                    .system("script:loot")
                    .unwrap()
                    .next_u32();
                let e = entity::spawn(world, ()).unwrap();
                let line = format!("wind {roll} loot-stream {other} spawned {}", e.get());
                world.resource_mut::<Seen>().0.push(line);
            },
        )
        .unwrap();
    without.step(&mut NoHooks).unwrap();
    assert_eq!(seen(&failing), seen(&without));
    assert_eq!(common::state_hash(&failing), common::state_hash(&without));
    // The committed form keeps everything, in order.
    let mut ok = transaction_world(false);
    let report = ok.step(&mut NoHooks).unwrap();
    assert!(report.errors.is_empty());
    assert_eq!(report.events.len(), 1);
    assert_eq!(report.events[0].subject, Some(id(1)));
    assert_eq!(report.decisions.len(), 1);
    assert!(seen(&ok)[0].ends_with("spawned 2"));
}

#[test]
fn a_refused_boundary_write_returns_its_ids_and_events() {
    let mut s = sim(1);
    let mut b = s.boundary();
    let first = b.spawn(()).unwrap();
    let mark = b.mark();
    let tentative = b.spawn(()).unwrap();
    b.emit(NewEvent::new(kind("test.tentative")));
    assert!(b.entity(tentative).is_some());
    b.rollback(mark);
    assert!(b.entity(tentative).is_none());
    assert!(b.entity(first).is_some());
    let again = b.spawn(()).unwrap();
    assert_eq!(
        again, tentative,
        "the id was never visible outside the write"
    );
    assert!(s.world().resource::<EventInbox>().events().is_empty());
    s.step(&mut NoHooks).unwrap();
}

#[test]
fn reports_carry_decisions_in_request_order() {
    let mut s = sim(1);
    s.add_system(
        "interface.turns",
        TickPhase::Finish,
        RunCondition::Always,
        |mut out: ResMut<TickOutput>, c: Res<SimClock>| {
            for r in ["a", "b"] {
                out.decide(DecisionRequest {
                    observer: id(c.tick.0),
                    reason: r.into(),
                    event: None,
                });
            }
        },
    )
    .unwrap();
    let r = s.step(&mut NoHooks).unwrap();
    let reasons: Vec<&str> = r.decisions.iter().map(|d| d.reason.as_str()).collect();
    assert_eq!(reasons, ["a", "b"]);
    assert_eq!(s.step(&mut NoHooks).unwrap().decisions[0].observer, id(2));
}
