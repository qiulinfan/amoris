//! Entity identity (docs/spec/simulation.md 7): `EntityId`, allocated by a persisted allocator in
//! the world, never reused, the same in every fork, restore and replay, and exact as a JavaScript
//! number. `bevy_ecs::Entity` never leaves this crate's world operations: it is not in components,
//! events, snapshots, hashes or anything a script sees.

mod index;
mod ops;

pub use index::EntityIndex;
pub use ops::{
    ReservedIds, SimCommands, despawn, entity, require, reserve, spawn, spawn_allocated,
};

use std::borrow::Cow;
use std::fmt;
use std::num::NonZeroU64;

use bevy_ecs::prelude::{Component, Resource};
use pocket_contract::{Problem, detail};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::json;

/// The largest id: 2^53 - 1, so every id is exact as a double. At a million spawns a second it
/// lasts 285 years.
pub const MAX_ENTITY_ID: u64 = (1 << 53) - 1;

/// A simulation entity's identity. Every simulation entity carries it as an immutable component,
/// so no system can change an entity's identity; components refer to other entities by it.
#[derive(Component, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[component(immutable)]
pub struct EntityId(NonZeroU64);

impl EntityId {
    /// The first id a world hands out.
    pub const FIRST: EntityId = EntityId(NonZeroU64::MIN);

    /// The id `id`, or `None` for 0 or above [`MAX_ENTITY_ID`].
    pub const fn new(id: u64) -> Option<EntityId> {
        if id > MAX_ENTITY_ID {
            return None;
        }
        match NonZeroU64::new(id) {
            Some(n) => Some(EntityId(n)),
            None => None,
        }
    }

    /// The id as an integer.
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// Exact: every id is a positive integer below 2^53.
    #[allow(clippy::cast_precision_loss)] // ids are below 2^53
    pub fn to_f64(self) -> f64 {
        self.0.get() as f64
    }

    /// From a script or JSON number: finite, integral, 1..=MAX_ENTITY_ID, else
    /// `sim.entity_id_invalid` (the script host reports its own `script.entity_missing`).
    pub fn from_f64(x: f64) -> Result<EntityId, Problem> {
        #[allow(clippy::cast_precision_loss)] // MAX_ENTITY_ID is exact in a double
        let max = MAX_ENTITY_ID as f64;
        if x.is_finite() && x == x.trunc() && (1.0..=max).contains(&x) {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked above
            let id = x as u64;
            if let Some(e) = EntityId::new(id) {
                return Ok(e);
            }
        }
        Err(Problem::new(
            "sim.entity_id_invalid",
            format!("{x} is not an entity id: ids are whole numbers from 1 to 2^53 - 1."),
            detail([("id", json!(x.to_string()))]),
        ))
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

impl Serialize for EntityId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_newtype_struct("EntityId", &self.0.get())
    }
}

impl<'de> Deserialize<'de> for EntityId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<EntityId, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = EntityId;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an entity id from 1 to 2^53 - 1")
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<EntityId, E> {
                EntityId::new(v).ok_or_else(|| E::invalid_value(de::Unexpected::Unsigned(v), &self))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<EntityId, E> {
                u64::try_from(v)
                    .ok()
                    .and_then(EntityId::new)
                    .ok_or_else(|| E::invalid_value(de::Unexpected::Signed(v), &self))
            }
            fn visit_newtype_struct<D: Deserializer<'de>>(
                self,
                d: D,
            ) -> Result<EntityId, D::Error> {
                let v = u64::deserialize(d)?;
                self.visit_u64(v)
            }
        }
        d.deserialize_newtype_struct("EntityId", V)
    }
}

impl JsonSchema for EntityId {
    fn schema_name() -> Cow<'static, str> {
        "EntityId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "integer",
            "minimum": 1,
            "maximum": MAX_ENTITY_ID,
            "description": "An entity's id: a whole number from 1 to 2^53 - 1, never reused."
        })
    }
}

/// The allocator: the id the next spawn receives, 1 in a new world. A persisted resource; decoding
/// refuses a `next` outside 1 to 2^53, as [`EntityAllocator::starting_at`] does.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug, Serialize, JsonSchema)]
pub struct EntityAllocator {
    next: u64,
}

impl<'de> Deserialize<'de> for EntityAllocator {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<EntityAllocator, D::Error> {
        /// The encoded shape, `EntityAllocator`'s own.
        #[derive(Deserialize)]
        #[serde(rename = "EntityAllocator")]
        struct Repr {
            next: u64,
        }
        let r = Repr::deserialize(d)?;
        EntityAllocator::starting_at(r.next).map_err(de::Error::custom)
    }
}

impl Default for EntityAllocator {
    fn default() -> Self {
        EntityAllocator { next: 1 }
    }
}

/// Where the allocator stood when an invocation began.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[must_use]
pub struct AllocMark(u64);

impl AllocMark {
    /// The id the allocator would have handed out next at the mark.
    pub fn next(self) -> u64 {
        self.0
    }
}

impl EntityAllocator {
    /// An allocator whose next id is `next` (1 to 2^53).
    pub fn starting_at(next: u64) -> Result<EntityAllocator, Problem> {
        if (1..=MAX_ENTITY_ID + 1).contains(&next) {
            Ok(EntityAllocator { next })
        } else {
            Err(entity_id_invalid(&next.to_string(), next))
        }
    }

    /// The id the next spawn receives.
    pub fn next(&self) -> u64 {
        self.next
    }

    /// The next id, or `sim.entity_ids_exhausted` past the bound.
    pub fn allocate(&mut self) -> Result<EntityId, Problem> {
        match EntityId::new(self.next) {
            Some(id) => {
                self.next += 1;
                Ok(id)
            }
            None => Err(Problem::new(
                "sim.entity_ids_exhausted",
                "The world has used every entity id up to 2^53 - 1.",
                detail([("next", json!(self.next))]),
            )),
        }
    }

    /// Taken when an invocation begins.
    pub fn mark(&self) -> AllocMark {
        AllocMark(self.next)
    }

    /// Returns a failed invocation's ids: they were never visible outside it.
    pub fn rollback(&mut self, mark: AllocMark) {
        self.next = mark.0;
    }

    /// Whether `id` was ever handed out.
    pub fn allocated(&self, id: EntityId) -> bool {
        id.get() < self.next
    }
}

/// The engine's plain-data name: at most 64 UTF-8 bytes, not necessarily unique; the shared
/// contract resolves an entity reference given as a string through it.
#[derive(Component, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, JsonSchema)]
pub struct Name(String);

impl<'de> Deserialize<'de> for Name {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Name, D::Error> {
        /// The encoded shape, `Name`'s own.
        #[derive(Deserialize)]
        #[serde(rename = "Name")]
        struct Repr(String);
        let Repr(name) = Repr::deserialize(d)?;
        Name::new(name).map_err(de::Error::custom)
    }
}

impl Name {
    /// Longest name in bytes.
    pub const MAX_BYTES: usize = 64;

    /// A name, or `sim.name_too_long` beyond 64 bytes.
    pub fn new(name: impl Into<String>) -> Result<Name, Problem> {
        let name = name.into();
        if name.len() > Self::MAX_BYTES {
            return Err(Problem::new(
                "sim.name_too_long",
                format!(
                    "A name is at most 64 bytes of UTF-8; this one has {}.",
                    name.len()
                ),
                detail([
                    ("bytes", json!(name.len())),
                    ("max", json!(Self::MAX_BYTES)),
                ]),
            ));
        }
        Ok(Name(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `sim.entity_id_invalid {id, next}`: an id that was never allocated, 0, or not an integer in range.
pub fn entity_id_invalid(id: &str, next: u64) -> Problem {
    Problem::new(
        "sim.entity_id_invalid",
        format!("{id} is not an entity id this world has given out; the next is {next}."),
        detail([("id", json!(id)), ("next", json!(next))]),
    )
}

/// `sim.entity_not_found {id}`: an operation needs a live entity and the id's entity is despawned.
pub fn entity_not_found(id: EntityId) -> Problem {
    Problem::new(
        "sim.entity_not_found",
        format!("Entity {} no longer exists.", id.get()),
        detail([("id", json!(id.get()))]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_from_numbers() {
        assert_eq!(EntityId::from_f64(1.0).unwrap().get(), 1);
        assert_eq!(
            EntityId::from_f64(9_007_199_254_740_991.0).unwrap().get(),
            MAX_ENTITY_ID
        );
        for bad in [
            0.0,
            1.5,
            -1.0,
            9_007_199_254_740_992.0,
            f64::NAN,
            f64::INFINITY,
        ] {
            assert_eq!(
                EntityId::from_f64(bad).unwrap_err().code,
                "sim.entity_id_invalid"
            );
        }
        assert!(EntityId::new(0).is_none());
        assert!(EntityId::new(MAX_ENTITY_ID + 1).is_none());
        assert_eq!(EntityId::new(7).unwrap().to_f64(), 7.0);
    }

    #[test]
    fn allocation_and_rollback() {
        let mut a = EntityAllocator::default();
        assert_eq!(a.allocate().unwrap().get(), 1);
        let m = a.mark();
        assert_eq!(a.allocate().unwrap().get(), 2);
        assert_eq!(a.allocate().unwrap().get(), 3);
        a.rollback(m);
        assert_eq!(a.allocate().unwrap().get(), 2);
        let mut end = EntityAllocator::starting_at(MAX_ENTITY_ID).unwrap();
        assert_eq!(end.allocate().unwrap().get(), MAX_ENTITY_ID);
        let e = end.allocate().unwrap_err();
        assert_eq!(e.code, "sim.entity_ids_exhausted");
        assert!(EntityAllocator::starting_at(0).is_err());
    }

    #[test]
    fn serde_forms() {
        let id = EntityId::new(42).unwrap();
        assert_eq!(serde_json::to_string(&id).unwrap(), "42");
        assert_eq!(serde_json::from_str::<EntityId>("42").unwrap(), id);
        assert!(serde_json::from_str::<EntityId>("0").is_err());
        assert!(serde_json::from_str::<EntityId>("9007199254740992").is_err());
        assert_eq!(serde_json::to_string(&Some(id)).unwrap(), "42");
        assert_eq!(std::mem::size_of::<Option<EntityId>>(), 8);
        assert!(Name::new("x".repeat(64)).is_ok());
        assert_eq!(
            Name::new("x".repeat(65)).unwrap_err().code,
            "sim.name_too_long"
        );
    }
}
