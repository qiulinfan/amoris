//! The physics state as world state (docs/spec/persistence.md 2 and 8; versions.md 3.6 and 6.3):
//! the persisted classes of this crate's types, the solver as one Cache section, and the
//! persisted names and traces of its components.
//!
//! The Cache's bytes are Rapier's `PhysicsWorld` through its own `serde` output, written by
//! bincode 1.3.3 (fixed-width little-endian integers, `u64` lengths, floats by their bits; trailing
//! bytes refused on decode): the encoding the physics spike wrote and restored natively and in the
//! browser to the same bytes. `pocket-physics` cannot link `pocket-persist`'s PCE (architecture.md
//! 5), and a cache section is opaque bytes (persistence.md 4.1), so the encoding is named in the
//! cache's identity instead.

use bevy_ecs::prelude::World;
use bincode::Options;
use pocket_contract::{Problem, detail};
use pocket_sim::persisted::{Persisted, PersistedCache, RegisterPersisted, Staged};
use rapier3d::prelude::PhysicsWorld;
use serde_json::json;
use serde_reflection::{Samples, Tracer};

use crate::boat::{Boat, Floater, Hull, Sail, Trim};
use crate::body::{
    BodyKind, Collider, ExternalForce, PartShape, RigidBody, Shape, Transform, Velocity,
};
use crate::sea::{Sea, Wind};
use crate::solver::{BodyIndex, ContactLog, Physics, entity_of, rebuild_index};

/// The cache's identity (versions.md 3.6): the library, its exact versions, the features that
/// change its serialized layout, the encoding, and this wrapper's layout number. A test checks the
/// versions against `Cargo.lock`.
pub const IDENTITY: &str = "rapier3d 0.36.0 parry3d 0.31.1 dim3 f32 serde-serialize \
                            enhanced-determinism bincode 1.3.3 layout 1";

/// Declares this crate's types (persistence.md 2): every component persisted, the solver a Cache
/// whose state, the resource [`Physics`], is Derived (restore removes it and the section's staged
/// apply puts it back; persistence.md 14, choice 15), the body index and the contact log Derived
/// (rebuilt by `physics.index` after a restore).
pub fn declare<R: RegisterPersisted>(r: &mut R) {
    r.component::<Transform>()
        .component::<Velocity>()
        .component::<RigidBody>()
        .component::<Collider>()
        .component::<ExternalForce>()
        .component::<Floater>()
        .component::<Boat>()
        .component::<Sail>()
        .component::<Hull>()
        .component::<Sea>()
        .component::<Wind>()
        .cache::<PhysicsCache>()
        .derived::<Physics>("the state of cache physics.rapier, put back by its section")
        .derived::<BodyIndex>("rebuilt from the bodies' user_data, which name their entities")
        .derived::<ContactLog>("filled by physics.step and emptied by physics.contacts each tick")
        .rebuild("physics.index", rebuild_index);
}

fn options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
}

/// The solver's section.
pub struct PhysicsCache;

/// A decoded solver, applied as the world's [`Physics`] (or none, for an empty section).
struct StagedPhysics(Option<Box<PhysicsWorld>>);

impl Staged for StagedPhysics {
    fn apply(self: Box<Self>, world: &mut World) {
        match self.0 {
            Some(w) => world.insert_resource(Physics { world: *w }),
            None => {
                world.remove_resource::<Physics>();
            }
        }
    }
}

fn encode_problem(reason: String) -> Problem {
    Problem::new(
        "persist.encode",
        format!("The physics cache could not be written: {reason}"),
        detail([
            ("section", json!(PhysicsCache::NAME)),
            ("entity", json!(null)),
            ("reason", json!(reason)),
        ]),
    )
}

fn noncanonical(reason: String) -> Problem {
    Problem::new(
        "persist.noncanonical",
        format!("The physics cache's bytes are not a solver this engine wrote: {reason}"),
        detail([("offset", json!(0)), ("reason", json!(reason))]),
    )
}

impl PersistedCache for PhysicsCache {
    const NAME: &'static str = "physics.rapier";

    fn identity() -> &'static str {
        IDENTITY
    }

    /// The solver's bytes; nothing when the world has no solver.
    fn encode(world: &World, out: &mut Vec<u8>) -> Result<(), Problem> {
        match world.get_resource::<Physics>() {
            Some(p) => options()
                .serialize_into(out, &p.world)
                .map_err(|e| encode_problem(e.to_string())),
            None => Ok(()),
        }
    }

    /// Decodes and checks that every body and collider names an entity.
    fn decode(bytes: &[u8]) -> Result<Box<dyn Staged>, Problem> {
        if bytes.is_empty() {
            return Ok(Box::new(StagedPhysics(None)));
        }
        let w: PhysicsWorld = options()
            .deserialize(bytes)
            .map_err(|e| noncanonical(e.to_string()))?;
        let bodies = w.bodies.iter().map(|(_, b)| b.user_data);
        let colliders = w.colliders.iter().map(|(_, c)| c.user_data);
        if let Some(bad) = bodies.chain(colliders).find(|u| entity_of(*u).is_none()) {
            return Err(noncanonical(format!(
                "a body or collider names no entity (user data {bad:#x})"
            )));
        }
        Ok(Box::new(StagedPhysics(Some(Box::new(w)))))
    }

    /// Rebuilds the solver from the components (versions.md 6.3): a new one, every body made at the
    /// next step from its `RigidBody`, `Collider`, `Transform` and `Velocity`, in id order. Contacts
    /// restart without warm-start impulses, so it continues close to, not exactly as, the old one.
    fn rebuild(world: &mut World) -> Result<(), Problem> {
        let rate = world.resource::<pocket_sim::SimClock>().dt();
        world.insert_resource(Physics::new(rate));
        rebuild_index(world);
        Ok(())
    }
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
    Transform => "Transform",
    Velocity => "Velocity",
    ExternalForce => "ExternalForce",
    Floater => "Floater",
    Sail => "Sail",
    Hull => "Hull",
    Sea => "Sea",
    Wind => "Wind",
}

impl Persisted for RigidBody {
    const NAME: &'static str = "RigidBody";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<BodyKind>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

impl Persisted for Collider {
    const NAME: &'static str = "Collider";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<PartShape>(s)?;
        t.trace_type::<Shape>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

impl Persisted for Boat {
    const NAME: &'static str = "Boat";
    const VERSION: u32 = 1;

    fn trace(t: &mut Tracer, s: &Samples) -> serde_reflection::Result<()> {
        t.trace_type::<Trim>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocket_sim::persisted::{record_samples, tracer_config};

    /// Every persisted type traces with `serde-reflection` and the registry resolves completely
    /// (persistence.md 3.5 and 6.2): what `pocket-persist` does to fingerprint them.
    #[test]
    fn every_persisted_type_traces() {
        let mut t = Tracer::new(tracer_config());
        let mut s = Samples::new();
        record_samples(&mut t, &mut s).unwrap();
        Transform::trace(&mut t, &s).unwrap();
        Velocity::trace(&mut t, &s).unwrap();
        RigidBody::trace(&mut t, &s).unwrap();
        Collider::trace(&mut t, &s).unwrap();
        ExternalForce::trace(&mut t, &s).unwrap();
        Floater::trace(&mut t, &s).unwrap();
        Boat::trace(&mut t, &s).unwrap();
        Sail::trace(&mut t, &s).unwrap();
        Hull::trace(&mut t, &s).unwrap();
        Sea::trace(&mut t, &s).unwrap();
        Wind::trace(&mut t, &s).unwrap();
        let reg = t.registry().unwrap();
        for name in [
            "Transform",
            "Velocity",
            "RigidBody",
            "BodyKind",
            "MassProps",
            "Collider",
            "Shape",
            "PartShape",
            "Part",
            "ExternalForce",
            "Floater",
            "BuoyPoint",
            "Boat",
            "Trim",
            "Sail",
            "Hull",
            "Sea",
            "Wave",
            "Wind",
        ] {
            assert!(
                reg.contains_key(name),
                "{name} missing from {:?}",
                reg.keys()
            );
        }
    }
}
