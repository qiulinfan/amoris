//! Current-pose resident observations for Walker. These are derived request receipts, never
//! checkpoint shadows: restoration clears them together with the unfinished water continuation.
//! A sleeping, unchanged field may rebind ordinary column data to a new pose in the same query
//! column. This is not a fresh GPU sample: the field identity and query are unchanged, and the
//! bound pose/tick identify the current consumer. Coupled Floater samples have their own cache.
use std::collections::{BTreeMap, BTreeSet};

use amoris_assets::water_field::FieldRecipe;
use amoris_contract::Problem;
use amoris_sim::{ContentHash, EntityId};
use bevy_ecs::prelude::Resource;

use crate::body::Transform;

#[derive(Clone, Debug, PartialEq)]
struct Grid {
    origin: [f64; 2],
    cell: f64,
    size: [u32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Point {
    pub actor: EntityId,
    pose: Transform,
    pub grid: [f64; 2],
    field: Grid,
}
impl Point {
    pub fn new(
        actor: EntityId,
        pose: Transform,
        origin: [f64; 3],
        region: &super::WaterRegion,
    ) -> Result<Option<Self>, Problem> {
        validate_pose(&pose)?;
        let grid = [
            (pose.position[0] - origin[0]) / region.cell_m,
            (pose.position[2] - origin[2]) / region.cell_m,
        ];
        if origin.iter().any(|v| !v.is_finite())
            || !region.cell_m.is_finite()
            || region.cell_m <= 0.0
            || grid.iter().any(|v| !v.is_finite())
        {
            return Err(failed("resident Walker sampling grid must be finite"));
        }
        Ok(grid
            .iter()
            .zip(region.size)
            .all(|(&q, n)| q >= 0.0 && q < f64::from(n))
            .then_some(Self {
                actor,
                pose,
                // Preserve Walker's authored column contact semantics. The common GPU sampler
                // is bilinear at arbitrary points; a centre request returns this exact column.
                grid: grid.map(|q| q.floor() + 0.5),
                field: Grid {
                    origin: [origin[0], origin[2]],
                    cell: region.cell_m,
                    size: region.size,
                },
            }))
    }
}

struct Observation {
    point: Point,
    region: EntityId,
    digest: [u8; 32],
    checkpoint: ContentHash,
    // The completed tick to which this column data is bound, not a GPU callback/sample age.
    tick: u64,
    column: super::WaterColumn,
}
#[derive(Resource, Default)]
pub(crate) struct Samples {
    rows: BTreeMap<EntityId, Observation>,
}
impl Samples {
    pub fn retain(&mut self, live: &BTreeSet<EntityId>) {
        self.rows.retain(|key, _| live.contains(key));
    }
    pub fn matches(&self, point: &Point, region: EntityId, recipe: &FieldRecipe) -> bool {
        self.rows.get(&point.actor).is_some_and(|old| {
            old.point == *point
                && old.region == region
                && old.digest == recipe.digest
                && old.checkpoint == recipe.checkpoint()
        })
    }
    /// Eligibility of ordinary column data, deliberately independent of the old actor pose.
    /// Only a nonadvancing sleeping plan may use this to avoid a fresh query. `get` still
    /// authenticates the complete current pose after the plan commits this binding.
    pub fn can_rebind(&self, point: &Point, region: EntityId, recipe: &FieldRecipe) -> bool {
        self.rows.get(&point.actor).is_some_and(|old| {
            old.point.actor == point.actor
                && old.point.grid == point.grid
                && old.point.field == point.field
                && old.region == region
                && old.digest == recipe.digest
                && old.checkpoint == recipe.checkpoint()
        })
    }
    pub fn reuse(
        &mut self,
        point: &Point,
        region: EntityId,
        recipe: &FieldRecipe,
        tick: u64,
    ) -> Result<(), Problem> {
        if !self.can_rebind(point, region, recipe) {
            return Err(failed(
                "sleeping Walker column data does not match its binding",
            ));
        }
        let old = self.rows.get_mut(&point.actor).expect("checked binding");
        old.point = point.clone();
        old.tick = tick;
        Ok(())
    }
    pub fn record(
        &mut self,
        point: Point,
        region: EntityId,
        recipe: &FieldRecipe,
        tick: u64,
        column: super::WaterColumn,
    ) {
        self.rows.insert(
            point.actor,
            Observation {
                point,
                region,
                digest: recipe.digest,
                checkpoint: recipe.checkpoint(),
                tick,
                column,
            },
        );
    }
    pub fn get(
        &self,
        point: &Point,
        region: EntityId,
        recipe: &FieldRecipe,
        tick: u64,
    ) -> Option<super::WaterColumn> {
        self.matches(point, region, recipe)
            .then(|| self.rows.get(&point.actor))
            .flatten()
            .filter(|old| old.tick == tick)
            .map(|old| old.column)
    }
}
pub(crate) fn validate_pose(pose: &Transform) -> Result<(), Problem> {
    if pose
        .position
        .iter()
        .chain(&pose.rotation)
        .any(|v| !v.is_finite())
    {
        Err(failed("Walker sampling pose must be finite"))
    } else {
        Ok(())
    }
}
pub(crate) fn failed(message: impl Into<String>) -> Problem {
    Problem::new(
        "water.swimming_failed",
        message.into(),
        amoris_contract::detail([]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use amoris_assets::water_field::FieldCheckpoint;

    #[test]
    fn identity_requires_current_pose_grid_region_field_and_tick() {
        let region = super::super::dam_scene(1);
        let actor = EntityId::FIRST;
        let id = EntityId::new(2).unwrap();
        let pose = Transform::at([1.0, 2.0, 1.0]);
        let point = Point::new(actor, pose, [0.0; 3], &region).unwrap().unwrap();
        let seed = FieldCheckpoint::new([2, 2], 0.0, vec![[0; 4]; 4]);
        let recipe = FieldRecipe::new(seed.key(), vec![], seed.digest(), String::new());
        let mut samples = Samples::default();
        samples.record(
            point.clone(),
            id,
            &recipe,
            7,
            [2.0, 2.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        );
        assert!(samples.get(&point, id, &recipe, 7).is_some());
        assert!(samples.get(&point, id, &recipe, 8).is_none());
        for axis in 0..3 {
            let mut moved = point.clone();
            moved.pose.position[axis] += 0.1;
            assert!(!samples.matches(&moved, id, &recipe));
        }
        let mut rotated = point.clone();
        rotated.pose.rotation[1] = 0.1;
        assert!(!samples.matches(&rotated, id, &recipe));
        let mut grid = point.clone();
        grid.field.cell *= 2.0;
        assert!(!samples.matches(&grid, id, &recipe));
        assert!(!samples.matches(&point, actor, &recipe));
        let mut changed = recipe.clone();
        changed.digest[0] ^= 1;
        assert!(!samples.matches(&point, id, &changed));
        let checkpoint = FieldCheckpoint::new([2, 2], 1.0, vec![[0; 4]; 4]);
        let changed = FieldRecipe::new(checkpoint.key(), vec![], recipe.digest, String::new());
        assert!(!samples.matches(&point, id, &changed));
        samples.reuse(&point, id, &recipe, 8).unwrap();
        assert!(samples.get(&point, id, &recipe, 8).is_some());
        let mut world = bevy_ecs::world::World::new();
        world.insert_resource(samples);
        super::super::compute::invalidate(&mut world);
        assert!(
            world
                .resource::<Samples>()
                .get(&point, id, &recipe, 8)
                .is_none(),
            "restoration hook discards every derived observation"
        );
        let mut samples = Samples::default();
        samples.record(
            point.clone(),
            id,
            &recipe,
            8,
            [2.0, 2.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        );
        samples.retain(&BTreeSet::new());
        assert!(samples.get(&point, id, &recipe, 8).is_none());
    }

    #[test]
    fn sleeping_rebind_authenticates_column_data_then_binds_the_complete_pose() {
        let mut region = super::super::dam_scene(1);
        region.cell_m = 1.0;
        let actor = EntityId::FIRST;
        let id = EntityId::new(2).unwrap();
        let old = Point::new(actor, Transform::at([1.2, 1.7, 1.2]), [0.0; 3], &region)
            .unwrap()
            .unwrap();
        let point = Point::new(
            actor,
            Transform::at_yaw([1.8, 3.7, 1.8], 60.0),
            [0.0; 3],
            &region,
        )
        .unwrap()
        .unwrap();
        let seed = FieldCheckpoint::new([2, 2], 0.0, vec![[0; 4]; 4]);
        let recipe = FieldRecipe::new(seed.key(), vec![], seed.digest(), String::new());
        let mut samples = Samples::default();
        let column = [2.0, 2.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        samples.record(old.clone(), id, &recipe, 7, column);
        assert!(samples.can_rebind(&point, id, &recipe));
        assert!(!samples.matches(&point, id, &recipe));
        assert!(
            samples.get(&point, id, &recipe, 8).is_none(),
            "a predicate never authenticates a new pose by itself"
        );
        samples.reuse(&point, id, &recipe, 8).unwrap();
        assert_eq!(samples.get(&point, id, &recipe, 8), Some(column));
        assert!(samples.get(&old, id, &recipe, 8).is_none());
        let mut wrong = point.clone();
        wrong.grid[0] += 1.0;
        assert!(!samples.can_rebind(&wrong, id, &recipe));
        wrong = point.clone();
        wrong.grid[1] += 1.0;
        assert!(!samples.can_rebind(&wrong, id, &recipe));
        let mut wrong = point.clone();
        wrong.actor = id;
        assert!(!samples.can_rebind(&wrong, id, &recipe));
        let mut wrong = point.clone();
        wrong.field.origin[0] += 0.25;
        assert!(!samples.can_rebind(&wrong, id, &recipe));
        wrong = point.clone();
        wrong.field.cell *= 2.0;
        assert!(!samples.can_rebind(&wrong, id, &recipe));
        wrong = point.clone();
        wrong.field.size[0] += 1;
        assert!(!samples.can_rebind(&wrong, id, &recipe));
        assert!(!samples.can_rebind(&point, actor, &recipe));
        let mut changed = recipe.clone();
        changed.digest[0] ^= 1;
        assert!(!samples.can_rebind(&point, id, &changed));
        let other = FieldCheckpoint::new([2, 2], 1.0, vec![[0; 4]; 4]);
        let changed = FieldRecipe::new(other.key(), vec![], recipe.digest, String::new());
        assert!(!samples.can_rebind(&point, id, &changed));
        assert!(samples.reuse(&point, id, &changed, 9).is_err());
        assert_eq!(
            samples.get(&point, id, &recipe, 8),
            Some(column),
            "rejected rebind leaves receipt untouched"
        );
    }

    #[test]
    fn nonfinite_pose_and_grid_fail_before_ownership() {
        let mut region = super::super::dam_scene(1);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut pose = Transform::at([20.0, 2.0, 20.0]);
            pose.position[1] = bad;
            assert_eq!(
                Point::new(EntityId::FIRST, pose, [0.0; 3], &region)
                    .unwrap_err()
                    .code,
                "water.swimming_failed"
            );
            pose = Transform::default();
            pose.rotation[0] = bad;
            assert!(Point::new(EntityId::FIRST, pose, [0.0; 3], &region).is_err());
        }
        region.cell_m = f64::NAN;
        assert!(Point::new(EntityId::FIRST, Transform::default(), [0.0; 3], &region).is_err());
    }
}
