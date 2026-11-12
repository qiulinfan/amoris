//! Sensors (simulation.md 8.5): a collider whose `CollisionFilter` says `sensor` detects what
//! overlaps it and pushes nothing. Rapier detects the bodies that enter and leave it, during the
//! step, and keeps the overlapping pairs in its narrow phase, which the physics cache carries.
//! `Walker`s are no Rapier bodies, so `physics.contacts` tests each walker's box against the
//! sensors after the step and keeps what it overlaps in the walker's [`SensorOverlaps`], persisted
//! like any component, so a fork, a restore and a replay report the same entries and exits.

use amoris_sim::EntityId;
use bevy_ecs::prelude::Component;
use rapier3d::prelude::{Group, InteractionGroups, InteractionTestMode, QueryFilter};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::body::CollisionFilter;
use crate::geom;
use crate::solver::{Physics, entity_of};
use crate::walker::Walker;

/// The sensors a `Walker`'s box overlapped after the last step, ascending by id. Written by
/// `physics.contacts`, present only while the list is not empty; a walker that enters or leaves a
/// sensor is told by `physics.sensor_entered` and `physics.sensor_left`.
#[derive(Component, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SensorOverlaps {
    /// The sensors, ascending by id.
    pub sensors: Vec<EntityId>,
}

/// The solver's groups for a filter (`groups` and `mask` as Rapier's memberships and filter,
/// tested with both conditions).
pub(crate) fn groups_of(filter: &CollisionFilter) -> InteractionGroups {
    InteractionGroups::new(
        Group::from_bits_retain(filter.groups),
        Group::from_bits_retain(filter.mask),
        InteractionTestMode::And,
    )
}

/// The sensors that a walker of `id` with its eye at `eye` overlaps, ascending by id: every
/// sensor collider meeting the walker's filter (all groups without one) whose shape intersects
/// the walker's box. The walker's own colliders, if it has any, are left out.
pub(crate) fn overlapped(
    physics: &Physics,
    id: EntityId,
    walker: &Walker,
    eye: [f64; 3],
    filter: Option<&CollisionFilter>,
) -> Vec<EntityId> {
    let (lo, hi) = walker.bounds(eye);
    let centre: [f64; 3] = std::array::from_fn(|a| 0.5 * (lo[a] + hi[a]));
    let half: [f64; 3] = std::array::from_fn(|a| 0.5 * (hi[a] - lo[a]));
    if !(geom::finite32(&centre) && half.iter().all(|h| *h > 0.0 && geom::finite32(&[*h; 3]))) {
        return Vec::new();
    }
    let shape = rapier3d::parry::shape::Cuboid::new(geom::vec_in(half));
    let pose = rapier3d::math::Pose::from_translation(geom::vec_in(centre));
    let groups = groups_of(&filter.copied().unwrap_or_default());
    let not_own = |_, co: &rapier3d::prelude::Collider| entity_of(co.user_data) != Some(id);
    let mut out: Vec<EntityId> = physics
        .world
        .intersect_shape(
            pose,
            &shape,
            QueryFilter::default()
                .exclude_solids()
                .groups(groups)
                .predicate(&not_own),
        )
        .filter_map(|(_, co)| entity_of(co.user_data))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// What changed between two ascending lists: the ids only in `now` (entered), then those only in
/// `before` (left), each ascending.
pub(crate) fn changes(before: &[EntityId], now: &[EntityId]) -> (Vec<EntityId>, Vec<EntityId>) {
    let entered = now
        .iter()
        .filter(|s| before.binary_search(s).is_err())
        .copied()
        .collect();
    let left = before
        .iter()
        .filter(|s| now.binary_search(s).is_err())
        .copied()
        .collect();
    (entered, left)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(ns: &[u64]) -> Vec<EntityId> {
        ns.iter().map(|n| EntityId::new(*n).unwrap()).collect()
    }

    #[test]
    fn changes_name_what_entered_and_what_left() {
        let (entered, left) = changes(&ids(&[2, 5, 9]), &ids(&[1, 5, 7, 9]));
        assert_eq!(entered, ids(&[1, 7]));
        assert_eq!(left, ids(&[2]));
        let (entered, left) = changes(&[], &[]);
        assert!(entered.is_empty() && left.is_empty());
    }

    #[test]
    fn groups_meet_when_each_is_in_the_others_mask() {
        let a = CollisionFilter {
            groups: 0b01,
            mask: 0b10,
            ..CollisionFilter::default()
        };
        let b = CollisionFilter {
            groups: 0b10,
            mask: 0b01,
            ..CollisionFilter::default()
        };
        let c = CollisionFilter {
            groups: 0b10,
            mask: 0b10,
            ..CollisionFilter::default()
        };
        assert!(a.meets(&b) && b.meets(&a));
        assert!(!a.meets(&c) && !c.meets(&a), "a is in no group c meets");
        assert!(CollisionFilter::default().meets(&a));
        for (x, y) in [(a, b), (a, c), (b, c), (CollisionFilter::default(), c)] {
            assert_eq!(
                x.meets(&y),
                groups_of(&x).test(groups_of(&y)),
                "{x:?} {y:?}"
            );
        }
        let nothing = CollisionFilter {
            mask: 0,
            ..CollisionFilter::default()
        };
        assert!(!nothing.meets(&CollisionFilter::default()));
    }
}
