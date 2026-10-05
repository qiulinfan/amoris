//! The recorder (docs/spec/replay.md 2.3): writes the start, every applied write under the tick it
//! is an input of, each tick's hash and section digests, keyframes, data and bundles, and the
//! segments a restore or a reset opens (2.5).

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use bevy_ecs::prelude::World;
use pocket_contract::Problem;
use pocket_sim::{ContentHash, SimClock, Tick, WorldSeed};

use super::{
    Applied, BundleRecord, DigestDelta, DigestLevel, LabelMap, ParentRef, RebaseCause, Record,
    RecordedWrite, ReplayHeader, file_head,
};
use crate::error;
use crate::hash::{SectionDigest, SectionKey, TickHash, world_hash_of};
use crate::registry::Registry;
use crate::snapshot::{Snapshot, section_digests, snapshot};
use crate::version::{EngineVersion, FormatTable};

/// How a recording is made.
#[derive(Clone, Debug)]
pub struct RecordOptions {
    /// Default 600 (persistence.md 13); 0: none.
    pub keyframe_every: u32,
    /// Default `Sections`.
    pub digests: DigestLevel,
    /// Default true: the bundles' TypeScript sources (versions.md 7.4).
    pub embed_bundles: bool,
    /// Default true: the compiled modules beside them.
    pub embed_compiled: bool,
    /// Default true: the bytes of every data file named.
    pub embed_data: bool,
    pub label: LabelMap,
    pub parent: Option<ParentRef>,
}

impl Default for RecordOptions {
    fn default() -> Self {
        RecordOptions {
            keyframe_every: 600,
            digests: DigestLevel::Sections,
            embed_bundles: true,
            embed_compiled: true,
            embed_data: true,
            label: LabelMap::new(),
            parent: None,
        }
    }
}

/// Where records go.
pub trait RecordSink: Send {
    fn write(&mut self, record: &[u8]) -> std::io::Result<()>;
    fn flush(&mut self) -> std::io::Result<()>;
}

/// Records in memory (the web, checks, a session's history). Clones share the bytes, so a caller
/// keeps one to read what was recorded.
#[derive(Clone, Default)]
pub struct MemorySink(Arc<Mutex<Vec<u8>>>);

impl MemorySink {
    pub fn new() -> MemorySink {
        MemorySink::default()
    }

    /// A copy of everything written so far.
    pub fn bytes(&self) -> Vec<u8> {
        self.0.lock().map(|b| b.clone()).unwrap_or_default()
    }
}

impl RecordSink for MemorySink {
    fn write(&mut self, record: &[u8]) -> std::io::Result<()> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("a poisoned memory sink"))?
            .extend_from_slice(record);
        Ok(())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Records to a file (native).
pub struct FileSink(BufWriter<File>);

impl FileSink {
    pub fn create(path: &Path) -> std::io::Result<FileSink> {
        Ok(FileSink(BufWriter::new(File::create(path)?)))
    }
}

impl RecordSink for FileSink {
    fn write(&mut self, record: &[u8]) -> std::io::Result<()> {
        self.0.write_all(record)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

/// What a finished recording holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReplaySummary {
    pub segments: u32,
    pub ticks: u64,
    pub bytes: u64,
    pub last: Tick,
}

/// Records one run. Every call happens on the game thread at a boundary, in the order things
/// happen. A record that cannot be written (a sink failure, a record that does not encode) breaks
/// the recording: nothing more is written, so the file stays a readable prefix, and every later
/// call that returns a result returns that problem (replay.md 6, choice 4).
pub struct Recorder {
    reg: Arc<Registry>,
    sink: Box<dyn RecordSink>,
    opts: RecordOptions,
    /// The current segment's start tick.
    t0: Tick,
    last: Tick,
    pending: Vec<RecordedWrite>,
    digests: Vec<(SectionKey, SectionDigest)>,
    bundles: BTreeSet<ContentHash>,
    /// The bundle last loaded, stamped on snapshots whose world names none.
    current: ContentHash,
    data: BTreeSet<ContentHash>,
    open: bool,
    tainted: bool,
    /// The first record that could not be written.
    failed: Option<Problem>,
    segments: u32,
    ticks: u64,
    bytes: u64,
    buf: Vec<u8>,
}

fn io(e: &std::io::Error) -> Problem {
    error::encode("replay", None, &format!("the sink failed: {e}"))
}

impl Recorder {
    /// Writes the header, the `Start` snapshot of `world`, its formats and its bundle.
    pub fn start(
        world: &World,
        reg: Arc<Registry>,
        bundle: &BundleRecord,
        run_config: &str,
        opts: RecordOptions,
        sink: Box<dyn RecordSink>,
    ) -> Result<Recorder, Problem> {
        let snap = snapshot(world, &reg)?;
        let clock = world.get_resource::<SimClock>().copied();
        let header = ReplayHeader {
            engine: EngineVersion::current().clone(),
            start_tick: snap.header().tick,
            seed: world.get_resource::<WorldSeed>().map_or(0, |s| s.0),
            tick_rate: clock.map_or(0, |c| c.rate.0),
            run_config: run_config.to_owned(),
            keyframe_every: opts.keyframe_every,
            digests: opts.digests,
            label: opts.label.clone(),
            parent: opts.parent,
        };
        let mut r = Recorder {
            t0: snap.header().tick,
            last: snap.header().tick,
            digests: snap.digests(),
            reg,
            sink,
            opts,
            pending: Vec::new(),
            bundles: BTreeSet::new(),
            current: bundle.hash,
            data: BTreeSet::new(),
            open: true,
            tainted: false,
            failed: None,
            segments: 1,
            ticks: 0,
            bytes: 0,
            buf: Vec::new(),
        };
        let head = file_head(&header)?;
        r.raw(&head)?;
        let snapshot = r.stamp(snap);
        r.put(&Record::Start { snapshot })?;
        let table = FormatTable::of(&r.reg, world)?;
        r.put(&Record::Formats { table })?;
        r.bundle_loaded(bundle);
        r.flush()?;
        Ok(r)
    }

    /// A snapshot with the recorder's bundle when its world's `SnapshotContext` names none.
    fn stamp(&self, snap: Snapshot) -> Snapshot {
        if snap.header().bundle == ContentHash([0; 32]) {
            snap.with_bundle(self.current)
        } else {
            snap
        }
    }

    /// The problem that broke the recording, if any.
    fn broken(&self) -> Result<(), Problem> {
        match &self.failed {
            Some(p) => Err(p.clone()),
            None => Ok(()),
        }
    }

    /// Keeps the first failure: the recording is broken from there.
    fn fail<T>(&mut self, r: Result<T, Problem>) -> Result<T, Problem> {
        if let Err(p) = &r
            && self.failed.is_none()
        {
            self.failed = Some(p.clone());
        }
        r
    }

    fn raw(&mut self, bytes: &[u8]) -> Result<(), Problem> {
        self.broken()?;
        self.bytes += bytes.len() as u64;
        let r = self.sink.write(bytes).map_err(|e| io(&e));
        self.fail(r)
    }

    fn put(&mut self, record: &Record) -> Result<(), Problem> {
        self.broken()?;
        let mut buf = std::mem::take(&mut self.buf);
        buf.clear();
        let r = match record.frame(&mut buf) {
            Ok(()) => self.raw(&buf),
            Err(e) => {
                let p = error::encode("replay", None, &e.reason());
                self.fail(Err(p))
            }
        };
        self.buf = buf;
        r
    }

    /// Recording never stops the game: a failure is kept, and the next call that returns a result
    /// returns it.
    fn put_quiet(&mut self, record: &Record) {
        let _ = self.put(record);
    }

    fn flush(&mut self) -> Result<(), Problem> {
        self.broken()?;
        let r = self.sink.flush().map_err(|e| io(&e));
        self.fail(r)
    }

    /// A write that succeeded, in application order (threads.md 5.3).
    pub fn applied(&mut self, write: &Applied) {
        self.pending.push(RecordedWrite::of(write));
    }

    /// A refused command, kept as a note and never replayed.
    pub fn refused(&mut self, write: &Applied, code: &str) {
        self.put_quiet(&Record::Refused {
            write: RecordedWrite::of(write),
            code: code.to_owned(),
        });
    }

    /// A simulation data file the game thread read (versions.md 3.3).
    pub fn data_read(&mut self, path: &str, hash: ContentHash, bytes: &[u8]) {
        self.data(hash, bytes);
        self.put_quiet(&Record::DataRead {
            path: path.to_owned(),
            hash,
        });
    }

    /// Bytes a later write names by hash (an import's result), once per hash.
    pub fn data(&mut self, hash: ContentHash, bytes: &[u8]) {
        if self.opts.embed_data && self.data.insert(hash) {
            self.put_quiet(&Record::Data {
                hash,
                bytes: bytes.to_vec(),
            });
        }
    }

    /// A bundle about to be loaded (before the write that swaps it), once per hash.
    pub fn bundle_loaded(&mut self, bundle: &BundleRecord) {
        self.current = bundle.hash;
        if !self.opts.embed_bundles || !self.bundles.insert(bundle.hash) {
            return;
        }
        let mut b = bundle.clone();
        if !self.opts.embed_compiled {
            b.compiled = None;
        }
        self.put_quiet(&Record::Bundle(b));
    }

    /// Ends the tick the world just completed: its writes, its hash and its section digests; a
    /// keyframe when the tick is a multiple of `keyframe_every` from the segment's start.
    /// The tick must follow the last one recorded in an open segment: `replay.segment_closed` after
    /// `end_segment` or a fault until `rebase`, `replay.tick_sequence` when the world was replaced
    /// without them; neither writes anything.
    pub fn end_tick(&mut self, world: &World) -> Result<TickHash, Problem> {
        self.broken()?;
        let tick = world.get_resource::<SimClock>().map_or(Tick(0), |c| c.tick);
        if !self.open {
            return Err(error::segment_closed(tick.0));
        }
        if self.last.0.checked_add(1) != Some(tick.0) {
            return Err(error::tick_sequence(self.last.0.saturating_add(1), tick.0));
        }
        let digests = section_digests(world, &self.reg)?;
        let hash = world_hash_of(digests.iter().map(|(_, d)| d));
        let delta = match self.opts.digests {
            DigestLevel::Sections => DigestDelta::between(&self.digests, &digests),
            DigestLevel::World => DigestDelta::default(),
        };
        self.digests = digests;
        let writes = std::mem::take(&mut self.pending);
        self.put(&Record::Tick {
            tick,
            writes,
            hash,
            digests: delta,
        })?;
        self.last = tick;
        self.ticks += 1;
        let every = u64::from(self.opts.keyframe_every);
        if every > 0
            && tick
                .0
                .checked_sub(self.t0.0)
                .is_some_and(|n| n.is_multiple_of(every))
        {
            let snapshot = self.stamp(snapshot(world, &self.reg)?);
            self.put(&Record::Keyframe { snapshot })?;
            self.flush()?;
        }
        Ok(hash)
    }

    /// Ends the open segment with the world as it is (before a restore or a reset replaces it):
    /// the writes applied at the last boundary and the hash after them.
    pub fn end_segment(&mut self, world: &World) -> Result<(), Problem> {
        self.broken()?;
        if !self.open {
            return Ok(());
        }
        let tick = world.get_resource::<SimClock>().map_or(Tick(0), |c| c.tick);
        let digests = section_digests(world, &self.reg)?;
        let writes = std::mem::take(&mut self.pending);
        self.put(&Record::End {
            tick,
            writes,
            world: world_hash_of(digests.iter().map(|(_, d)| d)),
        })?;
        self.open = false;
        self.flush()
    }

    /// Opens the next segment with the world a restore or a reset made (2.5): `Rebase` with its
    /// full snapshot, its formats, and its bundle when not recorded before. The open segment must
    /// have been ended with [`Recorder::end_segment`] (or by a fault) first: `replay.segment_open`.
    pub fn rebase(
        &mut self,
        world: &World,
        cause: RebaseCause,
        bundle: &BundleRecord,
    ) -> Result<(), Problem> {
        self.broken()?;
        if self.open {
            return Err(error::segment_open(self.last.0));
        }
        self.current = bundle.hash;
        let snap = self.stamp(snapshot(world, &self.reg)?);
        self.t0 = snap.header().tick;
        self.last = self.t0;
        self.digests = snap.digests();
        self.pending.clear();
        self.put(&Record::Rebase {
            cause,
            snapshot: snap,
        })?;
        let table = FormatTable::of(&self.reg, world)?;
        self.put(&Record::Formats { table })?;
        self.bundle_loaded(bundle);
        self.open = true;
        self.segments += 1;
        self.flush()
    }

    /// The world's formats changed (a hot update registered a project component): a `Formats`
    /// record with the new table (2.2).
    pub fn formats_changed(&mut self, world: &World) -> Result<(), Problem> {
        self.broken()?;
        let table = FormatTable::of(&self.reg, world)?;
        self.put(&Record::Formats { table })
    }

    /// A fault poisoned the world in `tick`: the record replaces that tick's and ends the segment.
    pub fn fault(&mut self, tick: Tick, system: &str, code: &str) {
        self.pending.clear();
        self.put_quiet(&Record::Fault {
            tick,
            system: system.to_owned(),
            code: code.to_owned(),
        });
        self.open = false;
        let _ = self.flush();
    }

    /// A debugger evaluated inside `tick` (script-host.md 13); recorded once.
    pub fn tainted(&mut self, tick: Tick, reason: &str) {
        if !self.tainted {
            self.tainted = true;
            self.put_quiet(&Record::Tainted {
                tick,
                reason: reason.to_owned(),
            });
        }
    }

    /// Ends the open segment and flushes.
    pub fn finish(mut self, world: &World) -> Result<ReplaySummary, Problem> {
        self.broken()?;
        self.end_segment(world)?;
        self.flush()?;
        Ok(ReplaySummary {
            segments: self.segments,
            ticks: self.ticks,
            bytes: self.bytes,
            last: self.last,
        })
    }
}
