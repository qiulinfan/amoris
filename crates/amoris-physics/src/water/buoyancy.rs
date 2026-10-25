//! Current-pose resident samples for the next force system. This cache is fully derived from
//! the current world and exact field: it is discarded on restore and never read as old tiles.
use std::collections::BTreeMap;

use amoris_assets::water_field::FieldRecipe;
use amoris_sim::{ContentHash, EntityId};
use bevy_ecs::prelude::Resource;

use crate::{body::Transform, geom::V3};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum HullPoint {
    Keel,
    Rudder,
    Center,
}
impl HullPoint {
    pub const ALL: [Self; 3] = [Self::Keel, Self::Rudder, Self::Center];
    pub fn local(self, hull: &crate::boat::Hull) -> V3 {
        match self {
            Self::Keel => hull.keel_at,
            Self::Rudder => hull.rudder_at,
            Self::Center => crate::geom::ZERO,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Role {
    Floater(usize),
    Hull(HullPoint),
}
pub(crate) type Key = (EntityId, Role);
#[derive(Clone, Debug, PartialEq)]
struct Grid {
    origin: [f64; 2],
    cell: f64,
    size: [u32; 2],
}

pub(crate) fn hull_failed(message: impl Into<String>) -> amoris_contract::Problem {
    amoris_contract::Problem::new(
        "water.hull_failed",
        message.into(),
        amoris_contract::detail([]),
    )
}

/// Validate a region once per system, then resolve point ownership without rescanning its tiles.
pub(crate) struct Cover {
    pub id: EntityId,
    pub resident: bool,
    origin: [f64; 2],
    cell: f64,
    size: [u32; 2],
}
impl Cover {
    pub fn new(id: EntityId, origin: V3, region: &super::WaterRegion) -> Option<Self> {
        let resident = region.solver == super::Solver::GpuResident;
        (resident || region.well_formed()).then_some(Self {
            id,
            resident,
            origin: [origin[0], origin[2]],
            cell: region.cell_m,
            size: region.size,
        })
    }
    /// The region's column size, m.
    pub fn cell(&self) -> f64 {
        self.cell
    }
    /// The region's columns along x and z.
    pub fn size(&self) -> [u32; 2] {
        self.size
    }
    pub fn grid_at(&self, at: V3) -> Option<[f64; 2]> {
        let grid = [
            (at[0] - self.origin[0]) / self.cell,
            (at[2] - self.origin[1]) / self.cell,
        ];
        grid.iter()
            .zip(self.size)
            .all(|(&q, n)| q >= 0.0 && q < f64::from(n))
            .then_some(grid)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Point {
    pub key: Key,
    pub at: V3,
    pub grid: [f64; 2],
    pub coupled: bool,
    pose: Option<Transform>,
    field: Option<Grid>,
}
impl Point {
    pub fn floater(actor: EntityId, index: usize, at: V3, grid: [f64; 2], coupled: bool) -> Self {
        Self {
            key: (actor, Role::Floater(index)),
            at,
            grid,
            coupled,
            pose: None,
            field: None,
        }
    }
    pub fn hull(
        actor: EntityId,
        role: HullPoint,
        pose: Transform,
        at: V3,
        grid: [f64; 2],
        coupled: bool,
        cover: &Cover,
    ) -> Result<Self, amoris_contract::Problem> {
        if pose
            .position
            .iter()
            .chain(&pose.rotation)
            .chain(&at)
            .chain(&grid)
            .any(|v| !v.is_finite())
        {
            return Err(hull_failed("Hull sampling pose and point must be finite"));
        }
        Ok(Self {
            key: (actor, Role::Hull(role)),
            at,
            grid,
            coupled,
            pose: Some(pose),
            field: Some(Grid {
                origin: cover.origin,
                cell: cover.cell,
                size: cover.size,
            }),
        })
    }
}

struct Observation {
    region: EntityId,
    at: V3,
    grid: [f64; 2],
    coupled: bool,
    pose: Option<Transform>,
    field: Option<Grid>,
    digest: [u8; 32],
    checkpoint: ContentHash,
    tick: u64,
    column: super::WaterColumn,
}

#[derive(Resource, Default)]
pub(crate) struct Samples {
    rows: BTreeMap<Key, Observation>,
}
impl Samples {
    pub fn retain(&mut self, live: &std::collections::BTreeSet<Key>) {
        self.rows.retain(|key, _| live.contains(key));
    }
    pub fn matches(&self, point: &Point, region: EntityId, recipe: &FieldRecipe) -> bool {
        self.rows.get(&point.key).is_some_and(|old| {
            old.region == region
                && old.at == point.at
                && old.grid == point.grid
                && old.coupled == point.coupled
                && old.pose == point.pose
                && old.field == point.field
                && old.digest == recipe.digest
                && old.checkpoint == recipe.checkpoint()
        })
    }
    pub fn reuse(&mut self, point: &Point, region: EntityId, recipe: &FieldRecipe, tick: u64) {
        debug_assert!(self.matches(point, region, recipe));
        if let Some(old) = self.rows.get_mut(&point.key) {
            old.tick = tick;
        }
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
            point.key,
            Observation {
                region,
                at: point.at,
                grid: point.grid,
                coupled: point.coupled,
                pose: point.pose,
                field: point.field,
                digest: recipe.digest,
                checkpoint: recipe.checkpoint(),
                tick,
                column,
            },
        );
    }
    /// Hull consumers authenticate the exact current pose, region/grid, sampling mode and field.
    pub fn get_hull(
        &self,
        point: &Point,
        region: EntityId,
        recipe: &FieldRecipe,
        tick: u64,
    ) -> Option<super::WaterColumn> {
        self.matches(point, region, recipe)
            .then(|| self.rows.get(&point.key))
            .flatten()
            .filter(|old| old.tick == tick)
            .map(|old| old.column)
    }
    pub fn get(
        &self,
        key: (EntityId, usize),
        at: V3,
        region: EntityId,
        tick: u64,
    ) -> Option<super::WaterColumn> {
        self.rows
            .get(&(key.0, Role::Floater(key.1)))
            .filter(|s| s.at == at && s.region == region && s.tick == tick)
            .map(|s| s.column)
    }
}

/// Bilinear sampling of centre columns and centred face velocities, matching GPU point queries,
/// with the free surface's slope and rate of rise from the same columns.
pub(crate) fn sample(region: &super::WaterRegion, grid: [f64; 2]) -> super::WaterColumn {
    let size = region.columns_along();
    let q = std::array::from_fn::<_, 2, _>(|a| {
        amoris_sim::math::clamp(grid[a] - 0.5, 0.0, f64::from(region.size[a] - 1))
    });
    let lo = q.map(|v| super::whole(v).expect("validated in-region sample"));
    let hi = std::array::from_fn::<_, 2, _>(|a| (lo[a] + 1).min(size[a] - 1));
    let t = std::array::from_fn::<_, 2, _>(|a| {
        q[a] - f64::from(u32::try_from(lo[a]).expect("bounded field"))
    });
    let column = |i: usize, k: usize| {
        let v = region.velocity_at(i, k);
        let (l, r) = (i.saturating_sub(1), (i + 1).min(size[0] - 1));
        let (d, u) = (k.saturating_sub(1), (k + 1).min(size[1] - 1));
        let span = |a: usize, b: usize| f64::from(u32::try_from(b - a).unwrap_or(1).max(1));
        let wet = region.depth_at(i, k) > 0.0;
        let here = region.surface_at(i, k);
        let side = |i: usize, k: usize| {
            if region.depth_at(i, k) > 0.0 {
                region.surface_at(i, k)
            } else {
                here
            }
        };
        let slope = if wet {
            [
                (side(r, k) - side(l, k)) / (span(l, r) * region.cell_m),
                (side(i, u) - side(i, d)) / (span(d, u) * region.cell_m),
            ]
        } else {
            [0.0; 2]
        };
        [
            here,
            region.depth_at(i, k),
            v[0],
            v[1],
            slope[0],
            slope[1],
            region.rise_at(i, k),
        ]
    };
    let a = column(lo[0], lo[1]);
    let b = column(hi[0], lo[1]);
    let c = column(lo[0], hi[1]);
    let d = column(hi[0], hi[1]);
    std::array::from_fn(|n| {
        let x0 = a[n] + (b[n] - a[n]) * t[0];
        let x1 = c[n] + (d[n] - c[n]) * t[0];
        x0 + (x1 - x0) * t[1]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use amoris_assets::water_field::FieldCheckpoint;

    #[test]
    fn hull_roles_are_distinct_and_authenticate_pose_field_grid_mode_and_tick() {
        let actor = EntityId::FIRST;
        let region_id = EntityId::new(2).unwrap();
        let mut region = super::super::dam_scene(1);
        region.cell_m = 1.0;
        let cover = Cover::new(region_id, [0.0; 3], &region).unwrap();
        let pose = Transform::at([4.0, 1.0, 4.0]);
        let at = pose.position;
        let point = |role| Point::hull(actor, role, pose, at, [4.0, 4.0], false, &cover).unwrap();
        let seed = FieldCheckpoint::new([2, 2], 0.0, vec![[0; 4]; 4]);
        let recipe = FieldRecipe::new(seed.key(), vec![], seed.digest(), String::new());
        let mut samples = Samples::default();
        let floater = Point::floater(actor, 0, at, [4.0, 4.0], false);
        samples.record(floater, region_id, &recipe, 7, [1.0; 7]);
        for (i, role) in HullPoint::ALL.into_iter().enumerate() {
            samples.record(
                point(role),
                region_id,
                &recipe,
                7,
                [f64::from(u32::try_from(i + 2).unwrap()); 7],
            );
        }
        assert_eq!(samples.rows.len(), 4);
        assert_eq!(samples.get((actor, 0), at, region_id, 7), Some([1.0; 7]));
        for (i, role) in HullPoint::ALL.into_iter().enumerate() {
            assert_eq!(
                samples.get_hull(&point(role), region_id, &recipe, 7),
                Some([f64::from(u32::try_from(i + 2).unwrap()); 7])
            );
        }
        let p = point(HullPoint::Center);
        assert!(samples.get_hull(&p, region_id, &recipe, 8).is_none());
        let mut changed = p.clone();
        changed.pose = Some(Transform::at_yaw(at, 90.0));
        assert!(
            samples.get_hull(&changed, region_id, &recipe, 7).is_none(),
            "even unchanged centre position requires the current full pose"
        );
        changed = p.clone();
        changed.coupled = true;
        assert!(!samples.matches(&changed, region_id, &recipe));
        changed = p.clone();
        changed.grid[0] += 1.0;
        assert!(!samples.matches(&changed, region_id, &recipe));
        changed = p.clone();
        changed.field.as_mut().unwrap().cell *= 2.0;
        assert!(!samples.matches(&changed, region_id, &recipe));
        assert!(!samples.matches(&p, actor, &recipe));
        let mut other = recipe.clone();
        other.digest[0] ^= 1;
        assert!(!samples.matches(&p, region_id, &other));
        let other_seed = FieldCheckpoint::new([2, 2], 1.0, vec![[0; 4]; 4]);
        let other = FieldRecipe::new(other_seed.key(), vec![], recipe.digest, String::new());
        assert!(!samples.matches(&p, region_id, &other));
        let mut world = bevy_ecs::world::World::new();
        world.insert_resource(samples);
        super::super::compute::invalidate(&mut world);
        assert!(
            world
                .resource::<Samples>()
                .get_hull(&p, region_id, &recipe, 7)
                .is_none()
        );
    }
}
