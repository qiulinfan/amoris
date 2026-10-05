//! Snapshots (docs/spec/persistence.md 4 and 6): the world as canonical sections with their
//! digests and world hash, the byte layout, and snapshot, world hash, restore and fork.

use std::sync::Arc;

use bevy_ecs::prelude::{Entity, Resource, With, World};
use pocket_contract::Problem;
use pocket_sim::sim::TickState;
use pocket_sim::{ContentHash, EntityAllocator, EntityId, EntityIndex, SimClock, Tick};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};

use crate::error;
use crate::format::Fingerprint;
use crate::hash::{
    SNAPSHOT_FORMAT, SectionDigest, SectionKey, SectionKind, WorldHash, digest_prefix,
    section_digest, valid_section_name, world_hash_of,
};
use crate::pce::{self, Decoder, PceError, write_uleb};
use crate::project::{self, Project};
use crate::registry::{Entry, Registry};
use crate::sections::{self, Codec, IdMap, StagedCache, StagedSection, decode_problem};
use crate::version::EngineVersion;

/// The snapshot magic.
pub const SNAPSHOT_MAGIC: &[u8; 8] = b"P3DSNAP\0";

/// A snapshot's header; it does not enter the world hash.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct SnapshotHeader {
    /// The clock's tick at the boundary.
    pub tick: Tick,
    /// Writes applied at that boundary first (simulation.md 4.2).
    pub writes: u32,
    pub engine: EngineVersion,
    /// The bundle loaded at the boundary (versions.md 3.2).
    pub bundle: ContentHash,
}

/// Kept current by `pocket-runtime` and read into the header; class Ignored. Restore sets it from
/// the snapshot's header, so a fork's snapshot equals its source's byte for byte.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub struct SnapshotContext {
    pub bundle: ContentHash,
    pub writes: u32,
}

impl Default for SnapshotContext {
    fn default() -> Self {
        SnapshotContext {
            bundle: ContentHash([0; 32]),
            writes: 0,
        }
    }
}

/// One section: its key, schema version and fingerprint, digest and bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SectionData {
    pub key: SectionKey,
    pub version: u32,
    pub fingerprint: Fingerprint,
    pub digest: SectionDigest,
    pub bytes: Arc<[u8]>,
}

/// Immutable, cheap to clone, `Send + Sync`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Snapshot {
    header: Arc<SnapshotHeader>,
    sections: Arc<[SectionData]>,
    hash: WorldHash,
}

impl Snapshot {
    /// A snapshot from its parts; the world hash is computed from the sections.
    pub fn from_parts(header: SnapshotHeader, sections: Vec<SectionData>) -> Snapshot {
        let hash = world_hash_of(sections.iter().map(|s| &s.digest));
        Snapshot {
            header: Arc::new(header),
            sections: sections.into(),
            hash,
        }
    }

    pub fn header(&self) -> &SnapshotHeader {
        &self.header
    }

    /// The same snapshot naming another bundle in its header (which the world hash does not
    /// cover): the recorder stamps the bundle it was told the world runs when the world's
    /// `SnapshotContext` names none.
    pub fn with_bundle(&self, bundle: ContentHash) -> Snapshot {
        let mut header = (*self.header).clone();
        header.bundle = bundle;
        Snapshot {
            header: Arc::new(header),
            sections: self.sections.clone(),
            hash: self.hash,
        }
    }

    pub fn world_hash(&self) -> WorldHash {
        self.hash
    }

    pub fn sections(&self) -> &[SectionData] {
        &self.sections
    }

    pub fn section(&self, key: &SectionKey) -> Option<&SectionData> {
        self.sections
            .binary_search_by(|s| s.key.cmp(key))
            .ok()
            .map(|i| &self.sections[i])
    }

    /// The section digests in section order.
    pub fn digests(&self) -> Vec<(SectionKey, SectionDigest)> {
        self.sections
            .iter()
            .map(|s| (s.key.clone(), s.digest))
            .collect()
    }

    /// The layout of 4.2.
    pub fn to_bytes(&self) -> Vec<u8> {
        let size: usize = self
            .sections
            .iter()
            .map(|s| s.bytes.len() + s.key.name.len() + 32)
            .sum();
        let mut out = Vec::with_capacity(size + 128);
        out.extend_from_slice(SNAPSHOT_MAGIC);
        out.extend_from_slice(&SNAPSHOT_FORMAT.to_le_bytes());
        let header = pce::to_bytes(&*self.header).unwrap_or_default();
        write_uleb(&mut out, header.len() as u64);
        out.extend_from_slice(&header);
        write_uleb(&mut out, self.sections.len() as u64);
        for s in self.sections.iter() {
            out.push(s.key.kind.code());
            write_uleb(&mut out, s.key.name.len() as u64);
            out.extend_from_slice(s.key.name.as_bytes());
            out.extend_from_slice(&s.version.to_le_bytes());
            out.extend_from_slice(&s.fingerprint.0);
            write_uleb(&mut out, s.bytes.len() as u64);
            out.extend_from_slice(&s.bytes);
        }
        out.extend_from_slice(&self.hash.0);
        out
    }

    /// Checks magic, format, framing, order and the trailing hash; decodes no section.
    pub fn from_bytes(bytes: &[u8]) -> Result<Snapshot, Problem> {
        if bytes.len() < 12 || &bytes[..8] != SNAPSHOT_MAGIC {
            let found = String::from_utf8_lossy(&bytes[..bytes.len().min(8)]).into_owned();
            return Err(error::format("the magic P3DSNAP", &found));
        }
        let format = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        if format != SNAPSHOT_FORMAT {
            return Err(error::version_format("snapshot", format, SNAPSHOT_FORMAT));
        }
        parse_body(bytes, 12).map_err(|e| match e {
            Parse::Pce(e) => e.problem(0),
            Parse::Problem(p) => p,
        })
    }
}

enum Parse {
    Pce(PceError),
    Problem(Problem),
}

impl From<Problem> for Parse {
    fn from(p: Problem) -> Parse {
        Parse::Problem(p)
    }
}

impl From<PceError> for Parse {
    fn from(e: PceError) -> Parse {
        Parse::Pce(e)
    }
}

fn parse_body(bytes: &[u8], start: usize) -> Result<Snapshot, Parse> {
    let mut d = Decoder::new(bytes, true);
    d.take(start)?;
    let hlen = usize::try_from(d.uleb()?).unwrap_or(usize::MAX);
    let at = d.pos();
    let hbytes = d.take(hlen)?;
    let header: SnapshotHeader = pce::from_bytes(hbytes, true).map_err(|e| e.at(0).problem(at))?;
    let count = d.uleb()?;
    let mut sections: Vec<SectionData> = Vec::new();
    let mut buf = Vec::new();
    for _ in 0..count {
        let at = d.pos();
        let code = d.value::<u8>()?;
        let kind = SectionKind::from_code(code).ok_or_else(|| {
            Parse::Problem(error::format("a section kind 0 to 3", &code.to_string()))
        })?;
        let name: String = d.value()?;
        if !valid_section_name(&name) {
            return Err(PceError::noncanonical(at, format!("the section name '{name}'")).into());
        }
        let version = d.value::<u32>()?;
        let fingerprint = Fingerprint(d.value::<[u8; 16]>()?);
        let len = usize::try_from(d.uleb()?).unwrap_or(usize::MAX);
        let data = d.take(len)?;
        let key = SectionKey { kind, name };
        if sections.last().is_some_and(|p| p.key >= key) {
            return Err(PceError::noncanonical(at, "sections out of order or repeated").into());
        }
        buf.clear();
        digest_prefix(&mut buf, key.kind, &key.name, version);
        buf.extend_from_slice(data);
        sections.push(SectionData {
            key,
            version,
            fingerprint,
            digest: section_digest(&buf),
            bytes: Arc::from(data),
        });
    }
    let stored = WorldHash(d.value::<[u8; 16]>()?);
    d.finish()?;
    let snap = Snapshot::from_parts(header, sections);
    if snap.hash != stored {
        return Err(Parse::Problem(error::hash_mismatch(stored, snap.hash)));
    }
    Ok(snap)
}

// A snapshot travels inside replays and saves as its bytes.
impl Serialize for Snapshot {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.to_bytes())
    }
}

impl<'de> Deserialize<'de> for Snapshot {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Snapshot, D::Error> {
        let bytes: &[u8] = <&[u8]>::deserialize(d)?;
        Snapshot::from_bytes(bytes).map_err(|p| de::Error::custom(p.message))
    }
}

/// `persist.not_at_boundary` while a tick runs.
pub(crate) fn at_boundary(world: &World) -> Result<(), Problem> {
    match world
        .get_resource::<TickState>()
        .and_then(TickState::running)
    {
        Some(t) => Err(error::not_at_boundary(t.0)),
        None => Ok(()),
    }
}

enum Source<'a> {
    Registered(&'a Entry),
    Project(Project),
}

impl Source<'_> {
    fn key(&self) -> &SectionKey {
        match self {
            Source::Registered(e) => &e.key,
            Source::Project(p) => &p.key,
        }
    }
    fn version(&self) -> u32 {
        match self {
            Source::Registered(e) => e.version,
            Source::Project(p) => p.schema.version,
        }
    }
    fn fingerprint(&self) -> Fingerprint {
        match self {
            Source::Registered(e) => e.fingerprint,
            Source::Project(p) => p.fingerprint,
        }
    }
}

/// The registry's sections and the world's project components, in section order.
fn sources<'a>(world: &World, reg: &'a Registry) -> Result<Vec<Source<'a>>, Problem> {
    let mut out: Vec<Source<'a>> = reg.entries().iter().map(Source::Registered).collect();
    for p in project::projects(world)? {
        if reg.entry(&p.key).is_some() {
            return Err(error::duplicate_name(&p.key.name));
        }
        out.push(Source::Project(p));
    }
    out.sort_by(|a, b| a.key().cmp(b.key()));
    Ok(out)
}

/// Encodes every present section into `buf` (prefix, then data) and hands it to `f` with the
/// offset where the data starts (5.4: one buffer, one hash call per section).
fn each_section(
    world: &World,
    reg: &Registry,
    mut f: impl FnMut(&SectionKey, u32, Fingerprint, &[u8], usize),
) -> Result<(), Problem> {
    at_boundary(world)?;
    reg.check_classes(world)?;
    reg.check_rows(world)?;
    let mut buf = Vec::with_capacity(4096);
    for s in sources(world, reg)? {
        let key = s.key();
        buf.clear();
        digest_prefix(&mut buf, key.kind, &key.name, s.version());
        let start = buf.len();
        let present = match &s {
            Source::Registered(e) => match &e.codec {
                Codec::Entities => sections::encode_entities(world, &mut buf).map(|()| true)?,
                Codec::Typed { encode, .. } => encode(world, &mut buf, key)?,
                Codec::Cache { encode, .. } => {
                    encode(world, &mut buf)?;
                    buf.len() > start
                }
            },
            Source::Project(p) => project::encode(world, p, &mut buf)?,
        };
        if present {
            f(key, s.version(), s.fingerprint(), &buf, start);
        }
    }
    Ok(())
}

fn header_of(world: &World) -> SnapshotHeader {
    let ctx = world
        .get_resource::<SnapshotContext>()
        .copied()
        .unwrap_or_default();
    SnapshotHeader {
        tick: world.get_resource::<SimClock>().map_or(Tick(0), |c| c.tick),
        writes: ctx.writes,
        engine: EngineVersion::current().clone(),
        bundle: ctx.bundle,
    }
}

/// The world's snapshot. Pure: changes nothing.
pub fn snapshot(world: &World, reg: &Registry) -> Result<Snapshot, Problem> {
    let mut sections = Vec::new();
    each_section(world, reg, |key, version, fingerprint, buf, start| {
        sections.push(SectionData {
            key: key.clone(),
            version,
            fingerprint,
            digest: section_digest(buf),
            bytes: Arc::from(&buf[start..]),
        });
    })?;
    Ok(Snapshot::from_parts(header_of(world), sections))
}

/// The section digests in section order.
pub fn section_digests(
    world: &World,
    reg: &Registry,
) -> Result<Vec<(SectionKey, SectionDigest)>, Problem> {
    let mut out = Vec::new();
    each_section(world, reg, |key, _, _, buf, _| {
        out.push((key.clone(), section_digest(buf)))
    })?;
    Ok(out)
}

/// The world hash.
pub fn world_hash(world: &World, reg: &Registry) -> Result<WorldHash, Problem> {
    let digests = section_digests(world, reg)?;
    Ok(world_hash_of(digests.iter().map(|(_, d)| d)))
}

/// How a restore checks itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RestoreOptions {
    /// Hash the restored world and compare with the snapshot's (one hash pass).
    pub verify: bool,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        RestoreOptions { verify: true }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RestoreReport {
    pub tick: Tick,
    pub world_hash: WorldHash,
    pub entities: u64,
}

/// Decodes every section of `snap` against the registry and `world`'s project components, before
/// anything changes.
fn stage(
    world: &World,
    snap: &Snapshot,
    reg: &Registry,
) -> Result<Vec<Box<dyn StagedSection>>, Problem> {
    let projects = project::projects(world)?;
    let entities = SectionKey::entities();
    if reg.entry(&entities).is_some() && snap.section(&entities).is_none() {
        return Err(error::noncanonical(
            0,
            "a snapshot without its entities section",
        ));
    }
    let mut staged: Vec<Box<dyn StagedSection>> = Vec::with_capacity(snap.sections().len());
    let mut live: Vec<EntityId> = Vec::new();
    for s in snap.sections() {
        let (version, fingerprint) = if let Some(e) = reg.entry(&s.key) {
            (e.version, e.fingerprint)
        } else if let Some(p) = projects.iter().find(|p| p.key == s.key) {
            (p.schema.version, p.fingerprint)
        } else {
            return Err(error::unknown_section(&s.key));
        };
        if (version, fingerprint) != (s.version, s.fingerprint) {
            return Err(error::version(
                &s.key,
                (s.version, &s.fingerprint.0),
                (version, &fingerprint.0),
            ));
        }
        let bytes = &s.bytes[..];
        let next: Box<dyn StagedSection> = match reg.entry(&s.key).map(|e| &e.codec) {
            Some(Codec::Entities) => {
                let e = sections::decode_entities(bytes)?;
                live = e.ids().to_vec();
                Box::new(e)
            }
            Some(Codec::Typed { decode, .. }) => decode(bytes, &s.key, &live)?,
            Some(Codec::Cache { decode, .. }) => Box::new(StagedCache(decode(bytes)?)),
            None => {
                let p = projects
                    .iter()
                    .find(|p| p.key == s.key)
                    .ok_or_else(|| error::unknown_section(&s.key))?;
                project::decode(p, bytes, &live)?
            }
        };
        staged.push(next);
    }
    Ok(staged)
}

/// Replaces `world`'s persisted state with the snapshot's (6.4): checks the target's classes and
/// rows and decodes every section first (a failure changes nothing), then despawns every entity
/// with an `EntityId`, removes every persisted resource, cache and Derived type, applies the
/// sections in order and runs the rebuild functions, the entity index first.
pub fn restore(
    world: &mut World,
    snap: &Snapshot,
    reg: &Registry,
    opts: RestoreOptions,
) -> Result<RestoreReport, Problem> {
    at_boundary(world)?;
    reg.check_classes(world)?;
    reg.check_rows(world)?;
    let staged = stage(world, snap, reg)?;
    let mut q = world.query_filtered::<Entity, With<EntityId>>();
    #[allow(clippy::disallowed_methods)] // every one is despawned; the order changes nothing
    let old: Vec<Entity> = q.iter(world).collect();
    for e in old {
        world.despawn(e);
    }
    world.remove_resource::<EntityAllocator>();
    for e in reg.entries() {
        if let Codec::Typed {
            remove: Some(remove),
            ..
        } = &e.codec
        {
            remove(world);
        }
    }
    for t in reg.derived() {
        if let Some(id) = world.components().get_id(t) {
            world.remove_resource_by_id(id);
        }
    }
    let mut ids = IdMap::default();
    for s in staged {
        s.apply(world, &mut ids);
    }
    world.insert_resource(SnapshotContext {
        bundle: snap.header().bundle,
        writes: snap.header().writes,
    });
    EntityIndex::rebuild(world);
    for (_, f) in reg.rebuilds() {
        f(world);
    }
    let entities = world
        .get_resource::<EntityIndex>()
        .map_or(0, |i| i.len() as u64);
    let hash = if opts.verify {
        let digests = section_digests(world, reg)?;
        let hash = world_hash_of(digests.iter().map(|(_, d)| d));
        if hash != snap.world_hash() {
            let mut differ: Vec<SectionKey> = digests
                .iter()
                .filter(|(k, d)| snap.section(k).is_none_or(|s| s.digest != *d))
                .map(|(k, _)| k.clone())
                .collect();
            differ.extend(
                snap.sections()
                    .iter()
                    .filter(|s| !digests.iter().any(|(k, _)| *k == s.key))
                    .map(|s| s.key.clone()),
            );
            differ.sort();
            return Err(error::restore_mismatch(snap.world_hash(), hash, &differ));
        }
        hash
    } else {
        snap.world_hash()
    };
    Ok(RestoreReport {
        tick: snap.header().tick,
        world_hash: hash,
        entities,
    })
}

/// `restore(fresh(), snapshot(world))`. `fresh` builds an empty world with the schedule's
/// resources and the non-persisted state (spec-sim's world builder).
pub fn fork(
    world: &World,
    reg: &Registry,
    fresh: impl FnOnce() -> World,
) -> Result<World, Problem> {
    let snap = snapshot(world, reg)?;
    let mut out = fresh();
    restore(&mut out, &snap, reg, RestoreOptions::default())?;
    Ok(out)
}

/// A fork into a world that already exists (a `Sim`'s, whose schedule holds it).
pub fn fork_into(
    world: &World,
    target: &mut World,
    reg: &Registry,
) -> Result<RestoreReport, Problem> {
    let snap = snapshot(world, reg)?;
    restore(target, &snap, reg, RestoreOptions::default())
}

/// The bytes of a decode error in a section, for callers that decode sections themselves.
pub fn section_problem(key: &SectionKey, e: &PceError) -> Problem {
    decode_problem(key, e)
}
