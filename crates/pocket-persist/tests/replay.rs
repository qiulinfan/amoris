//! Replays and divergence (docs/spec/replay.md 5): verify, the first diverging tick of a replay
//! with one input moved one tick, segments (R1), section digests (R2), faults and taints (R5),
//! refused writes (R6), truncation, and lockstep.

mod common;

use common::{Game, Input, game, input, inputs, record, run};
use pocket_persist::replay::{
    BundleRecord, DigestLevel, MemorySink, RebaseCause, Record, RecordOptions, Recorder, Replay,
    ReplayMode, ReplayOutcome, SeekScripts, VerifyOptions, first_divergence, lockstep, seek,
    verify,
};
use pocket_persist::{
    DivergenceKind, LockstepOptions, RestoreOptions, SectionKey, SectionKind, Side, TickRef,
    restore, snapshot,
};
use pocket_sim::Tick;
use serde_json::json;

const T: u64 = 30;

fn opts(keyframe_every: u32) -> RecordOptions {
    RecordOptions {
        keyframe_every,
        ..RecordOptions::default()
    }
}

fn verify_mode(bytes: &[u8], mode: ReplayMode) -> pocket_persist::ReplayReport {
    let replay = Replay::read(bytes).unwrap();
    verify(&replay, &mut game(500), VerifyOptions::mode(mode))
}

fn at(segment: u32, tick: u64) -> TickRef {
    TickRef {
        segment,
        tick: Tick(tick),
    }
}

/// Moves the first write of tick `from` to the front of tick `from + 1`.
fn move_write(bytes: &[u8], from: u64) -> Vec<u8> {
    let (header, mut records, complete) = Replay::read(bytes).unwrap().into_parts();
    let i = records
        .iter()
        .position(|r| matches!(r, Record::Tick { tick, .. } if tick.0 == from))
        .unwrap();
    let Record::Tick { writes, .. } = &mut records[i] else {
        unreachable!()
    };
    let w = writes.remove(0);
    let next = records
        .iter()
        .position(|r| matches!(r, Record::Tick { tick, .. } if tick.0 == from + 1))
        .unwrap();
    let Record::Tick { writes, .. } = &mut records[next] else {
        unreachable!()
    };
    writes.insert(0, w);
    Replay::from_records(header, records, complete)
        .unwrap()
        .to_bytes()
        .unwrap()
}

#[test]
fn a_recording_verifies() {
    let bytes = record(&mut game(1), &inputs(), 120, opts(50));
    let replay = Replay::read(&bytes).unwrap();
    assert!(replay.complete());
    assert_eq!(replay.ticks(0), Tick(1)..=Tick(120));
    assert_eq!(replay.writes(at(0, 5)).len(), 1);
    let report = verify(
        &replay,
        &mut game(77),
        VerifyOptions::mode(ReplayMode::Verify),
    );
    assert_eq!(report.outcome, ReplayOutcome::Identical);
    assert_eq!(report.ticks_run, 120);
    assert!(report.verified);
    // The bytes the reader writes back are the recorder's.
    assert_eq!(replay.to_bytes().unwrap(), bytes);
    // Starting at a keyframe re-runs only from there.
    let mut o = VerifyOptions::mode(ReplayMode::Verify);
    o.from = Some(at(0, 110));
    let report = verify(&replay, &mut game(77), o);
    assert_eq!(report.outcome, ReplayOutcome::Identical);
    assert_eq!(report.ticks_run, 20);
}

/// The localization check (checks.md 8.4): a copy with one input moved one tick later diverges at
/// the tick the input was moved from, naming the section it changed; with a keyframe on that
/// tick, the field and both values.
#[test]
fn an_input_moved_one_tick_names_that_tick() {
    let ins = vec![input(T, "push", json!({"id": 2, "dx": 1.5, "dy": 0.0}))];
    let bytes = record(&mut game(3), &ins, 60, opts(1));
    let moved = move_write(&bytes, T);
    let report = verify_mode(&moved, ReplayMode::Verify);
    let ReplayOutcome::Diverged(d) = report.outcome else {
        panic!("{:?}", report.outcome)
    };
    assert_eq!(d.at, at(0, T));
    assert_eq!(d.kind, DivergenceKind::Hash);
    assert!(
        d.writes.is_empty(),
        "the moved write is no longer at this tick"
    );
    let vel = SectionKey::new(SectionKind::Component, "Vel");
    assert!(d.sections.contains(&vel), "{:?}", d.sections);
    let f = d.fields.iter().find(|f| f.section == vel).unwrap();
    assert_eq!(f.entity.unwrap().get(), 2);
    assert_eq!(f.path, "/x");
    // The same, from two recordings compared by their tick hashes.
    let other = record(
        &mut game(3),
        &[input(T + 1, "push", json!({"id": 2, "dx": 1.5, "dy": 0.0}))],
        60,
        opts(0),
    );
    let a = Replay::read(&bytes).unwrap().tick_hashes();
    let b = Replay::read(&other).unwrap().tick_hashes();
    let d = first_divergence(&a, &b).unwrap();
    assert_eq!(d.at, at(0, T));
    // Entity 2 is despawned at tick 33 (the oldest of more than four goes every 11 ticks), and
    // with it the only difference.
    assert_eq!(d.reconverged_at, Some(at(0, 33)));
}

/// A divergence that disappears again is reported with where it reconverged: a copy that tags an
/// entity at boundary 9 and untags it at boundary 11 differs at ticks 10 and 11 only.
#[test]
fn reconvergence_is_reported() {
    let bytes = record(&mut game(4), &[], 40, opts(0));
    let (header, mut records, complete) = Replay::read(&bytes).unwrap().into_parts();
    for r in &mut records {
        if let Record::Tick { tick, writes, .. } = r {
            let w = |name: &str, params: &str| pocket_persist::RecordedWrite {
                source: pocket_persist::Source::Editor,
                seq: tick.0,
                name: name.into(),
                params: params.into(),
            };
            match tick.0 {
                10 => writes.push(w("tag", r#"{"id":1,"n":5}"#)),
                12 => writes.push(w("untag", r#"{"id":1}"#)),
                _ => {}
            }
        }
    }
    let copy = Replay::from_records(header, records, complete)
        .unwrap()
        .to_bytes()
        .unwrap();
    let replay = Replay::read(&copy).unwrap();
    let mut o = VerifyOptions::mode(ReplayMode::Verify);
    o.continue_after_divergence = true;
    let report = verify(&replay, &mut game(4), o);
    let ReplayOutcome::Diverged(d) = report.outcome else {
        panic!("{:?}", report.outcome)
    };
    assert_eq!(d.at, at(0, 10));
    assert_eq!(
        d.sections,
        vec![SectionKey::new(SectionKind::Component, "Tag")]
    );
    assert_eq!(d.reconverged_at, Some(at(0, 12)));
}

/// Records a session that restores a session snapshot, adopts a branch and resets: four segments.
fn session(seg1: &[Input]) -> Vec<u8> {
    let mut g = game(5);
    let sink = MemorySink::new();
    let bundle = BundleRecord::empty();
    let mut rec = Recorder::start(
        g.sim.world(),
        g.reg.clone(),
        &bundle,
        "{}",
        opts(25),
        Box::new(sink.clone()),
    )
    .unwrap();
    run(&mut g, Some(&mut rec), &inputs(), 30);
    let s1 = snapshot(g.sim.world(), &g.reg).unwrap();
    run(&mut g, Some(&mut rec), &inputs(), 50);
    rec.end_segment(g.sim.world()).unwrap();
    restore(g.sim.world_mut(), &s1, &g.reg, RestoreOptions::default()).unwrap();
    rec.rebase(g.sim.world(), RebaseCause::Restore, &bundle)
        .unwrap();
    run(&mut g, Some(&mut rec), seg1, 70);
    // A branch run elsewhere and adopted as main.
    let mut branch = game(5);
    restore(
        branch.sim.world_mut(),
        &s1,
        &branch.reg,
        RestoreOptions::default(),
    )
    .unwrap();
    run(
        &mut branch,
        None,
        &[input(35, "spawn", json!({"x": 1.0, "y": 2.0}))],
        45,
    );
    let adopted = snapshot(branch.sim.world(), &branch.reg).unwrap();
    rec.end_segment(g.sim.world()).unwrap();
    restore(
        g.sim.world_mut(),
        &adopted,
        &g.reg,
        RestoreOptions::default(),
    )
    .unwrap();
    rec.rebase(g.sim.world(), RebaseCause::Restore, &bundle)
        .unwrap();
    run(&mut g, Some(&mut rec), &[], 60);
    rec.end_segment(g.sim.world()).unwrap();
    let fresh = game(5);
    let start = snapshot(fresh.sim.world(), &fresh.reg).unwrap();
    restore(g.sim.world_mut(), &start, &g.reg, RestoreOptions::default()).unwrap();
    rec.rebase(g.sim.world(), RebaseCause::Reset, &bundle)
        .unwrap();
    run(&mut g, Some(&mut rec), &inputs(), 20);
    rec.finish(g.sim.world()).unwrap();
    sink.bytes()
}

/// R1: a session with a restore, an adopted branch and a reset verifies across its rebases; seek
/// reproduces a tick of each segment; two recordings that differ only in segment 1 diverge there.
#[test]
fn r1_segments() {
    let bytes = session(&[]);
    let replay = Replay::read(&bytes).unwrap();
    let segs = replay.segments();
    assert_eq!(segs.len(), 4);
    assert_eq!(segs[1].cause, Some(RebaseCause::Restore));
    assert_eq!(segs[3].cause, Some(RebaseCause::Reset));
    assert_eq!((segs[1].start.0, segs[1].last), (Tick(30), Tick(70)));
    assert_eq!((segs[2].start.0, segs[2].last), (Tick(45), Tick(60)));
    assert_eq!((segs[3].start.0, segs[3].last), (Tick(0), Tick(20)));
    let report = verify(
        &replay,
        &mut game(1),
        VerifyOptions::mode(ReplayMode::Verify),
    );
    assert_eq!(report.outcome, ReplayOutcome::Identical);
    assert_eq!(report.ticks_run, 50 + 40 + 15 + 20);
    for (segment, tick) in [(0, 40), (1, 33), (1, 70), (2, 51), (3, 7)] {
        let r = seek(
            &replay,
            &mut game(2),
            at(segment, tick),
            SeekScripts::Recorded,
        )
        .unwrap();
        assert_eq!(
            Some(r.world),
            replay.hash(at(segment, tick)),
            "seek to {segment}:{tick}"
        );
    }
    let e = seek(&replay, &mut game(2), at(1, 71), SeekScripts::Recorded).unwrap_err();
    assert_eq!(e.code, "replay.out_of_range");
    let other = session(&[input(40, "push", json!({"id": 5, "dx": 1.0, "dy": 1.0}))]);
    let d = first_divergence(
        &replay.tick_hashes(),
        &Replay::read(&other).unwrap().tick_hashes(),
    )
    .unwrap();
    assert_eq!(d.at, at(1, 40));
}

/// R2: sections appear and disappear (the Tag component's last row is removed, then added again);
/// two independent readers rebuild the same running lists; a divergence in the reappeared section
/// is named by its key.
#[test]
fn r2_section_digests() {
    let ins = vec![
        input(5, "tag", json!({"id": 1, "n": 1})),
        input(10, "untag", json!({"id": 1})),
        input(15, "tag", json!({"id": 2, "n": 2})),
    ];
    let bytes = record(&mut game(6), &ins, 25, opts(0));
    let (a, b) = (Replay::read(&bytes).unwrap(), Replay::read(&bytes).unwrap());
    let tag = SectionKey::new(SectionKind::Component, "Tag");
    for t in 1..=25 {
        let (da, db) = (a.digests(at(0, t)).unwrap(), b.digests(at(0, t)).unwrap());
        assert_eq!(da, db);
        let has = da.iter().any(|(k, _)| *k == tag);
        assert_eq!(has, (5..10).contains(&t) || t >= 15, "Tag present at {t}");
    }
    let (header, mut records, complete) = a.into_parts();
    for r in &mut records {
        if let Record::Tick { tick, writes, .. } = r
            && tick.0 == 15
        {
            writes[0].params = r#"{"id":2,"n":3}"#.into();
        }
    }
    let copy = Replay::from_records(header, records, complete)
        .unwrap()
        .to_bytes()
        .unwrap();
    let report = verify_mode(&copy, ReplayMode::Verify);
    let ReplayOutcome::Diverged(d) = report.outcome else {
        panic!("{:?}", report.outcome)
    };
    assert_eq!(d.at, at(0, 15));
    assert_eq!(d.sections, vec![tag]);
    // World-level digests only: the tick is named, the sections are not.
    let world_only = record(
        &mut game(6),
        &ins,
        25,
        RecordOptions {
            digests: DigestLevel::World,
            keyframe_every: 0,
            ..RecordOptions::default()
        },
    );
    assert!(
        Replay::read(&world_only)
            .unwrap()
            .digests(at(0, 3))
            .is_none()
    );
}

/// R5: a recording whose run faulted stops `verify` at the fault's tick with its code; one made
/// while a debugger evaluated inside a tick stops with `replay.tainted` there.
#[test]
fn r5_faults_and_taints() {
    let mut g = game(7);
    let sink = MemorySink::new();
    let mut rec = Recorder::start(
        g.sim.world(),
        g.reg.clone(),
        &BundleRecord::empty(),
        "{}",
        opts(0),
        Box::new(sink.clone()),
    )
    .unwrap();
    run(&mut g, Some(&mut rec), &[], 12);
    rec.fault(Tick(13), "script.update", "script.out_of_memory");
    rec.finish(g.sim.world()).unwrap();
    let report = verify_mode(&sink.bytes(), ReplayMode::Verify);
    let ReplayOutcome::Stopped(where_, p) = report.outcome else {
        panic!()
    };
    assert_eq!(
        (where_, p.code.as_str()),
        (at(0, 13), "script.out_of_memory")
    );
    assert_eq!(report.ticks_run, 12);

    let mut g = game(7);
    let sink = MemorySink::new();
    let mut rec = Recorder::start(
        g.sim.world(),
        g.reg.clone(),
        &BundleRecord::empty(),
        "{}",
        opts(0),
        Box::new(sink.clone()),
    )
    .unwrap();
    run(&mut g, Some(&mut rec), &[], 8);
    rec.tainted(Tick(9), "a debugger evaluated an expression");
    run(&mut g, Some(&mut rec), &[], 20);
    rec.finish(g.sim.world()).unwrap();
    let report = verify_mode(&sink.bytes(), ReplayMode::Verify);
    let ReplayOutcome::Stopped(where_, p) = report.outcome else {
        panic!()
    };
    assert_eq!((where_, p.code.as_str()), (at(0, 9), "replay.tainted"));
}

/// R6: a recorded write the replaying engine refuses stops `verify` with `replay.write_refused`;
/// lockstep reports `DivergenceKind::Write` naming the side that refused.
#[test]
fn r6_refused_writes() {
    let bytes = record(
        &mut game(8),
        &[input(6, "push", json!({"id": 1, "dx": 1.0, "dy": 0.0}))],
        12,
        opts(0),
    );
    let (header, mut records, complete) = Replay::read(&bytes).unwrap().into_parts();
    for r in &mut records {
        if let Record::Tick { tick, writes, .. } = r
            && tick.0 == 6
        {
            writes[0].params = r#"{"dx":1.0,"dy":0.0,"id":999}"#.into();
        }
    }
    let copy = Replay::from_records(header, records, complete)
        .unwrap()
        .to_bytes()
        .unwrap();
    let report = verify_mode(&copy, ReplayMode::Compare);
    let ReplayOutcome::Stopped(where_, p) = report.outcome else {
        panic!("{:?}", report.outcome)
    };
    assert_eq!(
        (where_, p.code.as_str()),
        (at(0, 6), "replay.write_refused")
    );
    assert_eq!(p.detail["error"]["code"], "sim.entity_not_found");

    let (mut a, mut b) = (game(8), game(8));
    let mut writes = |t: Tick, side: Side| {
        let id = if t.0 == 4 && side == Side::Actual {
            999
        } else {
            1
        };
        vec![pocket_persist::RecordedWrite {
            source: pocket_persist::Source::Player(0),
            seq: t.0,
            name: "push".into(),
            params: format!(r#"{{"dx":0.5,"dy":0.0,"id":{id}}}"#),
        }]
    };
    let r = lockstep(
        &mut a,
        &mut b,
        &mut writes,
        LockstepOptions::until(Tick(10)),
    );
    let d = r.divergence.unwrap();
    assert_eq!(d.at, at(0, 4));
    assert_eq!(
        d.kind,
        DivergenceKind::Write {
            index: 0,
            refused_by: Side::Actual
        }
    );
}

/// Lockstep of two games of one seed and inputs never diverges; with one input changed it names
/// the tick, the sections and the fields with both values.
#[test]
fn lockstep_same_process() {
    let ins = inputs();
    let feed = |side_change: Option<u64>| {
        let ins = ins.clone();
        move |t: Tick, side: Side| -> Vec<pocket_persist::RecordedWrite> {
            ins.iter()
                .filter(|i| i.tick == t.0)
                .map(|i| {
                    let mut p = i.params.clone();
                    if side == Side::Actual && side_change == Some(t.0) {
                        p["dx"] = json!(9.0);
                    }
                    pocket_persist::RecordedWrite {
                        source: pocket_persist::Source::Player(0),
                        seq: t.0,
                        name: i.name.into(),
                        params: p.to_string(),
                    }
                })
                .collect()
        }
    };
    let (mut a, mut b): (Game, Game) = (game(9), game(9));
    let r = lockstep(
        &mut a,
        &mut b,
        &mut feed(None),
        LockstepOptions::until(Tick(200)),
    );
    assert_eq!((r.ticks_run, r.divergence), (200, None));
    let (mut a, mut b) = (game(9), game(9));
    let r = lockstep(
        &mut a,
        &mut b,
        &mut feed(Some(31)),
        LockstepOptions::until(Tick(200)),
    );
    let d = r.divergence.unwrap();
    assert_eq!(d.at, at(0, 31));
    let vel = d.fields.iter().find(|f| f.section.name == "Vel").unwrap();
    assert_eq!((vel.entity.unwrap().get(), vel.path.as_str()), (2, "/x"));
    assert_ne!(vel.expected, vel.actual);
}

/// A recording cut short reads up to its last whole record and verifies that far; a cut inside the
/// start is `replay.truncated`; an unknown record tag is `replay.format`.
#[test]
fn truncation_and_format() {
    let bytes = record(&mut game(10), &[], 30, opts(0));
    let cut = &bytes[..bytes.len() - 7];
    let replay = Replay::read(cut).unwrap();
    assert!(!replay.complete());
    let report = verify(
        &replay,
        &mut game(1),
        VerifyOptions::mode(ReplayMode::Verify),
    );
    assert_eq!(report.outcome, ReplayOutcome::Identical);
    assert!(report.ticks_run >= 28);
    assert_eq!(
        Replay::read(&bytes[..40]).err().unwrap().code,
        "replay.truncated"
    );
    let mut bad = bytes.clone();
    bad.extend_from_slice(&[0x7f, 0x00]);
    assert_eq!(Replay::read(&bad).err().unwrap().code, "replay.format");
}

/// V5 (the parts that need only persistence): another run configuration is refused by `Verify`
/// with the whole comparison, runs in `Compare`; a substituted bundle is refused by `Verify` and
/// flagged in `Compare`; `Rerun` runs and verifies nothing.
#[test]
fn v5_the_version_gate() {
    let bytes = record(&mut game(11), &inputs(), 40, opts(0));
    let replay = Replay::read(&bytes).unwrap();
    struct Other(Game);
    impl pocket_persist::Stepper for Other {
        fn world(&self) -> &bevy_ecs::world::World {
            self.0.world()
        }
        fn world_mut(&mut self) -> &mut bevy_ecs::world::World {
            self.0.world_mut()
        }
        fn registry(&self) -> &pocket_persist::Registry {
            self.0.registry()
        }
        fn restore(
            &mut self,
            s: &pocket_persist::Snapshot,
            b: &BundleRecord,
        ) -> Result<(), pocket_contract::Problem> {
            self.0.restore(s, b)
        }
        fn apply(
            &mut self,
            w: &pocket_persist::RecordedWrite,
            s: &dyn pocket_persist::ReplaySource,
        ) -> Result<(), pocket_contract::Problem> {
            self.0.apply(w, s)
        }
        fn step(&mut self) -> Result<Vec<pocket_sim::Event>, pocket_contract::Problem> {
            self.0.step()
        }
        fn run_config(&self) -> String {
            r#"{"steps_per_tick":5}"#.into()
        }
    }
    let report = verify(
        &replay,
        &mut Other(game(1)),
        VerifyOptions::mode(ReplayMode::Verify),
    );
    let ReplayOutcome::Stopped(_, p) = &report.outcome else {
        panic!()
    };
    assert_eq!(p.code, "version.mismatch");
    assert!(p.detail.contains_key("run_config"));
    let report = verify(
        &replay,
        &mut Other(game(1)),
        VerifyOptions::mode(ReplayMode::Compare),
    );
    assert_eq!(report.outcome, ReplayOutcome::Identical);

    let empty = BundleRecord::empty();
    let mut sub = VerifyOptions::mode(ReplayMode::Verify);
    let new = BundleRecord::new(vec![("main.ts".into(), b"export {}".to_vec())], None);
    sub.substitute.insert(empty.hash, new.clone());
    let report = verify(&replay, &mut game(1), sub.clone());
    assert!(
        matches!(&report.outcome, ReplayOutcome::Stopped(_, p) if p.code == "version.mismatch")
    );
    sub.mode = ReplayMode::Compare;
    let report = verify(&replay, &mut game(1), sub);
    assert_eq!(report.outcome, ReplayOutcome::Identical);
    assert!(report.versions.bundles.iter().any(|(h, u)| *h == empty.hash
        && *u == pocket_persist::version::BundleUse::Substituted { with: new.hash }));
    let report = verify(
        &replay,
        &mut game(1),
        VerifyOptions::mode(ReplayMode::Rerun),
    );
    assert_eq!(
        (report.outcome, report.verified),
        (ReplayOutcome::Identical, false)
    );
}

/// R4: a recording with `embed_data` verifies from the file alone (the stepper finds the data file
/// in the recording); without, it stops with `replay.data_unavailable` where the file is needed.
#[test]
fn r4_data() {
    let (path, hash, bytes) = common::content();
    let ins = vec![input(6, "load", json!({"hash": hash.to_hex()}))];
    let with = record(&mut game(12), &ins, 20, opts(0));
    let replay = Replay::read(&with).unwrap();
    assert_eq!(replay.data(&hash), Some(bytes));
    assert!(
        replay
            .records()
            .iter()
            .any(|r| matches!(r, Record::DataRead { path: p, hash: h } if p == path && *h == hash))
    );
    let report = verify(
        &replay,
        &mut game(1),
        VerifyOptions::mode(ReplayMode::Verify),
    );
    assert_eq!(report.outcome, ReplayOutcome::Identical);
    let without = record(
        &mut game(12),
        &ins,
        20,
        RecordOptions {
            embed_data: false,
            keyframe_every: 0,
            ..RecordOptions::default()
        },
    );
    let replay = Replay::read(&without).unwrap();
    assert!(replay.data(&hash).is_none());
    let report = verify(
        &replay,
        &mut game(1),
        VerifyOptions::mode(ReplayMode::Verify),
    );
    let ReplayOutcome::Stopped(where_, p) = report.outcome else {
        panic!("{:?}", report.outcome)
    };
    assert_eq!(
        (where_, p.code.as_str()),
        (at(0, 6), "replay.data_unavailable")
    );
}

/// The file sink writes what the memory sink holds.
#[test]
fn file_sink() {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("pocket-persist-file-sink.p3dreplay");
    let mut g = game(13);
    let sink = pocket_persist::replay::FileSink::create(&path).unwrap();
    let mut rec = Recorder::start(
        g.sim.world(),
        g.reg.clone(),
        &BundleRecord::empty(),
        "{}",
        opts(10),
        Box::new(sink),
    )
    .unwrap();
    run(&mut g, Some(&mut rec), &inputs(), 25);
    rec.formats_changed(g.sim.world()).unwrap();
    rec.finish(g.sim.world()).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let replay = Replay::read(&bytes).unwrap();
    assert_eq!(replay.formats().len(), 2);
    let report = verify(
        &replay,
        &mut game(1),
        VerifyOptions::mode(ReplayMode::Verify),
    );
    assert_eq!(
        (report.outcome, report.ticks_run),
        (ReplayOutcome::Identical, 25)
    );
}
