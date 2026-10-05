//! Replaying a recording (docs/spec/replay.md 2.4 and 3.4; versions.md 7): the stepper the runtime
//! implements, `verify` in its three modes, and `seek`.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_ecs::prelude::World;
use pocket_contract::{Problem, detail};
use pocket_sim::{ContentHash, Event, Tick};
use serde_json::{Value, json};

use super::divergence::{Divergence, DivergenceKind};
use super::reader::{Replay, SegmentEnd, TickRef};
use super::{BundleRecord, DigestLevel, RecordedWrite};
use crate::diff::{DEFAULT_LIMIT, diff_with};
use crate::error;
use crate::hash::{SectionKey, WorldHash};
use crate::migrate::Migrations;
use crate::registry::Registry;
use crate::save::{ProjectMigrator, Target, cache_rebuilds, migrate_with};
use crate::snapshot::{Snapshot, section_digests, snapshot, world_hash};
use crate::version::{
    BundleUse, EngineVersion, FormatEntry, FormatTable, Match, VersionComparison,
};

/// The game loop as replay code drives it; implemented by `pocket-runtime`, which owns the world,
/// the script host and the command application.
pub trait Stepper {
    fn world(&self) -> &World;
    /// The world, for rebuilding the caches a `Rerun` migration dropped (versions.md 6.3).
    fn world_mut(&mut self) -> &mut World;
    fn registry(&self) -> &Registry;
    /// Replaces the world with the snapshot's and loads `bundle` (at a start, a `Rebase` or a
    /// keyframe).
    fn restore(&mut self, snap: &Snapshot, bundle: &BundleRecord) -> Result<(), Problem>;
    /// Applies a recorded write through the live path (2.1), session-level checks skipped.
    fn apply(&mut self, write: &RecordedWrite, source: &dyn ReplaySource) -> Result<(), Problem>;
    /// One tick; its event record.
    fn step(&mut self) -> Result<Vec<Event>, Problem>;
    /// The run configuration as canonical JSON (versions.md 3.7), which `Verify` compares with the
    /// recording's (open choice 2 of replay.md).
    fn run_config(&self) -> String;
    /// A bundle the recording does not embed, found by hash (versions.md 7.4: the project's current
    /// scripts when their hash matches, or the content store). None by default.
    fn find_bundle(&self, h: &ContentHash) -> Option<BundleRecord> {
        let _ = h;
        None
    }
    /// One TypeScript migration step of a project component, for `Rerun` (versions.md 7.2 and
    /// 8.3): the runtime runs it with its script host. By default there are none:
    /// `migrate.missing_step`.
    fn migrate_project(
        &mut self,
        component: &str,
        from: u32,
        value: Value,
    ) -> Result<Value, Problem> {
        let _ = value;
        Err(error::missing_step(component, from))
    }
}

/// Where a stepper finds the bundles and data a write or a `DataRead` names.
pub trait ReplaySource {
    fn bundle(&self, h: &ContentHash) -> Option<BundleRecord>;
    fn data(&self, h: &ContentHash) -> Option<Arc<[u8]>>;
}

/// No bundles and no data (a live run, lockstep).
impl ReplaySource for () {
    fn bundle(&self, _: &ContentHash) -> Option<BundleRecord> {
        None
    }
    fn data(&self, _: &ContentHash) -> Option<Arc<[u8]>> {
        None
    }
}

/// A replay's records, with substitutes taking the place of the bundles they name.
struct Sources<'a> {
    replay: &'a Replay,
    substitute: &'a BTreeMap<ContentHash, BundleRecord>,
}

impl Sources<'_> {
    /// A bundle by hash: a substitute, the recording's own, or one the stepper finds (7.4).
    fn find(&self, s: &dyn Stepper, h: &ContentHash) -> Option<BundleRecord> {
        self.bundle(h).or_else(|| s.find_bundle(h))
    }
}

impl ReplaySource for Sources<'_> {
    fn bundle(&self, h: &ContentHash) -> Option<BundleRecord> {
        self.substitute
            .get(h)
            .or_else(|| self.replay.bundle(h))
            .cloned()
    }
    fn data(&self, h: &ContentHash) -> Option<Arc<[u8]>> {
        self.replay.data(h).map(Arc::from)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReplayMode {
    Verify,
    Compare,
    Rerun,
}

#[derive(Clone, Debug)]
pub struct VerifyOptions {
    pub mode: ReplayMode,
    /// Start at the keyframe at or before this tick.
    pub from: Option<TickRef>,
    /// Bundles run in place of the recorded ones (open choice 3 of replay.md: persistence's own
    /// `BundleRecord`, since `pocket-persist` cannot name the script host's `Bundle`).
    pub substitute: BTreeMap<ContentHash, BundleRecord>,
    /// Keep running after a divergence to find `reconverged_at`.
    pub continue_after_divergence: bool,
    /// The engine's steps, which `Rerun` runs on each snapshot it restores as a load does
    /// (versions.md 6.2); project components' steps come from [`Stepper::migrate_project`].
    pub migrations: Arc<Migrations>,
}

impl VerifyOptions {
    pub fn mode(mode: ReplayMode) -> VerifyOptions {
        VerifyOptions {
            mode,
            from: None,
            substitute: BTreeMap::new(),
            continue_after_divergence: false,
            migrations: Arc::new(Migrations::default()),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum ReplayOutcome {
    Identical,
    Diverged(Box<Divergence>),
    Stopped(TickRef, Problem),
}

#[derive(Clone, PartialEq, Debug)]
pub struct ReplayReport {
    pub ticks_run: u64,
    pub outcome: ReplayOutcome,
    pub versions: VersionComparison,
    /// False in `Rerun`, which compares nothing.
    pub verified: bool,
}

/// Sections with whether they match.
type Matches = Vec<(SectionKey, Match)>;

fn schemas(replay: &Replay, current: &FormatTable) -> (Matches, Matches) {
    let mut recorded: BTreeMap<SectionKey, &crate::version::FormatEntry> = BTreeMap::new();
    for t in replay.formats() {
        for e in &t.entries {
            recorded.insert(e.section.clone(), e);
        }
    }
    let (mut schemas, mut caches) = (Vec::new(), Vec::new());
    for (key, e) in recorded {
        let now = current.get(&key);
        match &e.identity {
            Some(id) => {
                let cur = now
                    .and_then(|n| n.identity.clone())
                    .unwrap_or_else(|| "absent".into());
                caches.push((key, Match::of(id, cur)));
            }
            None => {
                let show = |v: u32, f: &crate::format::Fingerprint| format!("v{v} {f}");
                let cur =
                    now.map_or_else(|| "absent".to_owned(), |n| show(n.version, &n.fingerprint));
                schemas.push((key, Match::of(show(e.version, &e.fingerprint), cur)));
            }
        }
    }
    (schemas, caches)
}

/// The recording's versions against the stepper's (versions.md 7.1).
pub fn compare_versions(
    replay: &Replay,
    s: &dyn Stepper,
    substitute: &BTreeMap<ContentHash, BundleRecord>,
) -> VersionComparison {
    let rec = &replay.header().engine;
    let cur = EngineVersion::current();
    let mut hashes: Vec<ContentHash> = replay.bundles().iter().map(|b| b.hash).collect();
    for seg in 0..replay.segments().len() {
        if let Some(snap) = replay.start(u32::try_from(seg).unwrap_or(u32::MAX)) {
            hashes.push(snap.header().bundle);
        }
    }
    hashes.sort();
    hashes.dedup();
    let bundles = hashes
        .into_iter()
        .map(|h| {
            let used = match substitute.get(&h) {
                Some(b) => BundleUse::Substituted { with: b.hash },
                None if replay.bundle(&h).is_some() => BundleUse::Same,
                None if s.find_bundle(&h).is_some() => BundleUse::Available,
                None => BundleUse::Unavailable,
            };
            (h, used)
        })
        .collect();
    let current = FormatTable::of(s.registry(), s.world()).unwrap_or_default();
    let (schemas, caches) = schemas(replay, &current);
    VersionComparison {
        format: Match::Same,
        engine: Match::of(rec.source, cur.source),
        contract: Match::of(&rec.contract, &cur.contract),
        run_config: Match::of(&replay.header().run_config, s.run_config()),
        bundles,
        schemas,
        caches,
        data: Vec::new(),
    }
}

/// The bundle a snapshot names: a substitute, the recording's, or the stepper's (7.4).
fn bundle_for(
    src: &Sources<'_>,
    s: &dyn Stepper,
    snap: &Snapshot,
) -> Result<BundleRecord, Problem> {
    let h = snap.header().bundle;
    src.find(s, &h).ok_or_else(|| {
        error::bundle_unavailable(
            &h.to_hex(),
            "neither embedded, substituted nor found by the stepper",
        )
    })
}

/// The recorded formats of a snapshot's sections: for each, the entry of a `Formats` record with
/// its version and fingerprint.
fn recorded_formats(replay: &Replay, snap: &Snapshot) -> FormatTable {
    let tables = replay.formats();
    let entries: Vec<FormatEntry> = snap
        .sections()
        .iter()
        .filter_map(|sec| {
            tables
                .iter()
                .rev()
                .filter_map(|t| t.get(&sec.key))
                .find(|e| e.version == sec.version && e.fingerprint == sec.fingerprint)
                .cloned()
        })
        .collect();
    FormatTable { entries }
}

/// Runs a project component's steps through the stepper.
struct ByStepper<'a>(&'a mut dyn Stepper);

impl ProjectMigrator for ByStepper<'_> {
    fn migrate(&mut self, component: &str, from: u32, value: Value) -> Result<Value, Problem> {
        self.0.migrate_project(component, from, value)
    }
}

/// Restores a recorded snapshot into the stepper with its bundle. In `Rerun` the snapshot first
/// goes through the steps of a load (versions.md 6.2) with the recording's formats, and the
/// caches whose identity differs are rebuilt after the restore (6.3).
fn load(
    replay: &Replay,
    s: &mut dyn Stepper,
    src: &Sources<'_>,
    snap: &Snapshot,
    opts: &VerifyOptions,
) -> Result<(), Problem> {
    let bundle = bundle_for(src, &*s, snap)?;
    if opts.mode != ReplayMode::Rerun {
        return s.restore(snap, &bundle);
    }
    let table = recorded_formats(replay, snap);
    let current = FormatTable::of(s.registry(), s.world())?;
    let engine: Vec<SectionKey> = s
        .registry()
        .entries()
        .iter()
        .map(|e| e.key.clone())
        .collect();
    let target = Target {
        current: &current,
        engine: &|k| engine.binary_search(k).is_ok(),
        bundle: snap.header().bundle,
    };
    let (migrated, report) =
        migrate_with(snap, &table, &target, &opts.migrations, &mut ByStepper(s))?;
    s.restore(&migrated, &bundle)?;
    for rebuild in cache_rebuilds(s.registry(), &report.caches_rebuilt) {
        rebuild(s.world_mut())?;
    }
    Ok(())
}

/// `replay.out_of_range` unless `from` names a tick of a segment of the replay (replay.md 4).
fn check_from(replay: &Replay, from: TickRef) -> Result<(), Problem> {
    let segments = replay.segments();
    let Some(info) = segments.get(from.segment as usize) else {
        let last = segments.last().map_or(0, |i| i.last.0);
        return Err(error::out_of_range(from.segment, from.tick.0, 0, last));
    };
    let (first, last) = (info.start.0, info.last);
    if from.tick < first || from.tick > last {
        return Err(error::out_of_range(
            from.segment,
            from.tick.0,
            first.0,
            last.0,
        ));
    }
    Ok(())
}

/// What a divergence at `at` can name: sections from the recorded digests, fields from a keyframe.
fn locate(replay: &Replay, s: &dyn Stepper, at: TickRef, d: &mut Divergence) {
    if replay.header().digests == DigestLevel::Sections
        && let (Some(rec), Ok(live)) =
            (replay.digests(at), section_digests(s.world(), s.registry()))
    {
        let mut keys: Vec<SectionKey> = live
            .iter()
            .filter(|(k, dg)| {
                rec.iter()
                    .find(|(r, _)| r == k)
                    .is_none_or(|(_, rd)| rd != dg)
            })
            .map(|(k, _)| k.clone())
            .collect();
        keys.extend(
            rec.iter()
                .filter(|(k, _)| !live.iter().any(|(l, _)| l == k))
                .map(|(k, _)| k.clone()),
        );
        keys.sort();
        d.sections = keys;
    }
    if let (Some(kf), Ok(now)) = (replay.keyframe_at(at), snapshot(s.world(), s.registry()))
        && let Ok(table) = FormatTable::of(s.registry(), s.world())
        && let Ok(found) = diff_with(kf, &now, &table, DEFAULT_LIMIT)
    {
        d.sections = found.keys();
        d.fields = found.fields;
        d.fields_truncated = found.truncated;
    }
}

enum Step {
    Go,
    Done(ReplayOutcome),
}

/// Re-runs a recording (3.4): restores each segment's start (or the keyframe at `from`), applies
/// the recorded writes through the live path, steps, hashes and compares, as the mode says.
pub fn verify(replay: &Replay, s: &mut dyn Stepper, opts: VerifyOptions) -> ReplayReport {
    let versions = compare_versions(replay, &*s, &opts.substitute);
    let mut report = ReplayReport {
        ticks_run: 0,
        outcome: ReplayOutcome::Identical,
        verified: opts.mode != ReplayMode::Rerun,
        versions,
    };
    let start = TickRef {
        segment: 0,
        tick: replay.header().start_tick,
    };
    if let Some(from) = opts.from
        && let Err(p) = check_from(replay, from)
    {
        report.outcome = ReplayOutcome::Stopped(from, p);
        report.verified = false;
        return report;
    }
    let gate = match opts.mode {
        ReplayMode::Verify => {
            let differ = report.versions.verify_differences();
            (!differ.is_empty()).then(|| error::version_mismatch(report.versions.detail(), &differ))
        }
        ReplayMode::Compare => {
            let differ = report.versions.schema_differences();
            (!differ.is_empty()).then(|| error::schema_differs(&differ))
        }
        ReplayMode::Rerun => None,
    };
    if let Some(p) = gate {
        report.outcome = ReplayOutcome::Stopped(start, p);
        return report;
    }
    let src = Sources {
        replay,
        substitute: &opts.substitute,
    };
    let compare = opts.mode != ReplayMode::Rerun;
    let mut found: Option<Divergence> = None;
    let first = opts.from.map_or(0, |f| f.segment);
    for seg in replay.segments().into_iter().skip(first as usize) {
        match run_segment(
            replay,
            s,
            &src,
            seg.index,
            &opts,
            compare,
            &mut found,
            &mut report.ticks_run,
        ) {
            Step::Go => {}
            Step::Done(outcome) => {
                report.outcome = outcome;
                return report;
            }
        }
        if found.as_ref().is_some_and(|d| d.reconverged_at.is_some()) {
            break;
        }
    }
    if let Some(d) = found {
        report.outcome = ReplayOutcome::Diverged(Box::new(d));
    }
    report
}

/// A recorded write the stepper did not apply: `replay.write_refused`, unless the stepper could not
/// find what the write names (`replay.data_unavailable`, `replay.bundle_unavailable`), which stops
/// the run as itself.
fn refusal(segment: u32, tick: u64, w: &RecordedWrite, e: Problem) -> Problem {
    if e.is("replay.data_unavailable") || e.is("replay.bundle_unavailable") {
        e
    } else {
        error::write_refused(segment, tick, w.to_json(), &e)
    }
}

/// Compares the stepper's hash after `at` with the recorded one: records the first divergence, or
/// the tick where a divergence reconverged. `true` when the run should stop.
fn observe(
    replay: &Replay,
    s: &dyn Stepper,
    opts: &VerifyOptions,
    compare: bool,
    found: &mut Option<Divergence>,
    (at, expected, writes): (TickRef, WorldHash, &[RecordedWrite]),
) -> Result<bool, Problem> {
    if !compare {
        return Ok(false);
    }
    let actual = world_hash(s.world(), s.registry())?;
    match found {
        Some(d) => {
            if actual == expected && d.reconverged_at.is_none() {
                d.reconverged_at = Some(at);
                return Ok(true);
            }
            Ok(false)
        }
        None if actual != expected => {
            let mut d = Divergence::new(at, DivergenceKind::Hash);
            d.expected = Some(expected);
            d.actual = Some(actual);
            d.writes = writes.to_vec();
            locate(replay, s, at, &mut d);
            *found = Some(d);
            Ok(!opts.continue_after_divergence)
        }
        None => Ok(false),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_segment(
    replay: &Replay,
    s: &mut dyn Stepper,
    src: &Sources<'_>,
    segment: u32,
    opts: &VerifyOptions,
    compare: bool,
    found: &mut Option<Divergence>,
    ticks_run: &mut u64,
) -> Step {
    let Some(info) = replay.segments().into_iter().nth(segment as usize) else {
        return Step::Go;
    };
    let from = opts.from.filter(|f| f.segment == segment);
    let snap = match from {
        Some(f) => replay.keyframe_at_or_before(f),
        None => replay.start(segment),
    };
    let Some(snap) = snap else {
        return Step::Go;
    };
    let at0 = TickRef {
        segment,
        tick: snap.header().tick,
    };
    if let Err(e) = load(replay, s, src, snap, opts) {
        return Step::Done(ReplayOutcome::Stopped(at0, e));
    }
    match observe(
        replay,
        &*s,
        opts,
        compare,
        found,
        (at0, snap.world_hash(), &[]),
    ) {
        Ok(true) => {
            return Step::Done(ReplayOutcome::Diverged(Box::new(
                found
                    .take()
                    .unwrap_or_else(|| Divergence::new(at0, DivergenceKind::Hash)),
            )));
        }
        Ok(false) => {}
        Err(e) => return Step::Done(ReplayOutcome::Stopped(at0, e)),
    }
    let ticks = replay.ticks(segment);
    for t in ticks.start().0..=ticks.end().0 {
        let at = TickRef {
            segment,
            tick: Tick(t),
        };
        if t <= at0.tick.0 {
            continue;
        }
        if replay.tainted() == Some(at) {
            let why = "a debugger evaluated inside the tick";
            return Step::Done(ReplayOutcome::Stopped(at, error::tainted(segment, t, why)));
        }
        let writes = replay.writes(at);
        for w in writes {
            if let Err(e) = s.apply(w, src) {
                return Step::Done(ReplayOutcome::Stopped(at, refusal(segment, t, w, e)));
            }
        }
        if let Err(e) = s.step() {
            return Step::Done(ReplayOutcome::Stopped(at, e));
        }
        *ticks_run += 1;
        let Some(expected) = replay.hash(at) else {
            continue;
        };
        match observe(replay, &*s, opts, compare, found, (at, expected, writes)) {
            Ok(true) => {
                return match found.take() {
                    Some(d) => Step::Done(ReplayOutcome::Diverged(Box::new(d))),
                    None => Step::Go,
                };
            }
            Ok(false) => {}
            Err(e) => return Step::Done(ReplayOutcome::Stopped(at, e)),
        }
    }
    match &info.end {
        SegmentEnd::Fault { tick, code } => {
            let at = TickRef {
                segment,
                tick: *tick,
            };
            let p = Problem::new(
                code,
                format!(
                    "The recording's run faulted with {code} at tick {}; replay stops there.",
                    tick.0
                ),
                detail([("segment", json!(segment)), ("tick", json!(tick.0))]),
            );
            Step::Done(ReplayOutcome::Stopped(at, p))
        }
        SegmentEnd::Open => Step::Go,
        SegmentEnd::End => {
            let Some((tick, writes, world)) = replay.end(segment) else {
                return Step::Go;
            };
            let at = TickRef { segment, tick };
            for w in writes {
                if let Err(e) = s.apply(w, src) {
                    return Step::Done(ReplayOutcome::Stopped(at, refusal(segment, tick.0, w, e)));
                }
            }
            match observe(replay, &*s, opts, compare, found, (at, world, writes)) {
                Ok(true) => found.take().map_or(Step::Go, |d| {
                    Step::Done(ReplayOutcome::Diverged(Box::new(d)))
                }),
                Ok(false) => Step::Go,
                Err(e) => Step::Done(ReplayOutcome::Stopped(at, e)),
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeekScripts {
    /// The session's current bundle, loaded by the runtime after the seek returns (versions.md
    /// 7.5).
    Current,
    Recorded,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SeekReport {
    pub at: TickRef,
    pub world: WorldHash,
    pub resimulated: u64,
    /// Whether a bundle was loaded after `at` in the recording.
    pub scripts_changed_since: bool,
}

/// `replay.diverged`: a seek re-simulated a tick whose hash differs from the recorded one.
fn seek_diverged(at: TickRef) -> Problem {
    Problem::new(
        "replay.diverged",
        format!(
            "Re-simulating tick {} of segment {} gave another hash than the recording's.",
            at.tick.0, at.segment
        ),
        detail([("segment", json!(at.segment)), ("tick", json!(at.tick.0))]),
    )
}

/// Restores the keyframe at or before `at` and re-runs the recorded writes and ticks to it with
/// the recorded bundles, checking each tick hash. Loading the current scripts afterwards
/// (`SeekScripts::Current`) is the runtime's, which owns the script host.
pub fn seek(
    replay: &Replay,
    s: &mut dyn Stepper,
    at: TickRef,
    scripts: SeekScripts,
) -> Result<SeekReport, Problem> {
    let _ = scripts;
    let infos = replay.segments();
    let info = infos
        .get(at.segment as usize)
        .ok_or_else(|| error::out_of_range(at.segment, at.tick.0, 0, 0))?;
    let first = info.start.0;
    if at.tick < first || at.tick > info.last {
        return Err(error::out_of_range(
            at.segment,
            at.tick.0,
            first.0,
            info.last.0,
        ));
    }
    let snap = replay
        .keyframe_at_or_before(at)
        .ok_or_else(|| error::out_of_range(at.segment, at.tick.0, first.0, info.last.0))?;
    let src = Sources {
        replay,
        substitute: &BTreeMap::new(),
    };
    let bundle = bundle_for(&src, &*s, snap)?;
    s.restore(snap, &bundle)?;
    let mut resimulated = 0;
    for t in snap.header().tick.0 + 1..=at.tick.0 {
        let here = TickRef {
            segment: at.segment,
            tick: Tick(t),
        };
        for w in replay.writes(here) {
            s.apply(w, &src).map_err(|e| refusal(at.segment, t, w, e))?;
        }
        s.step()?;
        resimulated += 1;
        if replay.hash(here) != Some(world_hash(s.world(), s.registry())?) {
            return Err(seek_diverged(here));
        }
    }
    let world = world_hash(s.world(), s.registry())?;
    let seg = replay.segment(at.segment);
    let after = seg
        .and_then(|sg| {
            sg.ticks
                .iter()
                .find(|e| e.tick == at.tick)
                .map(|e| e.record)
        })
        .unwrap_or(0);
    let scripts_changed_since = replay.records()[after + 1..]
        .iter()
        .any(|r| matches!(r, super::Record::Bundle(_)));
    Ok(SeekReport {
        at,
        world,
        resimulated,
        scripts_changed_since,
    })
}
