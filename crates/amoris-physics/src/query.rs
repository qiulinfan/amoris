//! Spatial queries over the solver and the fields (docs/spec/architecture.md 4.4: the general
//! primitives perception and scripts use, charter 4.2.4): a ray cast, the bodies overlapping a
//! ball, the water and the wind at a point. They read the world at a boundary; the solver's
//! broad phase is as the last step left it, so a body moved since is found where it was.

use amoris_contract::codes::{Range, out_of_range};
use amoris_contract::{Pointer, Problem};
use amoris_sim::{EntityId, EntityIndex, SimClock};
use bevy_ecs::prelude::World;
use rapier3d::prelude::{QueryFilter, Ray, SharedShape};
use serde_json::Value;

use crate::geom::{self, SOLVER_MAX, V3};
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

/// `request.out_of_range {path, got, min, max}` for a query's argument outside `[min, max]`; NaN
/// and the infinities are named as text, since JSON has no number for them.
fn bad_argument(path: &str, got: f64, min: f64, max: f64, hint: Option<&str>) -> Problem {
    let number = |x: f64| serde_json::Number::from_f64(x);
    let got = number(got).map_or_else(
        || {
            Value::from(if got.is_nan() {
                "NaN"
            } else if got > 0.0 {
                "Infinity"
            } else {
                "-Infinity"
            })
        },
        Value::Number,
    );
    let range = Range {
        min: number(min),
        max: number(max),
        ..Range::default()
    };
    out_of_range(&Pointer::parse(path), &got, &range, hint)
}

/// `x` when it lies in `[min, max]` (so the solver's `f32` holds it), else the refusal.
fn within(path: &str, x: f64, min: f64, max: f64) -> Result<f64, Problem> {
    if x >= min && x <= max {
        Ok(x)
    } else {
        Err(bad_argument(path, x, min, max, None))
    }
}

/// The checked arguments of a ray cast: the origin, the direction scaled to unit length (so the
/// distance is in metres whatever length was given) and the reach, each held by the solver's
/// `f32`. Arguments are refused rather than rounded into something else (charter 3.4).
pub fn ray_arguments(origin: V3, dir: V3, max: f64) -> Result<(V3, V3, f64), Problem> {
    for (i, x) in origin.iter().enumerate() {
        within(&format!("/origin/{i}"), *x, -SOLVER_MAX, SOLVER_MAX)?;
    }
    for (i, x) in dir.iter().enumerate() {
        within(&format!("/dir/{i}"), *x, -SOLVER_MAX, SOLVER_MAX)?;
    }
    let len = geom::length(dir);
    if !(len > 0.0 && len.is_finite()) {
        return Err(bad_argument(
            "/dir",
            len,
            f64::MIN_POSITIVE,
            SOLVER_MAX,
            Some("That is the direction's length; a direction needs one above 0."),
        ));
    }
    let max = within("/max", max, 0.0, SOLVER_MAX)?;
    Ok((origin, geom::scale(dir, 1.0 / len), max))
}

/// The first body a ray from `origin` along the unit direction `dir` meets within `max` metres
/// among the colliders whose entity `accept` takes: perception's occlusion test, which looks only
/// at occluders other than the observer and its target (shared/contract/perception.md, step 4).
///
/// Its arguments are checked by [`ray_arguments`] and refused with `request.out_of_range {path}`:
/// an argument the solver's `f32` cannot hold (NaN or past 3.4e38 in magnitude), a zero direction
/// or a negative reach is an error, never a hit Rapier made from a rounded argument nor a clear
/// ray. `dir` is used as given, so a caller's unit direction gives the same bits; the caller
/// decides what a ray it could not cast means.
pub fn cast_ray_among(
    world: &World,
    origin: V3,
    dir: V3,
    max: f64,
    accept: &dyn Fn(EntityId) -> bool,
) -> Result<Option<RayHit>, Problem> {
    ray_arguments(origin, dir, max)?;
    let Some(physics) = world.get_resource::<Physics>() else {
        return Ok(None);
    };
    let keep = |_: rapier3d::prelude::ColliderHandle, c: &rapier3d::prelude::Collider| {
        entity_of(c.user_data).is_some_and(accept)
    };
    let filter = QueryFilter::default().predicate(&keep);
    let ray = Ray::new(geom::vec_in(origin), geom::vec_in(dir));
    let Some((handle, hit)) =
        physics
            .world
            .cast_ray_and_get_normal(&ray, geom::f32_of(max), true, filter)
    else {
        return Ok(None);
    };
    let Some(entity) = physics
        .world
        .colliders
        .get(handle)
        .and_then(|c| entity_of(c.user_data))
    else {
        return Ok(None);
    };
    let distance = f64::from(hit.time_of_impact);
    Ok(Some(RayHit {
        entity,
        distance,
        point: geom::add(origin, geom::scale(dir, distance)),
        normal: geom::vec_out(hit.normal),
    }))
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

/// What a script's query looks at (animation.md 7): every collider but sensors and the listed
/// entities' bodies, and, with `walkers`, the walkers too, each as an upright capsule in its box.
#[derive(Clone, Copy, Debug, Default)]
pub struct Among<'a> {
    pub exclude: &'a [EntityId],
    pub walkers: bool,
}

/// The walkers' capsules: entity, the capsule's lower and upper centres, its radius.
fn walker_capsules(world: &World, among: &Among) -> Vec<(EntityId, V3, V3, f64)> {
    let Some(index) = world.get_resource::<EntityIndex>() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if !among.walkers {
        return out;
    }
    for (id, e) in index.iter() {
        if among.exclude.contains(&id) {
            continue;
        }
        let (Some(w), Some(t)) = (
            world.get::<crate::walker::Walker>(e),
            world.get::<crate::body::Transform>(e),
        ) else {
            continue;
        };
        let r = w.half_width_m;
        let feet = t.position[1] - w.eye_m;
        let lo = [t.position[0], feet + r, t.position[2]];
        let hi = [
            t.position[0],
            feet + amoris_sim::math::max(w.height_m - r, r),
            t.position[2],
        ];
        out.push((id, lo, hi, r));
    }
    out
}

fn capsule(lo: V3, hi: V3, r: f64) -> (rapier3d::math::Pose, rapier3d::parry::shape::Capsule) {
    let mid = [
        0.5 * (lo[0] + hi[0]),
        0.5 * (lo[1] + hi[1]),
        0.5 * (lo[2] + hi[2]),
    ];
    let half = 0.5 * (hi[1] - lo[1]);
    (
        geom::pose_in(mid, geom::IDENTITY),
        rapier3d::parry::shape::Capsule::new_y(geom::f32_of(half), geom::f32_of(r)),
    )
}

/// Keeps the nearer of two hits; at the same distance the lower id.
fn nearer(a: Option<RayHit>, b: Option<RayHit>) -> Option<RayHit> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if (b.distance, b.entity) < (a.distance, a.entity) {
            b
        } else {
            a
        }),
        (a, b) => a.or(b),
    }
}

/// The first collider or walker a ray from `origin` along the unit `dir` meets within `max`.
pub fn ray(
    world: &World,
    origin: V3,
    dir: V3,
    max: f64,
    among: &Among,
) -> Result<Option<RayHit>, Problem> {
    let mut best = cast_ray_among(world, origin, dir, max, &|e| !among.exclude.contains(&e))?;
    use rapier3d::parry::query::RayCast;
    let r = Ray::new(geom::vec_in(origin), geom::vec_in(dir));
    for (id, lo, hi, rad) in walker_capsules(world, among) {
        let (pose, shape) = capsule(lo, hi, rad);
        if let Some(hit) = shape.cast_ray_and_get_normal(&pose, &r, geom::f32_of(max), true) {
            let d = f64::from(hit.time_of_impact);
            best = nearer(
                best,
                Some(RayHit {
                    entity: id,
                    distance: d,
                    point: geom::add(origin, geom::scale(dir, d)),
                    normal: geom::vec_out(hit.normal),
                }),
            );
        }
    }
    Ok(best)
}

/// The first collider or walker a ball of `radius` meets moving from `from` along the unit `dir`
/// for up to `max` metres (a swept sword, a thrown spear).
pub fn sweep(
    world: &World,
    from: V3,
    dir: V3,
    max: f64,
    radius: f64,
    among: &Among,
) -> Result<Option<RayHit>, Problem> {
    ray_arguments(from, dir, max)?;
    let ball = rapier3d::parry::shape::Ball::new(geom::f32_of(radius));
    let start = geom::pose_in(from, geom::IDENTITY);
    let options = rapier3d::parry::query::ShapeCastOptions {
        max_time_of_impact: geom::f32_of(max),
        target_distance: 0.0,
        stop_at_penetration: true,
        compute_impact_geometry_on_penetration: true,
    };
    let mut best = None;
    if let Some(physics) = world.get_resource::<Physics>() {
        let keep = |_: rapier3d::prelude::ColliderHandle, c: &rapier3d::prelude::Collider| {
            entity_of(c.user_data).is_some_and(|e| !among.exclude.contains(&e))
        };
        let filter = QueryFilter::default().exclude_sensors().predicate(&keep);
        if let Some((handle, hit)) =
            physics
                .world
                .cast_shape(&start, geom::vec_in(dir), &ball, options, filter)
            && let Some(entity) = physics
                .world
                .colliders
                .get(handle)
                .and_then(|c| entity_of(c.user_data))
        {
            let d = f64::from(hit.time_of_impact);
            best = Some(RayHit {
                entity,
                distance: d,
                point: geom::vec_out(start * hit.witness1),
                normal: geom::vec_out(hit.normal2),
            });
        }
    }
    for (id, lo, hi, rad) in walker_capsules(world, among) {
        let (pose, shape) = capsule(lo, hi, rad);
        let zero = geom::vec_in([0.0; 3]);
        if let Ok(Some(hit)) = rapier3d::parry::query::cast_shapes(
            &start,
            geom::vec_in(dir),
            &ball,
            &pose,
            zero,
            &shape,
            options,
        ) {
            let d = f64::from(hit.time_of_impact);
            best = nearer(
                best,
                Some(RayHit {
                    entity: id,
                    distance: d,
                    point: geom::add(from, geom::scale(dir, d)),
                    normal: geom::vec_out(hit.normal2),
                }),
            );
        }
    }
    Ok(best)
}

/// The colliders' entities and walkers a ball of `radius` at `center` overlaps, ascending by id.
pub fn overlap(world: &World, center: V3, radius: f64, among: &Among) -> Vec<EntityId> {
    let mut out: Vec<EntityId> = overlap_ball(world, center, radius)
        .into_iter()
        .filter(|e| !among.exclude.contains(e))
        .collect();
    let ball = rapier3d::parry::shape::Ball::new(geom::f32_of(radius));
    let at = geom::pose_in(center, geom::IDENTITY);
    for (id, lo, hi, rad) in walker_capsules(world, among) {
        let (pose, shape) = capsule(lo, hi, rad);
        if rapier3d::parry::query::intersection_test(&at, &ball, &pose, &shape)
            .is_ok_and(|i| i.intersecting)
        {
            out.push(id);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}
