//! Section codecs (docs/spec/persistence.md 4.1): the entities section, resources, components and
//! caches written as PCE, and decoded into staged values that restore applies only once every
//! section has decoded (6.4, atomic for errors).

use bevy_ecs::prelude::{Component, Entity, Resource, World};
use pocket_contract::Problem;
use pocket_sim::{EntityAllocator, EntityId, Persisted, PersistedCache, Staged};
use serde::Serialize;

use crate::error;
use crate::hash::SectionKey;
use crate::pce::{self, Decoder, PceError, write_uleb};

/// Where each restored `EntityId` lives in the world being restored, ascending by id.
#[derive(Default)]
pub(crate) struct IdMap(Vec<(EntityId, Entity)>);

impl IdMap {
    pub(crate) fn get(&self, id: EntityId) -> Entity {
        // Every row's id was checked against the live set before the world was touched.
        let i = self
            .0
            .binary_search_by_key(&id, |p| p.0)
            .expect("a row names a live entity");
        self.0[i].1
    }
}

/// A decoded section not yet applied.
pub(crate) trait StagedSection: Send {
    fn apply(self: Box<Self>, world: &mut World, ids: &mut IdMap);
}

type CacheDecodeFn = fn(&[u8]) -> Result<Box<dyn Staged>, Problem>;
type EncodeFn = fn(&World, &mut Vec<u8>, &SectionKey) -> Result<bool, Problem>;
type DecodeFn = fn(&[u8], &SectionKey, &[EntityId]) -> Result<Box<dyn StagedSection>, Problem>;

/// How one registered section is written and read.
pub(crate) enum Codec {
    Entities,
    Typed {
        encode: EncodeFn,
        decode: DecodeFn,
        /// Removes a persisted resource before a restore applies sections.
        remove: Option<fn(&mut World)>,
    },
    Cache {
        encode: fn(&World, &mut Vec<u8>) -> Result<(), Problem>,
        decode: CacheDecodeFn,
        rebuild: fn(&mut World) -> Result<(), Problem>,
    },
}

/// A decode failure inside a section, as `persist.noncanonical` or `persist.truncated` naming it.
pub(crate) fn decode_problem(key: &SectionKey, e: &PceError) -> Problem {
    match e {
        PceError::Truncated { offset } => error::truncated(*offset),
        PceError::Noncanonical { offset, reason } => {
            error::noncanonical(*offset, &format!("in section {key}: {reason}"))
        }
        PceError::Encode(reason) => error::noncanonical(0, &format!("in section {key}: {reason}")),
    }
}

/// Fails unless re-encoding what was decoded gives the bytes back (3.6: `encode(decode(b)) == b`),
/// which catches maps and sets whose entries arrive out of their container's order.
fn canonical(
    key: &SectionKey,
    bytes: &[u8],
    again: Result<Vec<u8>, PceError>,
) -> Result<(), Problem> {
    match again {
        Ok(b) if b == bytes => Ok(()),
        Ok(_) => Err(error::noncanonical(
            0,
            &format!("in section {key}: the value does not encode back to its bytes"),
        )),
        Err(e) => Err(decode_problem(key, &e)),
    }
}

// --- Entities -------------------------------------------------------------------------------

/// The allocator, then the live ids ascending.
pub(crate) fn encode_entities(world: &World, out: &mut Vec<u8>) -> Result<(), Problem> {
    let alloc = world
        .get_resource::<EntityAllocator>()
        .ok_or_else(|| error::encode("entities", None, "the world has no EntityAllocator"))?;
    pce::encode_into(alloc, out, true).map_err(|e| error::encode("entities", None, &e.reason()))?;
    let mut ids: Vec<EntityId> = match world.try_query::<&EntityId>() {
        #[allow(clippy::disallowed_methods)] // sorted by EntityId below
        Some(mut q) => q.iter(world).copied().collect(),
        None => Vec::new(),
    };
    ids.sort_unstable();
    write_uleb(out, ids.len() as u64);
    for id in ids {
        out.extend_from_slice(&id.get().to_le_bytes());
    }
    Ok(())
}

pub(crate) struct StagedEntities {
    alloc: EntityAllocator,
    ids: Vec<EntityId>,
}

impl StagedEntities {
    pub(crate) fn ids(&self) -> &[EntityId] {
        &self.ids
    }
}

pub(crate) fn decode_entities(bytes: &[u8]) -> Result<StagedEntities, Problem> {
    let key = SectionKey::entities();
    let fail = |e: PceError| decode_problem(&key, &e);
    let mut d = Decoder::new(bytes, true);
    let alloc: EntityAllocator = d.value().map_err(fail)?;
    let n = d.uleb().map_err(fail)?;
    let mut ids: Vec<EntityId> =
        Vec::with_capacity(usize::try_from(n).unwrap_or(0).min(bytes.len() / 8));
    for _ in 0..n {
        let at = d.pos();
        let id: EntityId = d.value().map_err(fail)?;
        if ids.last().is_some_and(|p| *p >= id) || !alloc.allocated(id) {
            return Err(fail(PceError::noncanonical(
                at,
                format!("entity {} is out of order or was never allocated", id.get()),
            )));
        }
        ids.push(id);
    }
    d.finish().map_err(fail)?;
    Ok(StagedEntities { alloc, ids })
}

impl StagedSection for StagedEntities {
    fn apply(self: Box<Self>, world: &mut World, ids: &mut IdMap) {
        world.insert_resource(self.alloc);
        let entities: Vec<Entity> = world.spawn_batch(self.ids.iter().copied()).collect();
        ids.0 = self.ids.into_iter().zip(entities).collect();
    }
}

// --- Resources ------------------------------------------------------------------------------

pub(crate) fn resource_codec<R: Persisted + Resource>() -> Codec {
    Codec::Typed {
        encode: encode_resource::<R>,
        decode: decode_resource::<R>,
        remove: Some(|w| {
            w.remove_resource::<R>();
        }),
    }
}

fn encode_resource<R: Persisted + Resource>(
    world: &World,
    out: &mut Vec<u8>,
    key: &SectionKey,
) -> Result<bool, Problem> {
    let Some(r) = world.get_resource::<R>() else {
        return Ok(false);
    };
    pce::encode_into(r, out, true)
        .map_err(|e| error::encode(&key.to_string(), None, &e.reason()))?;
    Ok(true)
}

struct StagedResource<R>(R);

impl<R: Resource> StagedSection for StagedResource<R> {
    fn apply(self: Box<Self>, world: &mut World, _: &mut IdMap) {
        world.insert_resource(self.0);
    }
}

fn decode_resource<R: Persisted + Resource>(
    bytes: &[u8],
    key: &SectionKey,
    _: &[EntityId],
) -> Result<Box<dyn StagedSection>, Problem> {
    let r: R = pce::from_bytes_canonical(bytes, true).map_err(|e| decode_problem(key, &e))?;
    Ok(Box::new(StagedResource(r)))
}

// --- Components -----------------------------------------------------------------------------

pub(crate) fn component_codec<C: Persisted + Component>() -> Codec {
    Codec::Typed {
        encode: encode_component::<C>,
        decode: decode_component::<C>,
        remove: None,
    }
}

/// The rows in ascending `EntityId`: their count, then per row the id and the value.
fn write_rows<C: Serialize>(
    rows: &[(EntityId, &C)],
    out: &mut Vec<u8>,
) -> Result<(), (EntityId, PceError)> {
    write_uleb(out, rows.len() as u64);
    for (id, c) in rows {
        out.extend_from_slice(&id.get().to_le_bytes());
        pce::encode_into(*c, out, true).map_err(|e| (*id, e))?;
    }
    Ok(())
}

fn encode_component<C: Persisted + Component>(
    world: &World,
    out: &mut Vec<u8>,
    key: &SectionKey,
) -> Result<bool, Problem> {
    let Some(mut q) = world.try_query::<(&EntityId, &C)>() else {
        return Ok(false);
    };
    #[allow(clippy::disallowed_methods)] // sorted by EntityId below (simulation.md 8.1)
    let mut rows: Vec<(EntityId, &C)> = q.iter(world).map(|(id, c)| (*id, c)).collect();
    if rows.is_empty() {
        return Ok(false);
    }
    rows.sort_unstable_by_key(|r| r.0);
    write_rows(&rows, out)
        .map_err(|(id, e)| error::encode(&key.to_string(), Some(id.get()), &e.reason()))?;
    Ok(true)
}

struct StagedRows<C>(Vec<(EntityId, C)>);

impl<C: Component> StagedSection for StagedRows<C> {
    fn apply(self: Box<Self>, world: &mut World, ids: &mut IdMap) {
        let ids = &*ids;
        world.insert_batch(self.0.into_iter().map(|(id, c)| (ids.get(id), c)));
    }
}

/// Reads rows: ascending ids, each live (`persist.orphan`), at least one row.
pub(crate) fn read_rows<T>(
    bytes: &[u8],
    key: &SectionKey,
    live: &[EntityId],
    mut value: impl FnMut(&mut Decoder<'_>) -> Result<T, PceError>,
) -> Result<Vec<(EntityId, T)>, Problem> {
    let fail = |e: PceError| decode_problem(key, &e);
    let mut d = Decoder::new(bytes, true);
    let n = d.uleb().map_err(fail)?;
    if n == 0 {
        return Err(fail(PceError::noncanonical(
            0,
            "a component section without rows",
        )));
    }
    let mut rows: Vec<(EntityId, T)> =
        Vec::with_capacity(usize::try_from(n).unwrap_or(0).min(bytes.len() / 8));
    for _ in 0..n {
        let at = d.pos();
        let id: EntityId = d.value().map_err(fail)?;
        if rows.last().is_some_and(|(p, _)| *p >= id) {
            return Err(fail(PceError::noncanonical(
                at,
                "rows out of EntityId order",
            )));
        }
        if live.binary_search(&id).is_err() {
            return Err(error::orphan(key, id.get()));
        }
        let v = value(&mut d).map_err(|e| fail(e.at(d.pos())))?;
        rows.push((id, v));
    }
    d.finish().map_err(fail)?;
    Ok(rows)
}

fn decode_component<C: Persisted + Component>(
    bytes: &[u8],
    key: &SectionKey,
    live: &[EntityId],
) -> Result<Box<dyn StagedSection>, Problem> {
    let rows: Vec<(EntityId, C)> = read_rows(bytes, key, live, |d| d.value::<C>())?;
    let refs: Vec<(EntityId, &C)> = rows.iter().map(|(id, c)| (*id, c)).collect();
    let mut again = Vec::with_capacity(bytes.len());
    let again = write_rows(&refs, &mut again)
        .map(|()| again)
        .map_err(|(_, e)| e);
    canonical(key, bytes, again)?;
    Ok(Box::new(StagedRows(rows)))
}

// --- Caches ---------------------------------------------------------------------------------

pub(crate) fn cache_codec<K: PersistedCache>() -> Codec {
    Codec::Cache {
        encode: K::encode,
        decode: K::decode,
        rebuild: K::rebuild,
    }
}

pub(crate) struct StagedCache(pub(crate) Box<dyn Staged>);

impl StagedSection for StagedCache {
    fn apply(self: Box<Self>, world: &mut World, _: &mut IdMap) {
        self.0.apply(world);
    }
}
