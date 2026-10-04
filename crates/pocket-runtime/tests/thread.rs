//! The game thread (docs/spec/threads.md 11): holding and refusing commands by `at`, the queue's
//! capacity, one client per source, a live run that its recording replays tick for tick, published
//! snapshots that restore to their hash, a panic inside and outside the tick, and the script host's
//! depth limit on the game thread.
#![cfg(all(feature = "thread", feature = "transpile"))]
// The tests' own clock and producer threads stand outside every tick, as a presenter's do.
#![allow(clippy::disallowed_methods)]
mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use pocket_link::{Kind, LoopState, SnapshotView, Source};
use pocket_persist::replay::{
    MemorySink, RecordOptions, Replay, ReplayMode, ReplayOutcome, VerifyOptions, verify,
};
use pocket_physics::{Boat, Transform, Wind};
use pocket_runtime::thread::{Clock, GameHandle, GameThread, ThreadOptions};
use pocket_runtime::{Game, GameBuilder};
use pocket_sim::{RunCondition, Tick, TickPhase};
use serde_json::{Value, json};

fn clock() -> Clock {
    let start = Instant::now();
    Arc::new(move || start.elapsed().as_secs_f64() * 1000.0)
}

fn spawn(
    make: impl FnOnce() -> Result<Game, pocket_contract::Problem> + Send + 'static,
) -> GameHandle {
    GameThread::spawn(make, ThreadOptions::new(clock())).unwrap_or_else(|e| panic!("{e:#?}"))
}

fn sailing() -> GameHandle {
    spawn(|| Game::new(common::sailing(), 1))
}

fn rudder(edit: f64) -> Value {
    json!({"edits": [{"op": "set", "entity": "Sloop", "component": "Boat", "value": {"rudder": edit}}]})
}

fn rudder_now(dev: &mut pocket_link::GameClient) -> f64 {
    let v = dev
        .call(
            "world_get",
            json!({"entity": "Sloop", "components": ["Boat"]}),
        )
        .unwrap()
        .into_json();
    v["components"]["Boat"]["rudder"].as_f64().unwrap()
}

#[test]
fn commands_are_held_for_their_tick_and_refused_when_it_passed() {
    let h = sailing();
    let mut dev = h.developer();
    let mut player = h.client(Source::Player(0)).unwrap();
    // An input of tick 5 waits for boundary 4.
    let (tx, rx) = std::sync::mpsc::channel();
    player
        .send("world_edit", rudder(0.5), Some(Tick(5)), move |r| {
            let _ = tx.send(r.is_ok());
        })
        .unwrap();
    dev.call("step", json!({"ticks": 3})).unwrap();
    assert_eq!(rudder_now(&mut dev), 0.0);
    assert!(rx.try_recv().is_err(), "applied before its boundary");
    // Tick 4 ends at boundary 4, where the input of tick 5 applies.
    dev.call("step", json!({"ticks": 1})).unwrap();
    assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
    assert_eq!(rudder_now(&mut dev), 0.5);
    // An input of a tick whose boundary passed is refused, and nothing changes.
    let e = player
        .call_at("world_edit", rudder(0.9), Some(Tick(3)))
        .unwrap_err();
    assert_eq!(e.code, "command.tick_passed");
    assert_eq!(e.detail["boundary"], json!(4));
    assert_eq!(rudder_now(&mut dev), 0.5);
    drop((dev, player));
    h.shutdown(2000).unwrap();
}

#[test]
fn held_commands_count_against_the_capacity_and_sources_are_one_client_each() {
    let h = sailing();
    let mut p = h.client(Source::Player(0)).unwrap();
    assert_eq!(
        h.client(Source::Player(0)).unwrap_err().code,
        "source.in_use"
    );
    assert_eq!(h.client(Source::Host).unwrap_err().code, "source.in_use");
    for _ in 0..pocket_link::QUEUE_CAPACITY {
        p.send("world_edit", rudder(0.1), Some(Tick(1_000_000)), |_| {})
            .unwrap();
    }
    let e = p
        .send("world_edit", rudder(0.2), Some(Tick(1_000_000)), |_| {})
        .unwrap_err();
    assert_eq!(e.code, "queue.full");
    drop(p);
    let mut again = h.client(Source::Player(0)).unwrap();
    let e = again.call("world_edit", rudder(0.3)).unwrap_err();
    assert_eq!(
        e.code, "queue.full",
        "the held commands still hold their places"
    );
    let mut dev = h.developer();
    assert_eq!(dev.source(), Source::Developer(0));
    assert_eq!(
        dev.call("status", json!({})).unwrap_err().code,
        "queue.full"
    );
    drop((dev, again));
    // The runtime's own shutdown is not kept out by a full queue.
    h.shutdown(2000).unwrap();
}

/// threads.md 5.1: a source's seq runs from 1 and keeps increasing within a run, also when its
/// client is dropped and the source handed out again, so a held envelope of the old client and the
/// new client's never share a seq.
#[test]
fn a_source_handed_out_again_continues_its_sequence() {
    let h = sailing();
    let mut p = h.client(Source::Player(0)).unwrap();
    let a = p
        .send("world_edit", rudder(0.1), Some(Tick(1_000)), |_| {})
        .unwrap();
    let b = p.send("status", json!({}), None, |_| {}).unwrap();
    assert_eq!((a, b), (1, 2));
    drop(p);
    let mut again = h.client(Source::Player(0)).unwrap();
    let c = again.send("status", json!({}), None, |_| {}).unwrap();
    assert_eq!(c, 3);
    // Another source starts at 1.
    let mut other = h.client(Source::Player(1)).unwrap();
    assert_eq!(other.send("status", json!({}), None, |_| {}).unwrap(), 1);
    drop((again, other));
    h.shutdown(2000).unwrap();
}

/// charter 3.4: a real-time speed of 0 or below, or past the limit, is refused and changes nothing.
#[test]
fn time_control_refuses_speeds_it_cannot_run() {
    let h = sailing();
    let mut dev = h.developer();
    for speed in [-4.0, 0.0, 1e6] {
        let e = dev
            .call(
                "time_control",
                json!({"pause": true, "pacing": {"real_time": {"speed": speed}}}),
            )
            .unwrap_err();
        assert_eq!(e.code, "request.out_of_range", "{speed}: {e:#?}");
        assert_eq!(e.detail["path"], json!("/pacing/real_time/speed"));
    }
    let s = dev.call("time_control", json!({})).unwrap().into_json();
    assert_eq!(s["pacing"], json!("stepped"), "{s}");
    assert_eq!(s["paused"], json!(false), "{s}");
    let options = ThreadOptions {
        pacing: pocket_runtime::thread::Pacing::RealTime { speed: -1.0 },
        ..ThreadOptions::new(clock())
    };
    let e = GameThread::spawn(|| Game::new(common::sailing(), 1), options)
        .err()
        .expect("refused");
    assert_eq!(e.code, "request.out_of_range");
    drop(dev);
    h.shutdown(2000).unwrap();
}

/// A world poisoned in real time halts: the time model stops asking for ticks, the status says
/// halted and paused, and the recording holds one `Fault` however long the game stays up.
#[test]
fn a_poisoned_world_halts_real_time_and_records_one_fault() {
    let sink = MemorySink::new();
    let rec = sink.clone();
    let h = spawn(move || {
        let mut g = GameBuilder::new(common::sailing())
            .system(panics_at_ten)
            .build()?;
        g.record(Box::new(rec), RecordOptions::default())?;
        Ok(g)
    });
    let reader = h.reader();
    let mut dev = h.developer();
    dev.call(
        "time_control",
        json!({"pacing": {"real_time": {"speed": 8.0}}}),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !reader.latest().time.halted && Instant::now() < deadline {
        reader.wait_newer(reader.latest().version, 100);
    }
    let latest = reader.latest();
    assert!(latest.time.halted, "{:?}", latest.time);
    assert!(latest.time.paused, "{:?}", latest.time);
    assert_eq!(latest.state().0, 9);
    // At 8x a tick is due every 2 ms: a model still asking would fault on each.
    std::thread::sleep(Duration::from_millis(200));
    let s = dev
        .call("time_control", json!({"pause": false}))
        .unwrap()
        .into_json();
    assert_eq!(s["halted"], json!(true), "{s}");
    std::thread::sleep(Duration::from_millis(100));
    let s = dev.call("time_control", json!({})).unwrap().into_json();
    assert_eq!(
        s["paused"],
        json!(true),
        "a resume asks once and halts again: {s}"
    );
    assert_eq!(
        dev.call("step", json!({"ticks": 1})).unwrap_err().code,
        "sim.world_poisoned"
    );
    drop(dev);
    h.shutdown(2000).unwrap();
    let replay = Replay::read(&sink.bytes()).unwrap();
    let faults = replay
        .records()
        .iter()
        .filter(|r| matches!(r, pocket_persist::replay::Record::Fault { .. }))
        .count();
    assert_eq!(faults, 1);
}

/// threads.md 11, live equals replay: the sailing game in real time at 8x while three producers
/// send writes at wall-clock instants; its recording replays with the same hash at every tick.
#[test]
fn a_live_run_replays_tick_for_tick() {
    live_runs(5);
}

/// The full check's variant (checks.md 6.2: `--include-ignored`): 20 producer timings.
#[test]
#[ignore]
fn twenty_live_runs_replay_tick_for_tick() {
    live_runs(20);
}

fn live_runs(runs: u64) {
    for run in 0..runs {
        let sink = MemorySink::new();
        let rec = sink.clone();
        let h = spawn(move || {
            let mut g = Game::new(common::sailing(), 1 + run)?;
            g.record(Box::new(rec), RecordOptions::default())?;
            Ok(g)
        });
        let reader = h.reader();
        let mut dev = h.developer();
        let producers: Vec<_> = (0..3u32)
            .map(|n| {
                let mut c = h.client(Source::Player(n)).unwrap();
                std::thread::spawn(move || {
                    let mut x = 0x9e37_79b9u64.wrapping_mul(u64::from(n) + 1 + run * 7);
                    for i in 0..12 {
                        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                        std::thread::sleep(Duration::from_millis(20 + (x >> 59)));
                        let value = f64::from(u32::try_from(x >> 40).unwrap_or(0) % 200) / 100.0 - 1.0;
                        let params = if i % 3 == 0 {
                            json!({"edits": [{"op": "set", "entity": "Sloop", "component": "Crew", "value": {"take": format!("Crate{}", 1 + (x >> 62))}}]})
                        } else {
                            rudder(value)
                        };
                        let _ = c.call("world_edit", params);
                    }
                })
            })
            .collect();
        dev.call(
            "time_control",
            json!({"pacing": {"real_time": {"speed": 8.0}}}),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while reader.latest().state().0 < 600 && Instant::now() < deadline {
            reader.wait_newer(reader.latest().version, 100);
        }
        for p in producers {
            p.join().unwrap();
        }
        dev.call("time_control", json!({"pause": true})).unwrap();
        let ticks = reader.latest().state().0;
        assert!(ticks >= 600, "only {ticks} ticks in 30 s");
        drop(dev);
        h.shutdown(2000).unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap();
        let written: usize = replay
            .records()
            .iter()
            .map(|r| match r {
                pocket_persist::replay::Record::Tick { writes, .. } => writes.len(),
                _ => 0,
            })
            .sum();
        assert!(written >= 30, "only {written} writes were recorded");
        let outcome = common::big_stack(move || {
            let replay = Replay::read(&sink.bytes()).unwrap();
            let mut g = Game::for_replay(&replay).unwrap();
            let r = verify(&replay, &mut g, VerifyOptions::mode(ReplayMode::Verify));
            (r.ticks_run, r.outcome)
        });
        assert_eq!(outcome.1, ReplayOutcome::Identical, "run {run}");
        assert!(outcome.0 >= 600);
    }
}

/// threads.md 11, snapshot fidelity: published snapshots restore into a fresh world with their
/// hash, and the columns read through `SnapshotView` equal that world's.
#[test]
fn published_snapshots_restore_and_read_back() {
    let h = sailing();
    let reader = h.reader();
    let mut dev = h.developer();
    let mut snaps = Vec::new();
    for i in 0..50 {
        if i % 7 == 3 {
            dev.call("world_edit", rudder(f64::from(i) / 50.0)).unwrap();
            snaps.push(reader.latest());
        }
        dev.call("step", json!({"ticks": 2})).unwrap();
        snaps.push(reader.latest());
    }
    assert!(reader.latest().version > 50);
    drop(dev);
    h.shutdown(2000).unwrap();
    common::big_stack(move || {
        let mut g = Game::new(common::sailing(), 99).unwrap();
        for s in &snaps {
            g.restore(&s.snapshot).unwrap_or_else(|e| panic!("{e:#?}"));
            assert_eq!(g.world_hash().unwrap(), s.snapshot.world_hash());
            let view = SnapshotView::new(s);
            let w = g.world();
            let index = w.resource::<pocket_sim::EntityIndex>();
            for (id, t) in view.column::<Transform>().unwrap() {
                assert_eq!(w.get::<Transform>(index.get(id).unwrap()), Some(&t));
            }
            for (id, b) in view.column::<Boat>().unwrap() {
                assert_eq!(w.get::<Boat>(index.get(id).unwrap()), Some(&b));
            }
            for (id, wind) in view.column::<Wind>().unwrap() {
                assert_eq!(w.get::<Wind>(index.get(id).unwrap()), Some(&wind));
            }
            let crew = view
                .get_json(pocket_sim::EntityId::new(3).unwrap(), "Crew")
                .unwrap();
            assert!(
                crew.is_some(),
                "the Sloop's Crew through the registry's format"
            );
            let boat = view
                .get_json(pocket_sim::EntityId::new(3).unwrap(), "Boat")
                .unwrap();
            assert_eq!(
                boat.unwrap()["afloat"],
                json!(view.column::<Boat>().unwrap()[0].1.afloat)
            );
        }
    });
}

fn panics_at_ten(sim: &mut pocket_sim::Sim) -> Result<(), pocket_contract::Problem> {
    sim.add_system(
        "test.panic",
        TickPhase::Update,
        RunCondition::Always,
        |clock: bevy_ecs::prelude::Res<pocket_sim::SimClock>| {
            assert!(clock.tick.0 != 10, "the test system panics at tick 10");
        },
    )
}

/// threads.md 11, panic: a system's panic poisons the world, not the thread; a panic outside the
/// tick stops the thread, and every later call answers `game.stopped`.
#[test]
fn panics_poison_the_world_or_stop_the_thread() {
    let h = spawn(|| {
        GameBuilder::new(common::sailing())
            .system(panics_at_ten)
            .command(
                "test.panic",
                Kind::Write,
                Arc::new(|_, _| panic!("a command panicked outside the tick")),
            )
            .build()
    });
    let reader = h.reader();
    let mut dev = h.developer();
    let e = dev.call("step", json!({"ticks": 20})).unwrap_err();
    assert_eq!(e.code, "sim.internal", "{e:#?}");
    let e = dev.call("step", json!({"ticks": 1})).unwrap_err();
    assert_eq!(e.code, "sim.world_poisoned");
    assert_eq!(reader.latest().state().0, 9);
    let s = dev.call("status", json!({})).unwrap().into_json();
    assert_eq!(s["poisoned"], json!(10), "{s}");
    let e = dev.call("test.panic", json!({})).unwrap_err();
    assert_eq!(e.code, "game.stopped", "{e:#?}");
    let e = dev.call("status", json!({})).unwrap_err();
    assert_eq!(e.code, "game.stopped");
    assert_eq!(reader.status().state, LoopState::Stopped);
    assert!(reader.stop_reason().unwrap().message.contains("panicked"));
    assert_eq!(reader.latest().state().0, 9);
    drop(dev);
    let _ = h.shutdown(2000);
    // A poisoned world steps again after a restore.
    common::big_stack(|| {
        let mut g = GameBuilder::new(common::sailing())
            .system(panics_at_ten)
            .build()
            .unwrap();
        g.run(5).unwrap();
        let snap = g.snapshot().unwrap();
        g.run(4).unwrap();
        assert_eq!(g.step().unwrap_err().code, "sim.internal");
        assert_eq!(g.step().unwrap_err().code, "sim.world_poisoned");
        g.restore(&snap).unwrap();
        g.run(4).unwrap();
    });
}

/// threads.md 11, stacks: a script recursing without end on the game thread stops at the call depth
/// limit, never at a stack overflow.
#[test]
fn the_depth_limit_holds_on_the_game_thread() {
    let source = pocket_script::ScriptSource::new().with(
        "scripts/main.ts",
        r#"import { game, system } from "pocket";
function down(n: number): number { return n <= 0 ? 0 : 1 + down(n + 1); }
const deep = system({ name: "deep", phase: "update", doc: "Recurses without end.",
    run(_ctx) { down(1); } });
export default game({ systems: [deep] });
"#,
    );
    let mut setup = (*common::sailing()).clone();
    setup.scene = pocket_runtime::Scene::empty();
    setup.scripts = pocket_runtime::compile(&source, false).unwrap();
    setup.source = Some(source);
    let setup = Arc::new(setup);
    let h = spawn(move || Game::new(setup, 1));
    let mut dev = h.developer();
    let r = dev.call("step", json!({"ticks": 1}));
    let text = format!("{r:?}");
    println!("{text}");
    assert!(text.contains("script.call_depth"), "{text}");
    assert!(!text.contains("stack_overflow"), "{text}");
    let s = dev.call("status", json!({})).unwrap().into_json();
    assert!(s["tick"].as_u64().unwrap() <= 1);
    drop(dev);
    h.shutdown(2000).unwrap();
}
