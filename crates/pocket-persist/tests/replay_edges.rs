//! The edges of recording and verifying (docs/spec/replay.md 2.3, 3.4 and 4; versions.md 7): a
//! sink failure surfaces at the next call, ticks out of sequence are refused, `from` outside the
//! replay is `replay.out_of_range`, a recording without its bundles verifies where the stepper finds
//! them, and `Rerun` migrates an older schema and rebuilds a cache whose identity changed (V5).

mod common;

use std::sync::Arc;

use bevy_ecs::prelude::*;
use common::{Counter, Game, Vel, game, input, inputs, record, run};
use pocket_contract::{Problem, detail};
use pocket_persist::migrate::{Apply, MigrationCtx, MigrationError, MigrationStep, Migrations};
use pocket_persist::replay::{
    BundleRecord, MemorySink, RebaseCause, RecordOptions, RecordSink, Recorder, Replay, ReplayMode,
    ReplayOutcome, SegmentEnd, VerifyOptions, verify,
};
use pocket_persist::version::BundleUse;
use pocket_persist::{
    RecordedWrite, Registry, ReplaySource, RestoreOptions, Snapshot, Stepper, TickRef, restore,
    sim_registry, snapshot,
};
use pocket_sim::{
    Event, NoHooks, Persisted, PersistedCache, RegisterPersisted, Sim, SimConfig, Staged, Tick,
    TickRate,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

fn at(segment: u32, tick: u64) -> TickRef {
    TickRef {
        segment,
        tick: Tick(tick),
    }
}

fn opts(keyframe_every: u32) -> RecordOptions {
    RecordOptions {
        keyframe_every,
        ..RecordOptions::default()
    }
}

/// A sink that accepts `left` writes and then fails every one.
struct Failing {
    inner: MemorySink,
    left: usize,
}

impl RecordSink for Failing {
    fn write(&mut self, record: &[u8]) -> std::io::Result<()> {
        if self.left == 0 {
            return Err(std::io::Error::other("the disk is full"));
        }
        self.left -= 1;
        self.inner.write(record)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A record that cannot be written inside a call without a result (here the data of a load) breaks
/// the recording: every later call that returns a result returns the failure, nothing more is
/// written, and what was written reads as a recording whose segment never ended.
#[test]
fn a_sink_failure_surfaces_at_the_next_call() {
    let mut g = game(14);
    let sink = MemorySink::new();
    // `start` writes the head, `Start`, `Formats` and `Bundle`; the fifth write fails.
    let mut rec = Recorder::start(
        g.sim.world(),
        g.reg.clone(),
        &BundleRecord::empty(),
        "{}",
        opts(0),
        Box::new(Failing {
            inner: sink.clone(),
            left: 4,
        }),
    )
    .unwrap();
    let (path, hash, bytes) = common::content();
    rec.data_read(path, hash, bytes);
    for _ in 0..2 {
        g.sim.step(&mut NoHooks).unwrap();
        let e = rec.end_tick(g.sim.world()).unwrap_err();
        assert_eq!(e.code, "persist.encode");
        assert!(e.message.contains("the disk is full"), "{}", e.message);
    }
    assert!(rec.formats_changed(g.sim.world()).is_err());
    assert!(rec.end_segment(g.sim.world()).is_err());
    assert!(rec.finish(g.sim.world()).is_err());
    let replay = Replay::read(&sink.bytes()).unwrap();
    assert_eq!(replay.segments()[0].end, SegmentEnd::Open);
    assert_eq!(replay.records().len(), 3);
}

/// `end_tick` after `end_segment` without `rebase` is `replay.segment_closed`, and after a restore
/// the recorder was not told of it is `replay.tick_sequence` (it used to underflow); neither
/// writes, so the file stays readable.
#[test]
fn ticks_out_of_sequence_are_refused() {
    let mut g = game(15);
    let sink = MemorySink::new();
    let bundle = BundleRecord::empty();
    let mut rec = Recorder::start(
        g.sim.world(),
        g.reg.clone(),
        &bundle,
        "{}",
        opts(4),
        Box::new(sink.clone()),
    )
    .unwrap();
    let s0 = snapshot(g.sim.world(), &g.reg).unwrap();
    run(&mut g, Some(&mut rec), &[], 5);
    let s5 = snapshot(g.sim.world(), &g.reg).unwrap();
    run(&mut g, Some(&mut rec), &[], 10);
    rec.end_segment(g.sim.world()).unwrap();
    g.sim.step(&mut NoHooks).unwrap();
    let e = rec.end_tick(g.sim.world()).unwrap_err();
    assert_eq!(e.code, "replay.segment_closed");
    assert!(Replay::read(&sink.bytes()).is_ok());

    restore(g.sim.world_mut(), &s5, &g.reg, RestoreOptions::default()).unwrap();
    rec.rebase(g.sim.world(), RebaseCause::Restore, &bundle)
        .unwrap();
    run(&mut g, Some(&mut rec), &[], 8);
    // Back to tick 0 without end_segment and rebase: the next tick is 1, not 9.
    restore(g.sim.world_mut(), &s0, &g.reg, RestoreOptions::default()).unwrap();
    g.sim.step(&mut NoHooks).unwrap();
    let e = rec.end_tick(g.sim.world()).unwrap_err();
    assert_eq!(
        (
            e.code.as_str(),
            e.detail["expected"].clone(),
            e.detail["found"].clone()
        ),
        ("replay.tick_sequence", json!(9), json!(1))
    );
    let replay = Replay::read(&sink.bytes()).unwrap();
    assert_eq!(replay.ticks(1), Tick(6)..=Tick(8));
}

/// `from` outside the replay stops verify with `replay.out_of_range` before any tick runs, and
/// is not a verification; a `from` inside starts at the keyframe at or before it.
#[test]
fn verify_from_outside_the_replay_is_out_of_range() {
    let bytes = record(&mut game(16), &inputs(), 20, opts(10));
    let replay = Replay::read(&bytes).unwrap();
    for from in [at(7, 0), at(0, 9999)] {
        let mut o = VerifyOptions::mode(ReplayMode::Verify);
        o.from = Some(from);
        let r = verify(&replay, &mut game(1), o);
        assert_eq!((r.verified, r.ticks_run), (false, 0));
        let ReplayOutcome::Stopped(stop, p) = &r.outcome else {
            panic!("{:?}", r.outcome)
        };
        assert_eq!((*stop, p.code.as_str()), (from, "replay.out_of_range"));
    }
    let mut o = VerifyOptions::mode(ReplayMode::Verify);
    o.from = Some(at(0, 15));
    let r = verify(&replay, &mut game(1), o);
    assert_eq!((r.outcome, r.ticks_run), (ReplayOutcome::Identical, 10));
}

/// The test game with a content store of bundles (versions.md 7.4).
struct Finder(Game, BundleRecord);

impl Stepper for Finder {
    fn world(&self) -> &World {
        self.0.world()
    }
    fn world_mut(&mut self) -> &mut World {
        self.0.world_mut()
    }
    fn registry(&self) -> &Registry {
        self.0.registry()
    }
    fn restore(&mut self, s: &Snapshot, b: &BundleRecord) -> Result<(), Problem> {
        assert_eq!(b.hash, self.1.hash, "restored with the bundle it found");
        self.0.restore(s, b)
    }
    fn apply(&mut self, w: &RecordedWrite, s: &dyn ReplaySource) -> Result<(), Problem> {
        self.0.apply(w, s)
    }
    fn step(&mut self) -> Result<Vec<Event>, Problem> {
        self.0.step()
    }
    fn run_config(&self) -> String {
        self.0.run_config()
    }
    fn find_bundle(&self, h: &pocket_sim::ContentHash) -> Option<BundleRecord> {
        (*h == self.1.hash).then(|| self.1.clone())
    }
}

/// With `embed_bundles: false` the bundle is `Unavailable` to a stepper that cannot find it
/// (`Verify` refuses, `Compare` stops at the start) and `Available` to one that finds it by hash,
/// which verifies the recording.
#[test]
fn a_recording_without_its_bundles_verifies_where_the_stepper_finds_them() {
    let bundle = BundleRecord::new(vec![("main.ts".into(), b"export {}".to_vec())], None);
    let mut g = game(17);
    let sink = MemorySink::new();
    let mut rec = Recorder::start(
        g.sim.world(),
        g.reg.clone(),
        &bundle,
        "{}",
        RecordOptions {
            embed_bundles: false,
            keyframe_every: 0,
            ..RecordOptions::default()
        },
        Box::new(sink.clone()),
    )
    .unwrap();
    run(&mut g, Some(&mut rec), &inputs(), 30);
    rec.finish(g.sim.world()).unwrap();
    let replay = Replay::read(&sink.bytes()).unwrap();
    assert!(replay.bundle(&bundle.hash).is_none());

    let r = verify(
        &replay,
        &mut game(1),
        VerifyOptions::mode(ReplayMode::Verify),
    );
    assert_eq!(r.versions.bundles, [(bundle.hash, BundleUse::Unavailable)]);
    assert!(matches!(&r.outcome, ReplayOutcome::Stopped(_, p) if p.code == "version.mismatch"));
    let r = verify(
        &replay,
        &mut game(1),
        VerifyOptions::mode(ReplayMode::Compare),
    );
    assert!(
        matches!(&r.outcome, ReplayOutcome::Stopped(t, p) if *t == at(0, 0) && p.code == "replay.bundle_unavailable")
    );

    let r = verify(
        &replay,
        &mut Finder(game(1), bundle.clone()),
        VerifyOptions::mode(ReplayMode::Verify),
    );
    assert_eq!(r.versions.bundles, [(bundle.hash, BundleUse::Available)]);
    assert_eq!((r.outcome, r.ticks_run), (ReplayOutcome::Identical, 30));
}

/// `Pos` at version 2: a height added.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename = "Pos")]
struct PosV2 {
    x: f64,
    y: f64,
    height: f64,
}

impl Persisted for PosV2 {
    const NAME: &'static str = "Pos";
    const VERSION: u32 = 2;
}

fn add_height(_: &MigrationCtx, mut v: Value) -> Result<Value, MigrationError> {
    let (Some(x), Some(y)) = (v["x"].as_f64(), v["y"].as_f64()) else {
        return Err(MigrationError::field("/x"));
    };
    v.as_object_mut()
        .ok_or_else(|| MigrationError::field(""))?
        .insert("height".into(), json!(x + y));
    Ok(v)
}

fn pos_step() -> MigrationStep {
    MigrationStep {
        kind: pocket_persist::SectionKind::Component,
        section: "Pos",
        from: 1,
        apply: Apply::Value(add_height),
        examples: &[(r#"{"x":1.0,"y":2.0}"#, r#"{"x":1.0,"y":2.0,"height":3.0}"#)],
    }
}

/// The test cache under another library layout: a load rebuilds it, marking the counter.
struct CounterV2;

impl PersistedCache for CounterV2 {
    const NAME: &'static str = "test.counter";
    fn identity() -> &'static str {
        "test counter layout 2"
    }
    fn encode(_: &World, _: &mut Vec<u8>) -> Result<(), Problem> {
        Ok(())
    }
    fn decode(_: &[u8]) -> Result<Box<dyn Staged>, Problem> {
        Err(Problem::new(
            "persist.noncanonical",
            "layout 2 writes nothing",
            detail([]),
        ))
    }
    fn rebuild(world: &mut World) -> Result<(), Problem> {
        world.insert_resource(Counter {
            hits: 999,
            wave: 0.0,
            limit: 0.0,
        });
        Ok(())
    }
}

/// A newer engine: `Pos` at version 2, the counter cache in layout 2, and no systems.
struct V2 {
    sim: Sim,
    reg: Registry,
}

fn v2() -> V2 {
    let mut b = sim_registry();
    b.component::<PosV2>()
        .component::<Vel>()
        .component::<common::Tag>()
        .component::<common::Bag>()
        .resource::<common::Score>()
        .cache::<CounterV2>()
        .derived::<Counter>("the state of cache test.counter, rebuilt by a load");
    V2 {
        sim: Sim::new(SimConfig {
            rate: TickRate::DEFAULT,
            seed: 1,
        })
        .unwrap(),
        reg: b.build().unwrap(),
    }
}

impl Stepper for V2 {
    fn world(&self) -> &World {
        self.sim.world()
    }
    fn world_mut(&mut self) -> &mut World {
        self.sim.world_mut()
    }
    fn registry(&self) -> &Registry {
        &self.reg
    }
    fn restore(&mut self, s: &Snapshot, _: &BundleRecord) -> Result<(), Problem> {
        restore(
            self.sim.world_mut(),
            s,
            &self.reg,
            RestoreOptions::default(),
        )
        .map(drop)
    }
    fn apply(&mut self, w: &RecordedWrite, s: &dyn ReplaySource) -> Result<(), Problem> {
        common::apply_src(&mut self.sim, &w.name, &w.params_json(), &|h| s.data(h))
    }
    fn step(&mut self) -> Result<Vec<Event>, Problem> {
        self.sim.step(&mut NoHooks).map(|r| r.events)
    }
    fn run_config(&self) -> String {
        "{}".to_owned()
    }
}

/// V5, schemas: a recording made with `Pos` at version 1 is refused by `Compare` in an engine with
/// version 2, and runs in `Rerun`, its start and keyframes migrated as a load migrates them and the
/// cache of another identity rebuilt; without the step, `Rerun` stops with `migrate.missing_step`.
#[test]
#[allow(clippy::disallowed_methods)] // every row is checked or counted: their order changes nothing
fn rerun_migrates_an_older_schema() {
    let ins = [
        input(5, "push", json!({"id": 1, "dx": 2.0, "dy": -1.0})),
        input(9, "tag", json!({"id": 2, "n": 7})),
        input(12, "bag", json!({"id": 3, "item": "rope", "count": 4})),
        input(26, "untag", json!({"id": 2})),
    ];
    let bytes = record(&mut game(18), &ins, 30, opts(10));
    let replay = Replay::read(&bytes).unwrap();

    let r = verify(&replay, &mut v2(), VerifyOptions::mode(ReplayMode::Compare));
    assert!(
        matches!(&r.outcome, ReplayOutcome::Stopped(_, p) if p.code == "version.schema_differs")
    );
    let r = verify(&replay, &mut v2(), VerifyOptions::mode(ReplayMode::Rerun));
    assert!(
        matches!(&r.outcome, ReplayOutcome::Stopped(t, p) if *t == at(0, 0) && p.code == "migrate.missing_step"),
        "{:?}",
        r.outcome
    );

    let mut o = VerifyOptions::mode(ReplayMode::Rerun);
    o.migrations = Arc::new(Migrations::new(vec![pos_step()], Vec::new()));
    let mut s = v2();
    let r = verify(&replay, &mut s, o.clone());
    assert_eq!(
        (r.outcome, r.ticks_run, r.verified),
        (ReplayOutcome::Identical, 30, false)
    );
    let w = s.sim.world_mut();
    assert_eq!(w.resource::<Counter>().hits, 999, "the cache was rebuilt");
    let mut q = w.query::<&PosV2>();
    assert!(q.iter(w).all(|p| p.height == p.x + p.y));
    assert_eq!(q.iter(w).count(), 3);

    // From a keyframe: tick 10's snapshot holds the bodies spawned since, migrated too.
    o.from = Some(at(0, 15));
    let mut s = v2();
    let r = verify(&replay, &mut s, o);
    assert_eq!((r.outcome, r.ticks_run), (ReplayOutcome::Identical, 20));
    let w = s.sim.world_mut();
    assert!(w.query::<&PosV2>().iter(w).count() > 3);
}
