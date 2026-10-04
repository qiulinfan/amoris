//! Reading a replay (docs/spec/replay.md 2.2 and 2.4): records in order, grouped into segments,
//! with each tick's hash, writes and section digests rebuilt from the deltas.

use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use pocket_contract::Problem;
use pocket_sim::{ContentHash, Tick};
use serde::{Deserialize, Serialize};

use super::{
    BundleRecord, DigestLevel, REPLAY_FORMAT, REPLAY_MAGIC, RebaseCause, Record, RecordedWrite,
    ReplayHeader, file_head,
};
use crate::error;
use crate::hash::{SectionDigest, SectionKey, TickHash, WorldHash};
use crate::pce::{self, Decoder, PceError};
use crate::snapshot::Snapshot;
use crate::version::FormatTable;

/// A tick of one segment: ticks restart, and may repeat, after a `Rebase`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct TickRef {
    pub segment: u32,
    pub tick: Tick,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SegmentEnd {
    End,
    Fault {
        tick: Tick,
        code: String,
    },
    /// Cut short.
    Open,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SegmentInfo {
    pub index: u32,
    /// `None` for segment 0.
    pub cause: Option<RebaseCause>,
    /// The start point `(tick, writes)`.
    pub start: (Tick, u32),
    pub last: Tick,
    pub end: SegmentEnd,
}

/// One tick's record, its digests rebuilt.
pub(crate) struct TickEntry {
    pub tick: Tick,
    pub record: usize,
    pub hash: TickHash,
    /// The running section list after the tick (`Sections` recordings only).
    pub digests: Option<Vec<(SectionKey, SectionDigest)>>,
}

pub(crate) struct Segment {
    pub info: SegmentInfo,
    /// The record that opened it (`Start` or `Rebase`) and the one past its last.
    pub first: usize,
    pub past: usize,
    pub ticks: Vec<TickEntry>,
    pub keyframes: Vec<(Tick, usize)>,
    /// The `End` record.
    pub end: Option<usize>,
}

/// A read replay.
pub struct Replay {
    header: ReplayHeader,
    records: Vec<Record>,
    complete: bool,
    pub(crate) segments: Vec<Segment>,
    bundles: BTreeMap<ContentHash, usize>,
    data: BTreeMap<ContentHash, usize>,
    tainted: Option<TickRef>,
}

fn snapshot_of(r: &Record) -> Option<&Snapshot> {
    match r {
        Record::Start { snapshot }
        | Record::Rebase { snapshot, .. }
        | Record::Keyframe { snapshot } => Some(snapshot),
        _ => None,
    }
}

impl Replay {
    /// Parses a replay: `replay.format` for bad magic, format or a record tag, `replay.truncated`
    /// when not even `Start` is whole; a shorter cut reads up to the last whole record and sets
    /// `complete` false.
    pub fn read(bytes: &[u8]) -> Result<Replay, Problem> {
        if bytes.len() < 12 || &bytes[..8] != REPLAY_MAGIC {
            return Err(error::replay_format(0, None, "not a replay (bad magic)"));
        }
        let format = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        if format != REPLAY_FORMAT {
            return Err(error::version_format("replay", format, REPLAY_FORMAT));
        }
        let mut d = Decoder::new(bytes, false);
        let head = (|| {
            d.take(12)?;
            let n = usize::try_from(d.uleb()?).unwrap_or(usize::MAX);
            let h = d.take(n)?;
            pce::from_bytes::<ReplayHeader>(h, false)
        })();
        let header = match head {
            Ok(h) => h,
            Err(PceError::Truncated { offset }) => return Err(error::replay_truncated(offset)),
            Err(e) => return Err(error::replay_format(d.pos(), None, &e.reason())),
        };
        let mut records = Vec::new();
        let mut complete = true;
        while !d.at_end() {
            let at = d.pos();
            let framed = (|| {
                let tag: u8 = d.value()?;
                let n = usize::try_from(d.uleb()?).unwrap_or(usize::MAX);
                Ok::<_, PceError>((tag, d.take(n)?))
            })();
            let (tag, payload) = match framed {
                Ok(x) => x,
                Err(PceError::Truncated { .. }) => {
                    complete = false;
                    break;
                }
                Err(e) => return Err(error::replay_format(at, None, &e.reason())),
            };
            match Record::decode(tag, payload) {
                None => return Err(error::replay_format(at, Some(tag), "an unknown record tag")),
                Some(Err(e)) => return Err(error::replay_format(at, Some(tag), &e.reason())),
                Some(Ok(r)) => records.push(r),
            }
        }
        if !matches!(records.first(), Some(Record::Start { .. })) {
            return Err(error::replay_truncated(d.pos()));
        }
        Replay::from_records(header, records, complete)
    }

    /// A replay from its records (to write an edited copy, or to index a recording made in
    /// memory). The first record must be `Start`.
    pub fn from_records(
        header: ReplayHeader,
        records: Vec<Record>,
        complete: bool,
    ) -> Result<Replay, Problem> {
        let mut r = Replay {
            header,
            records,
            complete,
            segments: Vec::new(),
            bundles: BTreeMap::new(),
            data: BTreeMap::new(),
            tainted: None,
        };
        r.index()?;
        Ok(r)
    }

    fn index(&mut self) -> Result<(), Problem> {
        let sections = self.header.digests == DigestLevel::Sections;
        let mut running: Vec<(SectionKey, SectionDigest)> = Vec::new();
        for (i, rec) in self.records.iter().enumerate() {
            let fail =
                |why: &str| error::replay_format(0, Some(rec.tag()), &format!("record {i}: {why}"));
            match rec {
                Record::Start { snapshot } | Record::Rebase { snapshot, .. } => {
                    if matches!(rec, Record::Start { .. }) != (i == 0) {
                        return Err(fail("Start must come first and only once"));
                    }
                    if let Some(s) = self.segments.last_mut() {
                        if s.info.end == SegmentEnd::Open && s.end.is_none() {
                            return Err(fail("a Rebase while the previous segment is open"));
                        }
                        s.past = i;
                    }
                    let h = snapshot.header();
                    running = snapshot.digests();
                    self.segments.push(Segment {
                        info: SegmentInfo {
                            index: u32::try_from(self.segments.len()).unwrap_or(u32::MAX),
                            cause: match rec {
                                Record::Rebase { cause, .. } => Some(*cause),
                                _ => None,
                            },
                            start: (h.tick, h.writes),
                            last: h.tick,
                            end: SegmentEnd::Open,
                        },
                        first: i,
                        past: self.records.len(),
                        ticks: Vec::new(),
                        keyframes: Vec::new(),
                        end: None,
                    });
                }
                _ if self.segments.is_empty() => return Err(fail("a record before Start")),
                Record::Tick {
                    tick,
                    hash,
                    digests,
                    ..
                } => {
                    let seg = self.segments.last_mut().ok_or_else(|| fail("no segment"))?;
                    if seg.info.end != SegmentEnd::Open || seg.end.is_some() {
                        return Err(fail("a Tick after its segment ended"));
                    }
                    if tick.0 != seg.info.last.0 + 1 {
                        return Err(fail(&format!(
                            "tick {} follows tick {}",
                            tick.0, seg.info.last.0
                        )));
                    }
                    let list = if sections {
                        digests.apply(&mut running).map_err(|w| fail(&w))?;
                        let ok =
                            crate::hash::world_hash_of(running.iter().map(|(_, d)| d)) == *hash;
                        if !ok {
                            return Err(fail("the section digests do not hash to the tick hash"));
                        }
                        Some(running.clone())
                    } else {
                        None
                    };
                    seg.info.last = *tick;
                    seg.ticks.push(TickEntry {
                        tick: *tick,
                        record: i,
                        hash: *hash,
                        digests: list,
                    });
                }
                Record::Keyframe { snapshot } => {
                    let seg = self.segments.last_mut().ok_or_else(|| fail("no segment"))?;
                    seg.keyframes.push((snapshot.header().tick, i));
                }
                Record::End { .. } => {
                    let seg = self.segments.last_mut().ok_or_else(|| fail("no segment"))?;
                    seg.end = Some(i);
                    seg.info.end = SegmentEnd::End;
                }
                Record::Fault { tick, code, .. } => {
                    let seg = self.segments.last_mut().ok_or_else(|| fail("no segment"))?;
                    seg.info.end = SegmentEnd::Fault {
                        tick: *tick,
                        code: code.clone(),
                    };
                }
                Record::Tainted { tick, .. } => {
                    let segment = self.segments.last().map_or(0, |s| s.info.index);
                    self.tainted.get_or_insert(TickRef {
                        segment,
                        tick: *tick,
                    });
                }
                Record::Bundle(b) => {
                    self.bundles.entry(b.hash).or_insert(i);
                }
                Record::Data { hash, .. } => {
                    self.data.entry(*hash).or_insert(i);
                }
                Record::Formats { .. } | Record::DataRead { .. } | Record::Refused { .. } => {}
            }
        }
        Ok(())
    }

    /// The bytes of this replay, as the recorder would write them.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Problem> {
        let mut out = file_head(&self.header)?;
        for r in &self.records {
            r.frame(&mut out)
                .map_err(|e| error::encode("replay", None, &e.reason()))?;
        }
        Ok(out)
    }

    pub fn header(&self) -> &ReplayHeader {
        &self.header
    }

    pub fn complete(&self) -> bool {
        self.complete
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// The records, to edit a copy (then [`Replay::from_records`]).
    pub fn into_parts(self) -> (ReplayHeader, Vec<Record>, bool) {
        (self.header, self.records, self.complete)
    }

    pub fn segments(&self) -> Vec<SegmentInfo> {
        self.segments.iter().map(|s| s.info.clone()).collect()
    }

    pub(crate) fn segment(&self, i: u32) -> Option<&Segment> {
        self.segments.get(i as usize)
    }

    /// The ticks a segment recorded (empty when it recorded none).
    pub fn ticks(&self, segment: u32) -> RangeInclusive<Tick> {
        match self.segment(segment) {
            Some(s) if !s.ticks.is_empty() => Tick(s.info.start.0.0 + 1)..=s.info.last,
            #[allow(clippy::reversed_empty_ranges)] // empty on purpose
            _ => Tick(1)..=Tick(0),
        }
    }

    pub(crate) fn entry(&self, at: TickRef) -> Option<&TickEntry> {
        let s = self.segment(at.segment)?;
        let first = s.ticks.first()?.tick.0;
        s.ticks
            .get(usize::try_from(at.tick.0.checked_sub(first)?).ok()?)
    }

    /// The hash after tick `at`; at a segment's start point, its snapshot's world hash.
    pub fn hash(&self, at: TickRef) -> Option<TickHash> {
        let s = self.segment(at.segment)?;
        if at.tick == s.info.start.0 {
            return snapshot_of(&self.records[s.first]).map(Snapshot::world_hash);
        }
        self.entry(at).map(|e| e.hash)
    }

    /// The section digests after tick `at` (`Sections` recordings).
    pub fn digests(&self, at: TickRef) -> Option<&[(SectionKey, SectionDigest)]> {
        self.entry(at)?.digests.as_deref()
    }

    /// The writes applied at boundary `tick - 1`.
    pub fn writes(&self, at: TickRef) -> &[RecordedWrite] {
        match self.entry(at).map(|e| &self.records[e.record]) {
            Some(Record::Tick { writes, .. }) => writes,
            _ => &[],
        }
    }

    /// A segment's `End`: the last boundary's writes and the hash after them.
    pub fn end(&self, segment: u32) -> Option<(Tick, &[RecordedWrite], WorldHash)> {
        let i = self.segment(segment)?.end?;
        match &self.records[i] {
            Record::End {
                tick,
                writes,
                world,
            } => Some((*tick, writes, *world)),
            _ => None,
        }
    }

    /// The segment's start snapshot.
    pub fn start(&self, segment: u32) -> Option<&Snapshot> {
        snapshot_of(&self.records[self.segment(segment)?.first])
    }

    /// The keyframe at or before `at` in its segment; the segment's start when none.
    pub fn keyframe_at_or_before(&self, at: TickRef) -> Option<&Snapshot> {
        let s = self.segment(at.segment)?;
        let k = s.keyframes.iter().rev().find(|(t, _)| *t <= at.tick);
        k.map_or_else(
            || snapshot_of(&self.records[s.first]),
            |(_, i)| snapshot_of(&self.records[*i]),
        )
    }

    /// The keyframe taken exactly after tick `at`.
    pub fn keyframe_at(&self, at: TickRef) -> Option<&Snapshot> {
        let s = self.segment(at.segment)?;
        let (_, i) = s.keyframes.iter().find(|(t, _)| *t == at.tick)?;
        snapshot_of(&self.records[*i])
    }

    pub fn bundle(&self, h: &ContentHash) -> Option<&BundleRecord> {
        match &self.records[*self.bundles.get(h)?] {
            Record::Bundle(b) => Some(b),
            _ => None,
        }
    }

    /// Every bundle the replay embeds, in record order.
    pub fn bundles(&self) -> Vec<&BundleRecord> {
        self.records
            .iter()
            .filter_map(|r| {
                if let Record::Bundle(b) = r {
                    Some(b)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn data(&self, h: &ContentHash) -> Option<&[u8]> {
        match &self.records[*self.data.get(h)?] {
            Record::Data { bytes, .. } => Some(bytes),
            _ => None,
        }
    }

    pub fn tainted(&self) -> Option<TickRef> {
        self.tainted
    }

    /// The format tables in record order (one after each start, more after hot updates).
    pub fn formats(&self) -> Vec<&FormatTable> {
        self.records
            .iter()
            .filter_map(|r| {
                if let Record::Formats { table } = r {
                    Some(table)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Every tick hash, segment by segment, each segment's start point first.
    pub fn tick_hashes(&self) -> Vec<(TickRef, TickHash)> {
        let mut out = Vec::new();
        for s in &self.segments {
            let segment = s.info.index;
            if let Some(snap) = snapshot_of(&self.records[s.first]) {
                out.push((
                    TickRef {
                        segment,
                        tick: s.info.start.0,
                    },
                    snap.world_hash(),
                ));
            }
            out.extend(s.ticks.iter().map(|e| {
                (
                    TickRef {
                        segment,
                        tick: e.tick,
                    },
                    e.hash,
                )
            }));
        }
        out
    }
}
