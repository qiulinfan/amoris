//! `ctx.water` (docs/spec/water.md 8 and 20): direct column access for CPU/full-readback fields
//! and immutable, stamped observations for resident probes. Direct additions/removals are staged
//! until system return; observations read completed field results and never await GPU work.

use amoris_physics::water::{
    Solver,
    resident::{WaterCheckpoints, WaterProbe, WaterResident},
};
use amoris_physics::{Transform, WaterRegion};
use amoris_sim::EntityId;
use amoris_sim::entity;
use amoris_sim::math;
use amoris_sim::{ContentHash, SimClock, Tick};
use bevy_ecs::prelude::World;
use rquickjs::{Array, Ctx, Object, TypedArray, Value};

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
    if r.solver == amoris_physics::water::Solver::GpuResident {
        return Err(ScriptError::new(
            "water.probe_required",
            "Use WaterProbe requests and ctx.water.observe(probe) for resident observations; edit water with WaterSource/WaterDrain.",
            ErrorPhase::Run,
        ));
    }
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

/// Query columns commit before commands. A changed request column already invalidates its
/// observation even though the write-back has not happened yet. Read only this entity's cells,
/// rather than materializing/validating every query on each sparse observation.
fn request_columns_changed<'js>(
    ctx: &Ctx<'js>,
    call: &CallState,
    id: EntityId,
    comp: &crate::call::Comp,
) -> Result<bool, ScriptError> {
    let detached = || {
        ScriptError::new(
            "script.write_type",
            "A WaterProbe request column was detached or replaced.",
            ErrorPhase::Run,
        )
        .component("WaterProbe")
    };
    for query in &call.queries {
        let Ok(row) = query.ids.binary_search(&id) else {
            continue;
        };
        for (_, columns) in query.comps.iter().filter(|(c, _)| c.id == comp.id) {
            for col in columns {
                let object = col.array.clone().restore(ctx).map_err(|_| detached())?;
                let array = TypedArray::<f64>::from_object(object).map_err(|_| detached())?;
                // SAFETY: the host call executes no JavaScript while these bytes are read.
                let bytes = unsafe { array.as_bytes() }.ok_or_else(detached)?;
                if bytes.len() != col.original.len() * 8 {
                    return Err(detached());
                }
                let start = row * 8;
                let value = f64::from_ne_bytes(
                    bytes[start..start + 8]
                        .try_into()
                        .expect("checked column row"),
                );
                if value.to_bits() != col.original[row].to_bits() {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Read the latest completed water-boundary result (at most one tick old). The script schema exposes only request fields;
/// output fields remain engine-owned and cannot be written through a script overlay.
pub fn observe<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let v = arg(ctx, args, 0);
    let observed = with_call(ctx, "water.observe", |call, world| {
        let Some(id) = entity_arg(&v).filter(|&id| call.overlay.alive(world, id)) else {
            return Ok(None);
        };
        let comp = crate::call::resolve_comp(world, "WaterProbe")?;
        if request_columns_changed(ctx, call, id, &comp)? {
            return Ok(None);
        }
        let Some(request) = call.overlay.value(world, id, &comp) else {
            return Ok(None);
        };
        let Some(region_id) = EntityId::from_f64(request.nums[0]).ok() else {
            return Ok(None);
        };
        let at = [request.nums[1], request.nums[2]];
        let Some(e) = entity::entity(world, id) else {
            return Ok(None);
        };
        let Some(probe) = world.get::<WaterProbe>(e) else {
            return Ok(None);
        };
        let tick = world.resource::<SimClock>().tick.0;
        if probe.tick > tick || tick - probe.tick > 1 {
            return Ok(None);
        }
        let Some(re) = entity::entity(world, region_id) else {
            return Ok(None);
        };
        let (Some(region), Some(state)) =
            (world.get::<WaterRegion>(re), world.get::<WaterResident>(re))
        else {
            return Ok(None);
        };
        if region.solver != Solver::GpuResident || state.completed_tick != probe.tick {
            return Ok(None);
        }
        let Some(seed) = world
            .get_resource::<WaterCheckpoints>()
            .and_then(|s| s.fields.get(&state.recipe.checkpoint()))
        else {
            return Ok(None);
        };
        let p = world.get::<Transform>(re).map_or([0.0; 3], |t| t.position);
        if seed.size != region.size || region.column_of([p[0], p[2]], at[0], at[1]).is_none() {
            return Ok(None);
        }
        let grid = [
            (at[0] - p[0]) / region.cell_m,
            (at[1] - p[2]) / region.cell_m,
        ];
        Ok(probe
            .matches(region_id, at, grid, &state.recipe)
            .then(|| (probe.clone(), tick - probe.tick)))
    })?;
    let Some((probe, age)) = observed else {
        return Ok(Value::new_undefined(ctx.clone()));
    };
    let pair = |v: [f64; 2]| -> rquickjs::Result<Value<'js>> {
        let a = Array::new(ctx.clone())?;
        a.set(0, v[0])?;
        a.set(1, v[1])?;
        freeze_new(ctx, a.into_object())
    };
    let o = Object::new(ctx.clone())?;
    o.set(
        "region",
        probe.sampled_region.expect("matched region").to_f64(),
    )?;
    o.set("at", pair(probe.sampled_at)?)?;
    o.set("tick", Tick(probe.tick).to_f64())?;
    o.set("ageTicks", Tick(age).to_f64())?;
    o.set("surface", probe.surface)?;
    o.set("depth", probe.depth)?;
    o.set("velocity", pair(probe.velocity)?)?;
    o.set("stateDigest", ContentHash(probe.state_digest).to_hex())?;
    Ok(freeze_new(ctx, o)?)
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
