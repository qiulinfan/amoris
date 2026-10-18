//! `ctx.water` (docs/spec/water.md 8): the water of a region at a world point. Reads see the world
//! as the system started; `add` and `remove` are staged and apply when the system returns, as
//! `world.set` does, and wake the column's tile. A point outside the region answers 0 and writes
//! nothing.

use amoris_physics::{Transform, WaterRegion};
use amoris_sim::EntityId;
use amoris_sim::entity;
use amoris_sim::math;
use bevy_ecs::prelude::World;
use rquickjs::{Array, Ctx, Value};

use super::world::entity_arg;
use super::{NResult, arg, with_call};
use crate::call::{CallState, Command};
use crate::convert::freeze_new;
use crate::error::{ErrorPhase, ScriptError, num};

fn range(message: String) -> ScriptError {
    ScriptError::new("water.range", message, ErrorPhase::Run)
}

fn missing(v: &Value<'_>) -> ScriptError {
    let shown = v.as_number().map_or(serde_json::json!(null), num);
    ScriptError::new(
        "water.missing",
        format!("Entity {shown} is not a water region (it has no WaterRegion)."),
        ErrorPhase::Run,
    )
    .value(shown)
}

/// The region an argument names, and its corner (world x, z).
fn region<'w>(
    call: &CallState,
    world: &'w World,
    v: &Value<'_>,
) -> Result<(EntityId, &'w WaterRegion, [f64; 2]), ScriptError> {
    let id = entity_arg(v)
        .filter(|&id| call.overlay.alive(world, id))
        .ok_or_else(|| missing(v))?;
    let e = entity::entity(world, id).ok_or_else(|| missing(v))?;
    let r = world.get::<WaterRegion>(e).ok_or_else(|| missing(v))?;
    let p = world.get::<Transform>(e).map_or([0.0; 3], |t| t.position);
    Ok((id, r, [p[0], p[2]]))
}

fn point<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> Result<(f64, f64), ScriptError> {
    let x = arg(ctx, args, 1).as_number().filter(|x| x.is_finite());
    let z = arg(ctx, args, 2).as_number().filter(|x| x.is_finite());
    match (x, z) {
        (Some(x), Some(z)) => Ok((x, z)),
        _ => Err(range("x and z must be finite numbers, metres".into())),
    }
}

fn amount<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> Result<f64, ScriptError> {
    arg(ctx, args, 3)
        .as_number()
        .filter(|m| m.is_finite() && *m >= 0.0)
        .ok_or_else(|| range("an amount of water is finite cubic metres, 0 or more".into()))
}

/// A read at the column under `(x, z)`: `f` of the region and the column, or 0 outside.
fn read<'js>(
    ctx: &Ctx<'js>,
    args: &[Value<'js>],
    what: &str,
    f: fn(&WaterRegion, usize, usize) -> f64,
) -> Result<f64, ScriptError> {
    let (x, z) = point(ctx, args)?;
    let v = arg(ctx, args, 0);
    with_call(ctx, what, |call, world| {
        let (_, r, corner) = region(call, world, &v)?;
        Ok(r.column_of(corner, x, z).map_or(0.0, |(i, k)| f(r, i, k)))
    })
}

/// `waterDepth(region, x, z)`: metres.
pub fn depth<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let d = read(ctx, args, "water.depth", WaterRegion::depth_at)?;
    Ok(Value::new_float(ctx.clone(), d))
}

/// `waterSurface(region, x, z)`: world y.
pub fn surface<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let s = read(ctx, args, "water.surface", WaterRegion::surface_at)?;
    Ok(Value::new_float(ctx.clone(), s))
}

/// `waterVelocity(region, x, z)`: `[vx, vz]`, m/s.
pub fn velocity<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let (x, z) = point(ctx, args)?;
    let v = arg(ctx, args, 0);
    let vel = with_call(ctx, "water.velocity", |call, world| {
        let (_, r, corner) = region(call, world, &v)?;
        Ok(r.column_of(corner, x, z)
            .map_or([0.0; 2], |(i, k)| r.velocity_at(i, k)))
    })?;
    let a = Array::new(ctx.clone())?;
    a.set(0, vel[0])?;
    a.set(1, vel[1])?;
    Ok(freeze_new(ctx, a.into_object())?)
}

/// The column's water as this call sees it: the world's and the call's staged changes, m^3.
fn held(call: &CallState, r: &WaterRegion, id: EntityId, column: (usize, usize)) -> f64 {
    let mut m3 = r.depth_at(column.0, column.1) * r.cell_m * r.cell_m;
    for c in &call.commands {
        if let Command::Water {
            id: cid,
            column: cc,
            m3: d,
        } = c
            && *cid == id
            && *cc == column
        {
            m3 += d;
        }
    }
    m3
}

/// `waterAdd(region, x, z, m3)`: staged.
pub fn add<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let (x, z) = point(ctx, args)?;
    let m3 = amount(ctx, args)?;
    let v = arg(ctx, args, 0);
    with_call(ctx, "water.add", |call, world| {
        let (id, r, corner) = region(call, world, &v)?;
        if let Some(column) = r.column_of(corner, x, z)
            && m3 > 0.0
        {
            call.commands.push(Command::Water { id, column, m3 });
        }
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

/// `waterRemove(region, x, z, m3)`: staged; takes at most what is there and returns what it took.
pub fn remove<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let (x, z) = point(ctx, args)?;
    let m3 = amount(ctx, args)?;
    let v = arg(ctx, args, 0);
    let taken = with_call(ctx, "water.remove", |call, world| {
        let (id, r, corner) = region(call, world, &v)?;
        let Some(column) = r.column_of(corner, x, z) else {
            return Ok(0.0);
        };
        let take = math::min(m3, math::max(held(call, r, id, column), 0.0));
        if take > 0.0 {
            call.commands.push(Command::Water {
                id,
                column,
                m3: -take,
            });
        }
        Ok(take)
    })?;
    Ok(Value::new_float(ctx.clone(), taken))
}
