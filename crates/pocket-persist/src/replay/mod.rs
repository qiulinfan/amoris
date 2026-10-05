//! Replays (docs/spec/replay.md 2): a start snapshot plus every write applied afterwards with each
//! tick's hash, in segments that a restore or a reset opens; the recorder, the reader, verify,
//! seek, lockstep and the first divergence.

mod divergence;
mod reader;
mod recorder;
mod verify;

use std::collections::BTreeMap;

use pocket_contract::Problem;
use pocket_sim::{ContentHash, Tick};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use divergence::{
    Divergence, DivergenceKind, LockstepOptions, LockstepReport, first_divergence, lockstep,
};
pub use reader::{Replay, SegmentEnd, SegmentInfo, TickRef};
pub use recorder::{FileSink, MemorySink, RecordOptions, RecordSink, Recorder, ReplaySummary};
pub use verify::{
    ReplayMode, ReplayOutcome, ReplayReport, ReplaySource, SeekReport, SeekScripts, Stepper,
    VerifyOptions, seek, verify,
};

use crate::error;
use crate::hash::{SectionDigest, SectionKey, TickHash, WorldHash};
use crate::pce::{self, Decoder, PceError, write_uleb};
use crate::snapshot::Snapshot;
use crate::version::{EngineVersion, FormatTable, Labels};

/// The replay magic.
pub const REPLAY_MAGIC: &[u8; 8] = b"P3DRPLY\0";
/// The replay layout's format number (versions.md 3.4).
pub const REPLAY_FORMAT: u32 = 1;

/// Who sent a command (threads.md 5.1). Declared here, below `pocket-link`, which re-exports it,
/// because recorded writes carry it (open choice 1 of replay.md). The derived order is the order
/// in which a boundary applies commands from different sources.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum Source {
    /// The runtime's own producers: prepared script swaps, finished imports.
    Host,
    /// The person at the editor.
    Editor,
    /// A developer-facing session, numbered in the order clients were handed out.
    Developer(u32),
    /// A player, human or agent, by its seat's index.
    Player(u32),
}

/// A write as the command queue hands it to the recorder (threads.md 5.3).
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    /// The tick it is an input of: applied at boundary `tick - 1`.
    pub tick: Tick,
    /// Its position among the writes applied at that boundary.
    pub index: u32,
    pub source: Source,
    pub seq: u64,
    pub name: String,
    /// The canonical recorded form.
    pub params: serde_json::Value,
}

/// A write as a replay stores it (2.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordedWrite {
    pub source: Source,
    pub seq: u64,
    /// The command's catalog name.
    pub name: String,
    /// `serde_json` compact text of the canonical params: keys sorted, floats shortest round trip.
    pub params: String,
}

impl RecordedWrite {
    /// The recorded form of an applied write.
    pub fn of(w: &Applied) -> RecordedWrite {
        RecordedWrite {
            source: w.source,
            seq: w.seq,
            name: w.name.clone(),
            params: canonical_json(&w.params),
        }
    }

    /// The params, read back with `float_roundtrip` to the same bits.
    pub fn params_json(&self) -> serde_json::Value {
        serde_json::from_str(&self.params).unwrap_or(serde_json::Value::Null)
    }

    /// The write as JSON, for problems and reports.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"source": self.source, "seq": self.seq, "name": self.name,
                           "params": self.params_json()})
    }
}

/// The canonical text of a JSON value: `serde_json` compact output, whose maps iterate in key order
/// (no `preserve_order`).
pub fn canonical_json(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".to_owned())
}

/// What a replay holds besides its records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayHeader {
    pub engine: EngineVersion,
    pub start_tick: Tick,
    pub seed: u64,
    pub tick_rate: u32,
    /// Canonical JSON run configuration (versions.md 3.7).
    pub run_config: String,
    /// 0: none.
    pub keyframe_every: u32,
    pub digests: DigestLevel,
    pub label: Labels,
    pub parent: Option<ParentRef>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DigestLevel {
    World,
    Sections,
}

/// The run a recording was forked from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentRef {
    pub replay: Option<ContentHash>,
    pub tick: Tick,
    pub world: WorldHash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RebaseCause {
    Restore,
    Reset,
}

/// A bundle as a replay carries it (2.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BundleRecord {
    pub hash: ContentHash,
    /// The TypeScript sources the hash covers, in path order.
    pub files: Vec<(String, Vec<u8>)>,
    pub compiled: Option<CompiledRecord>,
}

impl BundleRecord {
    /// A bundle of sources, its hash computed (versions.md 3.2) and its files put in path order.
    pub fn new(
        mut files: Vec<(String, Vec<u8>)>,
        compiled: Option<CompiledRecord>,
    ) -> BundleRecord {
        files.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        BundleRecord {
            hash: crate::version::bundle_hash(&files),
            files,
            compiled,
        }
    }

    /// The empty bundle: a world that runs no scripts.
    pub fn empty() -> BundleRecord {
        BundleRecord::new(Vec::new(), None)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledRecord {
    /// `EngineVersion.source` of the engine that compiled it.
    pub engine_source: ContentHash,
    pub modules: Vec<CompiledModuleRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompiledModuleRecord {
    pub path: String,
    pub js: String,
    pub map: String,
}

/// Section digests that changed since the previous tick of the segment (2.2).
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DigestDelta {
    pub added: Vec<SectionKey>,
    pub removed: Vec<u32>,
    pub changed: Vec<(u32, SectionDigest)>,
}

impl DigestDelta {
    /// The delta from `prev` to `next`, both in section order.
    pub fn between(
        prev: &[(SectionKey, SectionDigest)],
        next: &[(SectionKey, SectionDigest)],
    ) -> DigestDelta {
        let mut d = DigestDelta::default();
        for (i, (k, _)) in prev.iter().enumerate() {
            if next.binary_search_by(|(n, _)| n.cmp(k)).is_err() {
                d.removed.push(u32::try_from(i).unwrap_or(u32::MAX));
            }
        }
        for (i, (k, digest)) in next.iter().enumerate() {
            match prev.binary_search_by(|(p, _)| p.cmp(k)) {
                Err(_) => {
                    d.added.push(k.clone());
                    d.changed
                        .push((u32::try_from(i).unwrap_or(u32::MAX), *digest));
                }
                Ok(j) if prev[j].1 != *digest => {
                    d.changed
                        .push((u32::try_from(i).unwrap_or(u32::MAX), *digest));
                }
                Ok(_) => {}
            }
        }
        d
    }

    /// Applies the delta to a running list (2.2): remove, compact, insert, then change.
    pub fn apply(&self, list: &mut Vec<(SectionKey, SectionDigest)>) -> Result<(), String> {
        let mut drop = vec![false; list.len()];
        for &i in &self.removed {
            *drop
                .get_mut(i as usize)
                .ok_or_else(|| format!("removed index {i} past {} sections", list.len()))? = true;
        }
        let mut it = drop.iter();
        list.retain(|_| !*it.next().unwrap_or(&false));
        for k in &self.added {
            match list.binary_search_by(|(p, _)| p.cmp(k)) {
                Ok(_) => return Err(format!("added section {k} is present already")),
                Err(at) => list.insert(at, (k.clone(), SectionDigest([0; 16]))),
            }
        }
        for (i, digest) in &self.changed {
            let slot = list
                .get_mut(*i as usize)
                .ok_or_else(|| format!("changed index {i} past the sections"))?;
            slot.1 = *digest;
        }
        Ok(())
    }
}

/// One record of a replay file (2.2).
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    Start {
        snapshot: Snapshot,
    },
    Formats {
        table: FormatTable,
    },
    Bundle(BundleRecord),
    Tick {
        tick: Tick,
        writes: Vec<RecordedWrite>,
        hash: TickHash,
        digests: DigestDelta,
    },
    Keyframe {
        snapshot: Snapshot,
    },
    DataRead {
        path: String,
        hash: ContentHash,
    },
    Refused {
        write: RecordedWrite,
        code: String,
    },
    End {
        tick: Tick,
        writes: Vec<RecordedWrite>,
        world: WorldHash,
    },
    Rebase {
        cause: RebaseCause,
        snapshot: Snapshot,
    },
    Data {
        hash: ContentHash,
        bytes: Vec<u8>,
    },
    Fault {
        tick: Tick,
        system: String,
        code: String,
    },
    Tainted {
        tick: Tick,
        reason: String,
    },
}

impl Record {
    pub fn tag(&self) -> u8 {
        match self {
            Record::Start { .. } => 0x01,
            Record::Formats { .. } => 0x02,
            Record::Bundle(_) => 0x03,
            Record::Tick { .. } => 0x04,
            Record::Keyframe { .. } => 0x05,
            Record::DataRead { .. } => 0x06,
            Record::Refused { .. } => 0x07,
            Record::End { .. } => 0x08,
            Record::Rebase { .. } => 0x09,
            Record::Data { .. } => 0x0A,
            Record::Fault { .. } => 0x0B,
            Record::Tainted { .. } => 0x0C,
        }
    }

    /// The payload's PCE.
    pub fn payload(&self) -> Result<Vec<u8>, PceError> {
        let mut out = Vec::new();
        let o = &mut out;
        match self {
            Record::Start { snapshot } | Record::Keyframe { snapshot } => {
                pce::encode_into(snapshot, o, false)
            }
            Record::Formats { table } => pce::encode_into(table, o, false),
            Record::Bundle(b) => pce::encode_into(b, o, false),
            Record::Tick {
                tick,
                writes,
                hash,
                digests,
            } => pce::encode_into(&(tick, writes, hash, digests), o, false),
            Record::DataRead { path, hash } => pce::encode_into(&(path, hash), o, false),
            Record::Refused { write, code } => pce::encode_into(&(write, code), o, false),
            Record::End {
                tick,
                writes,
                world,
            } => pce::encode_into(&(tick, writes, world), o, false),
            Record::Rebase { cause, snapshot } => pce::encode_into(&(cause, snapshot), o, false),
            Record::Data { hash, bytes } => {
                pce::encode_into(hash, o, false)?;
                write_uleb(o, bytes.len() as u64);
                o.extend_from_slice(bytes);
                Ok(())
            }
            Record::Fault { tick, system, code } => {
                pce::encode_into(&(tick, system, code), o, false)
            }
            Record::Tainted { tick, reason } => pce::encode_into(&(tick, reason), o, false),
        }?;
        Ok(out)
    }

    /// The framed record: tag, ULEB128 length, payload.
    pub fn frame(&self, out: &mut Vec<u8>) -> Result<(), PceError> {
        let payload = self.payload()?;
        out.push(self.tag());
        write_uleb(out, payload.len() as u64);
        out.extend_from_slice(&payload);
        Ok(())
    }

    /// Decodes a payload of `tag`; `None` for an unknown tag.
    pub fn decode(tag: u8, p: &[u8]) -> Option<Result<Record, PceError>> {
        Some(match tag {
            0x01 => de(p).map(|snapshot| Record::Start { snapshot }),
            0x02 => de(p).map(|table| Record::Formats { table }),
            0x03 => de(p).map(Record::Bundle),
            0x04 => de(p).map(|(tick, writes, hash, digests)| Record::Tick {
                tick,
                writes,
                hash,
                digests,
            }),
            0x05 => de(p).map(|snapshot| Record::Keyframe { snapshot }),
            0x06 => de(p).map(|(path, hash)| Record::DataRead { path, hash }),
            0x07 => de(p).map(|(write, code)| Record::Refused { write, code }),
            0x08 => de(p).map(|(tick, writes, world)| Record::End {
                tick,
                writes,
                world,
            }),
            0x09 => de(p).map(|(cause, snapshot)| Record::Rebase { cause, snapshot }),
            0x0A => {
                let mut d = Decoder::new(p, false);
                (|| {
                    let hash: ContentHash = d.value()?;
                    let bytes: &[u8] = d.value()?;
                    d.finish()?;
                    Ok(Record::Data {
                        hash,
                        bytes: bytes.to_vec(),
                    })
                })()
            }
            0x0B => de(p).map(|(tick, system, code)| Record::Fault { tick, system, code }),
            0x0C => de(p).map(|(tick, reason)| Record::Tainted { tick, reason }),
            _ => return None,
        })
    }
}

fn de<T: serde::de::DeserializeOwned>(p: &[u8]) -> Result<T, PceError> {
    pce::from_bytes(p, false)
}

/// The file's start: magic, format and header.
pub(crate) fn file_head(header: &ReplayHeader) -> Result<Vec<u8>, Problem> {
    let mut out = Vec::new();
    out.extend_from_slice(REPLAY_MAGIC);
    out.extend_from_slice(&REPLAY_FORMAT.to_le_bytes());
    let h = pce::to_bytes(header).map_err(|e| error::encode("replay header", None, &e.reason()))?;
    write_uleb(&mut out, h.len() as u64);
    out.extend_from_slice(&h);
    Ok(out)
}

/// Labels as a map, for `RecordOptions`.
pub type LabelMap = BTreeMap<String, String>;
