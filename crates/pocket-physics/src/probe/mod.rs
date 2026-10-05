//! This crate's own checks of determinism and forks (the web tests and the integration tests),
//! without `pocket-persist`, which `pocket-physics` cannot link (architecture.md 5).
//!
//! [`Ledger`] implements `RegisterPersisted` over the declarations of `pocket_sim::persisted` and
//! this crate's [`crate::declare`], so the checks see exactly the world state those declarations
//! name: the entities, every persisted resource and component, and the physics cache through its
//! own `encode` and `decode`. A [`Snap`] is their bytes in declaration order (bincode for
//! resources and components; the cache's own bytes), its hash FNV-1a over them, and [`restore`]
//! puts a snapshot into a fresh world and runs the declared rebuilds. A state the declarations
//! leave out makes a fork diverge, which is what the checks look for. It is a stand-in for the
//! hash and fork of persistence.md (PCE, XXH3-128), not their implementation.

mod scenes;

pub use scenes::{
    CHAIN_TICKS, GOLDEN, boat_position, chain, chain_report, check_bvh_log2, check_chain,
    check_forks, crew, fresh, golden_line, sailboat_world, sailing_world,
};

use bevy_ecs::prelude::{Component, Entity, Resource, World};
use pocket_contract::Problem;
use pocket_sim::persisted::{Persisted, PersistedCache, RegisterPersisted, Staged};
use pocket_sim::rng::fnv1a64_extend;
use pocket_sim::{EntityAllocator, EntityId, EntityIndex, Sim};
use serde::Serialize;
use serde::de::DeserializeOwned;

type Encode = fn(&World) -> Result<Option<Vec<u8>>, String>;
type Rebuild = (&'static str, fn(&mut World));
type Restore = fn(&mut World, &[u8], &[(EntityId, Entity)]) -> Result<(), String>;

enum Entry {
    Entities,
    Section {
        name: &'static str,
        encode: Encode,
        restore: Restore,
    },
}

/// The declarations, as `pocket-persist`'s registry builder would receive them.
#[derive(Default)]
pub struct Ledger {
    entries: Vec<Entry>,
    rebuilds: Vec<Rebuild>,
    /// Every name declared, persisted or not, for the checks' listings.
    pub names: Vec<(&'static str, &'static str)>,
}

fn bytes<T: Serialize>(v: &T) -> Result<Vec<u8>, String> {
    bincode::serialize(v).map_err(|e| e.to_string())
}

fn value<T: DeserializeOwned>(b: &[u8]) -> Result<T, String> {
    bincode::deserialize(b).map_err(|e| e.to_string())
}

fn encode_resource<R: Persisted + Resource>(w: &World) -> Result<Option<Vec<u8>>, String> {
    w.get_resource::<R>().map(bytes).transpose()
}

fn restore_resource<R: Persisted + Resource>(
    w: &mut World,
    b: &[u8],
    _: &[(EntityId, Entity)],
) -> Result<(), String> {
    w.insert_resource(value::<R>(b)?);
    Ok(())
}

/// Rows in ascending `EntityId`: the id and the value.
fn encode_component<C: Persisted + Component>(w: &World) -> Result<Option<Vec<u8>>, String> {
    let mut rows: Vec<(u64, &C)> = Vec::new();
    for (id, e) in w.resource::<EntityIndex>().iter() {
        if let Some(c) = w.get::<C>(e) {
            rows.push((id.get(), c));
        }
    }
    if rows.is_empty() {
        return Ok(None);
    }
    bytes(&rows).map(Some)
}

fn restore_component<C: Persisted + Component>(
    w: &mut World,
    b: &[u8],
    live: &[(EntityId, Entity)],
) -> Result<(), String> {
    for (id, c) in value::<Vec<(u64, C)>>(b)? {
        let i = live
            .binary_search_by_key(&id, |x| x.0.get())
            .map_err(|_| format!("a row of {} names {id}, which is not live", C::NAME))?;
        w.entity_mut(live[i].1).insert(c);
    }
    Ok(())
}

fn encode_cache<K: PersistedCache>(w: &World) -> Result<Option<Vec<u8>>, String> {
    let mut out = Vec::new();
    K::encode(w, &mut out).map_err(|p: Problem| p.message)?;
    Ok((!out.is_empty()).then_some(out))
}

fn restore_cache<K: PersistedCache>(
    w: &mut World,
    b: &[u8],
    _: &[(EntityId, Entity)],
) -> Result<(), String> {
    let staged: Box<dyn Staged> = K::decode(b).map_err(|p| p.message)?;
    staged.apply(w);
    Ok(())
}

impl RegisterPersisted for Ledger {
    fn entities(&mut self) -> &mut Self {
        self.names.push(("entities", "Entities"));
        self.entries.push(Entry::Entities);
        self
    }

    fn resource<R: Persisted + Resource>(&mut self) -> &mut Self {
        self.names.push((R::NAME, "Resource"));
        self.entries.push(Entry::Section {
            name: R::NAME,
            encode: encode_resource::<R>,
            restore: restore_resource::<R>,
        });
        self
    }

    fn component<C: Persisted + Component>(&mut self) -> &mut Self {
        self.names.push((C::NAME, "Component"));
        self.entries.push(Entry::Section {
            name: C::NAME,
            encode: encode_component::<C>,
            restore: restore_component::<C>,
        });
        self
    }

    fn cache<K: PersistedCache>(&mut self) -> &mut Self {
        self.names.push((K::NAME, "Cache"));
        self.entries.push(Entry::Section {
            name: K::NAME,
            encode: encode_cache::<K>,
            restore: restore_cache::<K>,
        });
        self
    }

    fn derived<T: 'static>(&mut self, _reason: &'static str) -> &mut Self {
        self.names.push((std::any::type_name::<T>(), "Derived"));
        self
    }

    fn ignore<T: 'static>(&mut self, _reason: &'static str) -> &mut Self {
        self.names.push((std::any::type_name::<T>(), "Ignored"));
        self
    }

    fn rebuild(&mut self, name: &'static str, f: fn(&mut World)) -> &mut Self {
        self.rebuilds.push((name, f));
        self
    }
}

/// The ledger of `pocket-sim`'s and this crate's declarations.
pub fn ledger() -> Ledger {
    let mut l = Ledger::default();
    pocket_sim::persisted::declare(&mut l);
    crate::declare(&mut l);
    l
}

/// A world's persisted state: each section's name and bytes, in declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snap {
    pub sections: Vec<(&'static str, Vec<u8>)>,
}

impl Snap {
    /// FNV-1a 64 over every section's name, length and bytes.
    pub fn hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for (name, b) in &self.sections {
            h = fnv1a64_extend(h, name.as_bytes());
            h = fnv1a64_extend(h, &(b.len() as u64).to_le_bytes());
            h = fnv1a64_extend(h, b);
        }
        h
    }

    /// The bytes of one section.
    pub fn section(&self, name: &str) -> Option<&[u8]> {
        self.sections
            .iter()
            .find(|s| s.0 == name)
            .map(|s| s.1.as_slice())
    }
}

impl Ledger {
    /// The world's persisted state at a boundary.
    pub fn snapshot(&self, w: &World) -> Result<Snap, String> {
        let mut sections = Vec::new();
        for e in &self.entries {
            match e {
                Entry::Entities => {
                    let ids: Vec<u64> = w
                        .resource::<EntityIndex>()
                        .ids()
                        .map(EntityId::get)
                        .collect();
                    sections.push(("entities", bytes(&(w.resource::<EntityAllocator>(), ids))?));
                }
                Entry::Section { name, encode, .. } => {
                    if let Some(b) = encode(w)? {
                        sections.push((*name, b));
                    }
                }
            }
        }
        Ok(Snap { sections })
    }

    /// Puts `snap` into `sim`, a fresh world with the schedule (no entities), and runs the
    /// declared rebuilds in order.
    pub fn restore(&self, sim: &mut Sim, snap: &Snap) -> Result<(), String> {
        let w = sim.world_mut();
        let (alloc, ids): (EntityAllocator, Vec<u64>) =
            value(snap.section("entities").ok_or("no entities section")?)?;
        w.insert_resource(alloc);
        let mut live = Vec::with_capacity(ids.len());
        for id in ids {
            let id = EntityId::new(id).ok_or("entity id 0")?;
            live.push((id, w.spawn(id).id()));
        }
        for e in &self.entries {
            if let Entry::Section { name, restore, .. } = e
                && let Some(b) = snap.section(name)
            {
                restore(w, b, &live).map_err(|m| format!("{name}: {m}"))?;
            }
        }
        for (_, f) in &self.rebuilds {
            f(w);
        }
        Ok(())
    }
}

/// `snapshot` then `restore` into `fresh()`: a fork.
pub fn fork(ledger: &Ledger, sim: &Sim, fresh: fn() -> Sim) -> Result<Sim, String> {
    let snap = ledger.snapshot(sim.world())?;
    let mut out = fresh();
    ledger.restore(&mut out, &snap)?;
    Ok(out)
}
