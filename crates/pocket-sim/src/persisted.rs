//! The per-type persistence hooks (docs/spec/persistence.md 6.1 and 6.2), declared here so that
//! physics, assets and scripts register state and name content without depending on
//! `pocket-persist`, which defines their contracts and implements the whole-world operations over
//! them (architecture.md 2, principle 6).
//!
//! Every type that can appear in the world is in exactly one class (persistence.md 2): Resource,
//! Component and Cache are snapshotted and hashed; Derived is rebuilt after a restore; Ignored
//! carries no game state. A crate declares its types' classes through [`RegisterPersisted`], which
//! `pocket-persist`'s registry builder implements; [`declare`] is this crate's declaration.

use std::fmt;

use bevy_ecs::prelude::{Component, Resource, World};
use pocket_contract::Problem;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_reflection::{Samples, Tracer, TracerConfig};

use crate::entity::{EntityAllocator, EntityId, EntityIndex, Name, ReservedIds};
use crate::event::{EventCounter, EventInbox, EventKind, EventOutbox};
use crate::registry::ComponentRegistry;
use crate::rng::{RngTable, WorldSeed};
use crate::sim::{Poisoned, TickOutput, TickState};
use crate::time::{SimClock, TickRate};

/// A persisted Resource or Component type: its stable section name, its schema version, and how
/// `serde-reflection` traces its format (for the fingerprint, versions.md 3.5).
pub trait Persisted: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// The stable section name, `[A-Za-z][A-Za-z0-9_:.]{0,63}` (persistence.md 4.1).
    const NAME: &'static str;
    /// The schema version, from 1 (versions.md 3.5).
    const VERSION: u32;

    /// Traces the type's format. The default traces `Self` with the given samples; a type holding
    /// an enum below its top level traces that enum first (serde-reflection explores only the
    /// top-level enum's variants by itself). The tracer should be made with [`tracer_config`] and
    /// `samples` come from [`record_samples`].
    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<Self>(s).map(|_| ())
    }
}

/// A library's state across ticks that changes results, kept as opaque bytes (a Cache section,
/// persistence.md 8): the physics solver.
pub trait PersistedCache: Send + Sync + 'static {
    /// The stable section name.
    const NAME: &'static str;
    /// The cache's identity: the library and layout its bytes come from (versions.md 3.6).
    fn identity() -> &'static str;
    /// Writes the cache's state.
    fn encode(world: &World, out: &mut Vec<u8>) -> Result<(), Problem>;
    /// Decodes and validates bytes without touching a world.
    fn decode(bytes: &[u8]) -> Result<Box<dyn Staged>, Problem>;
    /// Rebuilds the cache, inexactly, from the Component sections (versions.md 6.3).
    fn rebuild(world: &mut World) -> Result<(), Problem>;
}

/// A decoded, validated section not yet applied.
pub trait Staged: Send {
    fn apply(self: Box<Self>, world: &mut World);
}

/// How a crate declares its types' classes. `pocket-persist`'s `RegistryBuilder` implements it, so
/// crates below persistence declare without depending on it.
pub trait RegisterPersisted {
    /// The Entities section: the `EntityAllocator` and the live `EntityId`s.
    fn entities(&mut self) -> &mut Self;
    fn resource<R: Persisted + Resource>(&mut self) -> &mut Self;
    fn component<C: Persisted + Component>(&mut self) -> &mut Self;
    fn cache<K: PersistedCache>(&mut self) -> &mut Self;
    /// Recomputed from persisted state with identical results at any boundary.
    fn derived<T: 'static>(&mut self, reason: &'static str) -> &mut Self;
    /// Carries no game state.
    fn ignore<T: 'static>(&mut self, reason: &'static str) -> &mut Self;
    /// Run after a restore, in registration order.
    fn rebuild(&mut self, name: &'static str, f: fn(&mut World)) -> &mut Self;
}

/// Declares this crate's types: the entities, the clock, the seed, the event counter and inbox and
/// `Name` as persisted; the index, the RNG table, the outbox and the tick's scratch state as
/// derived (rebuilt by [`rebuild_derived`]); the component registry as ignored.
pub fn declare<R: RegisterPersisted>(r: &mut R) {
    r.entities()
        .resource::<SimClock>()
        .resource::<WorldSeed>()
        .resource::<EventCounter>()
        .resource::<EventInbox>()
        .component::<Name>()
        .derived::<EntityIndex>("rebuilt from the EntityId components in one sorted pass")
        .derived::<RngTable>("empty at every boundary; streams derive from the seed and the tick")
        .derived::<ReservedIds>("a reservation ends with its invocation, tick or boundary write")
        .derived::<EventOutbox>("empty at every boundary; tick n's events move to the inbox")
        .derived::<TickState>("no tick or invocation runs at a boundary")
        .derived::<TickOutput>("taken by each step's report; empty at every boundary")
        .derived::<Poisoned>("a restored world is not poisoned")
        .ignore::<ComponentRegistry>("schema metadata the world builder registers")
        .rebuild("sim.derived", rebuild_derived);
}

/// Puts back the derived state after a restore: the entity index from the `EntityId`s, and the
/// empty RNG table, outbox and tick state.
pub fn rebuild_derived(world: &mut World) {
    EntityIndex::rebuild(world);
    world.insert_resource(ReservedIds::default());
    world.insert_resource(RngTable::default());
    world.insert_resource(EventOutbox::default());
    world.insert_resource(TickState::default());
    world.insert_resource(TickOutput::default());
    world.remove_resource::<Poisoned>();
}

/// The tracer configuration persisted types need: serde-reflection's default, plus samples for
/// structs, since two of this crate's (`EntityAllocator` and `EventCounter`) refuse the 0 the
/// tracer would offer.
pub fn tracer_config() -> TracerConfig {
    TracerConfig::default().record_samples_for_structs(true)
}

/// Records the sample values tracing needs for this crate's types whose `Deserialize` refuses the
/// tracer's own values (0 and the empty string): an `EntityId` and an allocator are never 0, a
/// rate is 1 to 1000, an event counter starts at 1 and an event kind has a dot. A struct's sample
/// takes effect only with [`tracer_config`].
pub fn record_samples(t: &mut Tracer, s: &mut Samples) -> serde_reflection::Result<()> {
    t.trace_value(s, &EntityId::FIRST)?;
    t.trace_value(s, &TickRate::DEFAULT)?;
    t.trace_value(s, &EntityAllocator::default())?;
    t.trace_value(s, &EventCounter::default())?;
    if let Ok(kind) = EventKind::new("sample.kind") {
        t.trace_value(s, &kind)?;
    }
    Ok(())
}

macro_rules! persisted {
    ($($t:ty => $name:literal),* $(,)?) => {
        $(impl Persisted for $t {
            const NAME: &'static str = $name;
            const VERSION: u32 = 1;
        })*
    };
}

persisted! {
    SimClock => "SimClock",
    WorldSeed => "WorldSeed",
    EventCounter => "EventCounter",
    Name => "Name",
}

impl Persisted for EventInbox {
    const NAME: &'static str = "EventInbox";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<crate::data::PlainData>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

/// A content hash: BLAKE3-256 through `derive_key` with a dated context string per use (bundles,
/// data files, engine sources; persistence.md 5.1, versions.md 3).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    /// `BLAKE3-derive_key(context, bytes)`.
    pub fn derive(context: &str, bytes: &[u8]) -> ContentHash {
        ContentHash(blake3::derive_key(context, bytes))
    }

    /// An incremental hasher for content written in pieces.
    pub fn hasher(context: &str) -> ContentHasher {
        ContentHasher(blake3::Hasher::new_derive_key(context))
    }

    /// 64 lowercase hex digits.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// From 64 hex digits.
    pub fn from_hex(s: &str) -> Option<ContentHash> {
        if s.len() != 64 || !s.is_ascii() {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, o) in out.iter_mut().enumerate() {
            *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(ContentHash(out))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", self.to_hex())
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Hashes content written in pieces under one context.
pub struct ContentHasher(blake3::Hasher);

impl ContentHasher {
    pub fn update(&mut self, bytes: &[u8]) -> &mut ContentHasher {
        self.0.update(bytes);
        self
    }

    pub fn finish(&self) -> ContentHash {
        ContentHash(*self.0.finalize().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_is_derive_key() {
        let a = ContentHash::derive("Pocket3D 2026-10-03 test v1", b"hello");
        let mut h = ContentHash::hasher("Pocket3D 2026-10-03 test v1");
        h.update(b"hel").update(b"lo");
        assert_eq!(h.finish(), a);
        assert_eq!(ContentHash::from_hex(&a.to_hex()), Some(a));
        assert_ne!(
            a,
            ContentHash::derive("Pocket3D 2026-10-03 other v1", b"hello")
        );
        assert!(ContentHash::from_hex("zz").is_none());
    }

    #[test]
    fn every_persisted_type_traces() {
        let mut t = Tracer::new(tracer_config());
        let mut s = Samples::new();
        record_samples(&mut t, &mut s).unwrap();
        SimClock::trace(&mut t, &s).unwrap();
        WorldSeed::trace(&mut t, &s).unwrap();
        EventCounter::trace(&mut t, &s).unwrap();
        EventInbox::trace(&mut t, &s).unwrap();
        Name::trace(&mut t, &s).unwrap();
        t.trace_type::<EntityAllocator>(&s).unwrap();
        let reg = t.registry().unwrap();
        for name in [
            "SimClock",
            "Tick",
            "TickRate",
            "WorldSeed",
            "EntityAllocator",
            "EventCounter",
            "EventInbox",
            "Event",
            "EventKind",
            "PlainData",
            "EntityId",
            "Name",
        ] {
            assert!(
                reg.contains_key(name),
                "{name} missing from {:?}",
                reg.keys()
            );
        }
    }

    /// Decoding is strict (persistence.md 3.6): what a constructor refuses, `Deserialize` refuses,
    /// so a snapshot or replay cannot bring in a rate of 0 or an inbox mark past its events.
    #[test]
    fn decoding_checks_what_constructors_check() {
        use serde_json::from_str;
        let refused = |r: Result<(), serde_json::Error>, what: &str| {
            assert!(r.is_err(), "{what} decoded");
        };
        let clock = |rate: u32| format!(r#"{{"tick":3,"rate":{rate}}}"#);
        assert_eq!(from_str::<SimClock>(&clock(60)).unwrap().rate, TickRate(60));
        refused(from_str::<SimClock>(&clock(0)).map(drop), "rate 0");
        refused(from_str::<SimClock>(&clock(1001)).map(drop), "rate 1001");
        refused(
            from_str::<SimClock>(r#"{"tick":9007199254740992,"rate":60}"#).map(drop),
            "tick 2^53",
        );
        assert_eq!(
            from_str::<WorldSeed>("9007199254740991").unwrap().0,
            (1 << 53) - 1
        );
        refused(
            from_str::<WorldSeed>("9007199254740992").map(drop),
            "seed 2^53",
        );
        assert_eq!(
            from_str::<EntityAllocator>(r#"{"next":1}"#).unwrap().next(),
            1
        );
        refused(
            from_str::<EntityAllocator>(r#"{"next":0}"#).map(drop),
            "next 0",
        );
        refused(
            from_str::<EntityAllocator>(r#"{"next":9007199254740993}"#).map(drop),
            "next 2^53 + 1",
        );
        refused(
            from_str::<EventCounter>(r#"{"next":0}"#).map(drop),
            "counter 0",
        );
        assert!(from_str::<Name>(&format!("\"{}\"", "x".repeat(64))).is_ok());
        refused(
            from_str::<Name>(&format!("\"{}\"", "x".repeat(65))).map(drop),
            "a 65-byte name",
        );
        refused(from_str::<EventKind>(r#""Bad""#).map(drop), "kind Bad");
        refused(
            from_str::<EventInbox>(r#"{"events":[],"boundary_from":5}"#).map(drop),
            "an inbox mark past its events",
        );
        let event = |seq: u64, data: &str| {
            format!(
                r#"{{"seq":{seq},"tick":1,"kind":"a.b","subject":null,"cause":null,"data":{data}}}"#
            )
        };
        let inbox = |events: &[String], from: u32| {
            format!(
                r#"{{"events":[{}],"boundary_from":{from}}}"#,
                events.join(",")
            )
        };
        let good = inbox(&[event(1, r#""Null""#), event(2, r#"{"Number":1.5}"#)], 1);
        let decoded = from_str::<EventInbox>(&good).unwrap();
        assert_eq!(decoded.tick_events().len(), 1);
        assert_eq!(decoded.boundary_events().len(), 1);
        refused(
            from_str::<EventInbox>(&inbox(&[event(2, r#""Null""#), event(2, r#""Null""#)], 0))
                .map(drop),
            "repeated sequence numbers",
        );
        let unsorted = r#"{"Object":[["b","Null"],["a","Null"]]}"#;
        refused(
            from_str::<EventInbox>(&inbox(&[event(1, unsorted)], 0)).map(drop),
            "unsorted object keys",
        );
    }
}
