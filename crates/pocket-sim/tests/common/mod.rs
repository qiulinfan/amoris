//! Helpers shared by the integration tests: a small world with a test component, a hash over its
//! state in id order, and a recording of what systems saw.

#![allow(dead_code)]

use bevy_ecs::prelude::*;
use pocket_sim::rng::fnv1a64_extend;
use pocket_sim::{EntityId, EventCounter, EventInbox, Sim, SimClock, SimConfig, TickRate};
use serde::{Deserialize, Serialize};

/// A test component with a float that order-dependent rules change.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Value(pub f64);

/// What systems record, outside the simulated state (a test's own observation).
#[derive(Resource, Default, Debug, Clone, PartialEq)]
pub struct Seen(pub Vec<String>);

pub fn sim(seed: u64) -> Sim {
    let mut s = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed,
    })
    .unwrap();
    s.world_mut().insert_resource(Seen::default());
    s
}

pub fn seen(s: &Sim) -> Vec<String> {
    s.world().resource::<Seen>().0.clone()
}

/// A hash of the state this test cares about, visiting entities in id order: the clock, the
/// allocator, the event counter and inbox, and every entity's id and `Value` bits.
pub fn state_hash(s: &Sim) -> u64 {
    let w = s.world();
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    h = fnv1a64_extend(h, &w.resource::<SimClock>().tick.0.to_le_bytes());
    h = fnv1a64_extend(
        h,
        &w.resource::<pocket_sim::EntityAllocator>()
            .next()
            .to_le_bytes(),
    );
    h = fnv1a64_extend(h, &w.resource::<EventCounter>().next().to_le_bytes());
    for e in w.resource::<EventInbox>().events() {
        h = fnv1a64_extend(h, &e.seq.0.to_le_bytes());
        h = fnv1a64_extend(h, e.kind.as_str().as_bytes());
        h = fnv1a64_extend(h, serde_json::to_string(&e.data).unwrap().as_bytes());
    }
    for (id, entity) in w.resource::<pocket_sim::EntityIndex>().iter() {
        h = fnv1a64_extend(h, &id.get().to_le_bytes());
        if let Some(v) = w.get::<Value>(entity) {
            h = fnv1a64_extend(h, &v.0.to_bits().to_le_bytes());
        }
    }
    h
}

pub fn id(n: u64) -> EntityId {
    EntityId::new(n).unwrap()
}
