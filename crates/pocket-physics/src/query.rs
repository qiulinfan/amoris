//! Spatial queries over the solver and the fields (docs/spec/architecture.md 4.4: the general
//! primitives perception and scripts use, charter 4.2.4): a ray cast, the bodies overlapping a
//! ball, the water and the wind at a point. They read the world at a boundary; the solver's
//! broad phase is as the last step left it, so a body moved since is found where it was.

use bevy_ecs::prelude::World;
use pocket_sim::{EntityId, EntityIndex, SimClock};
use rapier3d::prelude::{QueryFilter, Ray, SharedShape};

use crate::geom::{self, V3};
use crate::sea::{Sea, Water, Wind};
use crate::solver::{BodyIndex, Physics, entity_of};

/// What a ray hit: the entity, how far along the ray, the point and the surface's normal there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RayHit {
    pub entity: EntityId,
    pub distance: f64,
    pub point: V3,
    pub normal: V3,
}

/// The first body a ray from `origin` along the unit direction `dir` meets within `max` metres,
/// leaving out `exclude`'s body.
pub fn cast_ray(
    world: &World,
    origin: V3,
    dir: V3,
    max: f64,
    exclude: Option<EntityId>,
) -> Option<RayHit> {
    let physics = world.get_resource::<Physics>()?;
    let mut filter = QueryFilter::default();
    if let Some(h) = exclude.and_then(|id| world.get_resource::<BodyIndex>()?.get(id)) {
        filter = filter.exclude_rigid_body(h);
    }
    let ray = Ray::new(geom::vec_in(origin), geom::vec_in(dir));
    let (handle, hit) =
        physics
            .world
            .cast_ray_and_get_normal(&ray, geom::f32_of(max), true, filter)?;
    let entity = entity_of(physics.world.colliders.get(handle)?.user_data)?;
    let distance = f64::from(hit.time_of_impact);
    Some(RayHit {
        entity,
        distance,
        point: geom::add(origin, geom::scale(dir, distance)),
        normal: geom::vec_out(hit.normal),
    })
}

/// The entities whose colliders overlap a ball of `radius` at `center`, ascending by id.
pub fn overlap_ball(world: &World, center: V3, radius: f64) -> Vec<EntityId> {
    let Some(physics) = world.get_resource::<Physics>() else {
        return Vec::new();
    };
    let ball = SharedShape::ball(geom::f32_of(radius));
    let pose = geom::pose_in(center, geom::IDENTITY);
    let mut out: Vec<EntityId> = physics
        .world
        .intersect_shape(pose, &*ball, QueryFilter::default())
        .filter_map(|(_, c)| entity_of(c.user_data))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The component of the lowest id that has `C`, walking the entity index in id order.
fn first<C: bevy_ecs::component::Component>(world: &World) -> Option<&C> {
    let index = world.get_resource::<EntityIndex>()?;
    index.iter().find_map(|(_, e)| world.get::<C>(e))
}

/// The sea's surface and the water's velocity over (x, z) now (the boundary's time), or `None`
/// without a sea.
pub fn water_at(world: &World, x: f64, z: f64) -> Option<Water> {
    let t = world.get_resource::<SimClock>()?.time();
    Some(first::<Sea>(world)?.field().at(x, z, t))
}

/// The true wind at `p` now (calm without a `Wind`).
pub fn wind_at(world: &World, p: V3) -> V3 {
    let t = world.get_resource::<SimClock>().map_or(0.0, SimClock::time);
    first::<Wind>(world).map_or(geom::ZERO, |w| w.at(p, t))
}
