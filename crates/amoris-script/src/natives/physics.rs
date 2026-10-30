//! `ctx.physics` (docs/spec/animation.md 7): rays, swept balls and overlapping balls over the
//! solver's colliders and, unless left out, the walkers as capsules. They read the world as the
//! last step left it, never the system's own staged writes.

use amoris_physics::query::{self, Among, RayHit};
use amoris_sim::EntityId;
use rquickjs::{Array, Ctx, Object, Value};

use super::world::entity_arg;
use super::{NResult, arg, with_call};
use crate::convert::freeze_new;
use crate::error::{ErrorPhase, ScriptError};

fn range(message: String) -> ScriptError {
    ScriptError::new("physics.range", message, ErrorPhase::Run)
}

fn triple(v: &Value<'_>, what: &str) -> Result<[f64; 3], ScriptError> {
    let bad = || range(format!("{what} must be an array of three finite numbers"));
    let a = v.as_array().ok_or_else(bad)?;
    if a.len() != 3 {
        return Err(bad());
    }
    let mut out = [0.0; 3];
    for (i, o) in out.iter_mut().enumerate() {
        *o = a
            .get::<Value>(i)
            .ok()
            .and_then(|x| x.as_number())
            .filter(|x| x.is_finite())
            .ok_or_else(bad)?;
    }
    Ok(out)
}

fn positive(v: &Value<'_>, what: &str) -> Result<f64, ScriptError> {
    v.as_number()
        .filter(|x| x.is_finite() && *x >= 0.0)
        .ok_or_else(|| range(format!("{what} must be a finite number, 0 or more")))
}

/// The options object: `exclude` (entities) and `walkers` (default true).
fn options(v: &Value<'_>) -> Result<(Vec<EntityId>, bool), ScriptError> {
    let Some(o) = v.as_object() else {
        return Ok((Vec::new(), true));
    };
    let mut exclude = Vec::new();
    if let Ok(list) = o.get::<_, Value>("exclude")
        && !list.is_undefined()
        && !list.is_null()
    {
        let a = list
            .as_array()
            .ok_or_else(|| range("options.exclude must be a list of entities".into()))?;
        for i in 0..a.len() {
            let e = a.get::<Value>(i).ok().and_then(|v| entity_arg(&v));
            exclude
                .push(e.ok_or_else(|| range("options.exclude must be a list of entities".into()))?);
        }
    }
    let walkers = o
        .get::<_, Value>("walkers")
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    Ok((exclude, walkers))
}

fn unit(d: [f64; 3], what: &str) -> Result<([f64; 3], f64), ScriptError> {
    let len = amoris_sim::math::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
    if len <= 0.0 {
        return Err(range(format!("{what} must not be zero")));
    }
    Ok(([d[0] / len, d[1] / len, d[2] / len], len))
}

fn hit<'js>(ctx: &Ctx<'js>, h: Option<RayHit>) -> NResult<'js> {
    let Some(h) = h else {
        return Ok(Value::new_null(ctx.clone()));
    };
    let o = Object::new(ctx.clone())?;
    let v3 = |v: [f64; 3]| -> rquickjs::Result<Value<'js>> {
        let a = Array::new(ctx.clone())?;
        for (i, x) in v.into_iter().enumerate() {
            a.set(i, x)?;
        }
        Ok(a.into_value())
    };
    o.set("entity", h.entity.to_f64())?;
    o.set("distance", h.distance)?;
    o.set("point", v3(h.point)?)?;
    o.set("normal", v3(h.normal)?)?;
    Ok(freeze_new(ctx, o)?)
}

fn problem(p: amoris_contract::Problem) -> ScriptError {
    range(p.message)
}

/// `physicsRay(from, dir, maxM, options)`: the first thing a ray meets, or null.
pub fn ray<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let from = triple(&arg(ctx, args, 0), "from")?;
    let (dir, _) = unit(triple(&arg(ctx, args, 1), "dir")?, "dir")?;
    let max = positive(&arg(ctx, args, 2), "maxM")?;
    let (exclude, walkers) = options(&arg(ctx, args, 3))?;
    let found = with_call(ctx, "physics.ray", |_, world| {
        query::ray(
            world,
            from,
            dir,
            max,
            &Among {
                exclude: &exclude,
                walkers,
            },
        )
        .map_err(problem)
    })?;
    hit(ctx, found)
}

/// `physicsSweep(from, to, radius, options)`: the first thing a ball moving from `from` to `to`
/// meets, or null.
pub fn sweep<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let from = triple(&arg(ctx, args, 0), "from")?;
    let to = triple(&arg(ctx, args, 1), "to")?;
    let radius = positive(&arg(ctx, args, 2), "radius")?;
    let (exclude, walkers) = options(&arg(ctx, args, 3))?;
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    // A ball that does not move: an overlap's first entity at distance 0.
    let (dir, len) = unit(d, "to - from").unwrap_or(([0.0, -1.0, 0.0], 0.0));
    let found = with_call(ctx, "physics.sweep", |_, world| {
        query::sweep(
            world,
            from,
            dir,
            len,
            radius,
            &Among {
                exclude: &exclude,
                walkers,
            },
        )
        .map_err(problem)
    })?;
    hit(ctx, found)
}

/// `physicsSphere(center, radius, options)`: the entities a ball overlaps, ascending by id.
pub fn sphere<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let center = triple(&arg(ctx, args, 0), "center")?;
    let radius = positive(&arg(ctx, args, 1), "radius")?;
    let (exclude, walkers) = options(&arg(ctx, args, 2))?;
    let found = with_call(ctx, "physics.sphere", |_, world| {
        Ok(query::overlap(
            world,
            center,
            radius,
            &Among {
                exclude: &exclude,
                walkers,
            },
        ))
    })?;
    let a = Array::new(ctx.clone())?;
    for (i, e) in found.into_iter().enumerate() {
        a.set(i, e.to_f64())?;
    }
    Ok(freeze_new(ctx, a.into_object())?)
}

fn grid_arg(
    call: &crate::call::CallState,
    world: &bevy_ecs::prelude::World,
    v: &Value<'_>,
) -> Result<EntityId, ScriptError> {
    entity_arg(v)
        .filter(|&id| call.overlay.alive(world, id))
        .ok_or_else(|| range("the first argument must be an entity with a NavGrid".into()))
}

fn points<'js>(ctx: &Ctx<'js>, ps: &[[f64; 3]]) -> NResult<'js> {
    let a = Array::new(ctx.clone())?;
    for (i, p) in ps.iter().enumerate() {
        let t = Array::new(ctx.clone())?;
        for (j, x) in p.iter().enumerate() {
            t.set(j, *x)?;
        }
        a.set(i, freeze_new(ctx, t.into_object())?)?;
    }
    Ok(freeze_new(ctx, a.into_object())?)
}

/// `navPath(grid, from, to)`: waypoints from `from` to `to` over the grid's walkable floors, or
/// null (animation.md 8). The first query bakes the grid.
pub fn nav_path<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let g = arg(ctx, args, 0);
    let from = triple(&arg(ctx, args, 1), "from")?;
    let to = triple(&arg(ctx, args, 2), "to")?;
    let path = with_call(ctx, "nav.path", |call, world| {
        let id = grid_arg(call, world, &g)?;
        let baked = amoris_physics::nav::baked(world, id)
            .ok_or_else(|| range("the first argument must be an entity with a NavGrid".into()))?;
        Ok(baked.path(from, to))
    })?;
    match path {
        Some(p) => points(ctx, &p),
        None => Ok(Value::new_null(ctx.clone())),
    }
}

/// `navNearest(grid, at, radius)`: the walkable point nearest `at` within `radius`, or null.
pub fn nav_nearest<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let g = arg(ctx, args, 0);
    let at = triple(&arg(ctx, args, 1), "at")?;
    let radius = positive(&arg(ctx, args, 2), "radius")?;
    let p = with_call(ctx, "nav.nearest", |call, world| {
        let id = grid_arg(call, world, &g)?;
        let baked = amoris_physics::nav::baked(world, id)
            .ok_or_else(|| range("the first argument must be an entity with a NavGrid".into()))?;
        Ok(baked.nearest(at, radius))
    })?;
    match p {
        Some(p) => {
            let t = Array::new(ctx.clone())?;
            for (j, x) in p.iter().enumerate() {
                t.set(j, *x)?;
            }
            Ok(freeze_new(ctx, t.into_object())?)
        }
        None => Ok(Value::new_null(ctx.clone())),
    }
}
