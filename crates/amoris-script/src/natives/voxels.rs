//! `ctx.voxels` (docs/spec/voxels.md 4): reads, edits and ray casts of block volumes. Edits are
//! staged on a copy of the volume that the call's later reads see; the copy replaces the volume
//! when the call commits, and a throw discards it.

use amoris_sim::EntityId;
use amoris_sim::entity;
use amoris_sim::num::to_i32;
use amoris_sim::voxels::{Cell, Voxels};
use bevy_ecs::prelude::World;
use rquickjs::{Array, Ctx, Object, Value};
use serde_json::json;

use super::world::entity_arg;
use super::{NResult, arg, with_call};
use crate::call::{CallState, Command, resolve_comp};
use crate::convert::freeze_new;
use crate::error::{ErrorPhase, ScriptError, num};

fn range(message: String) -> ScriptError {
    ScriptError::new("voxels.range", message, ErrorPhase::Run)
}

fn missing(v: &Value<'_>) -> ScriptError {
    let shown = v.as_number().map_or(json!(null), num);
    ScriptError::new(
        "voxels.missing",
        format!("Entity {shown} is not a block volume (it has no Voxels)."),
        ErrorPhase::Run,
    )
    .value(shown)
}

/// The volume an argument names as the call sees it: its staged copy, else the world's.
fn volume<'w>(
    call: &'w CallState,
    world: &'w World,
    v: &Value<'_>,
) -> Result<(EntityId, &'w Voxels), ScriptError> {
    let id = entity_arg(v)
        .filter(|&id| call.overlay.alive(world, id))
        .ok_or_else(|| missing(v))?;
    if let Some(&i) = call.voxels.get(&id)
        && let Some(Command::Voxels { volume, .. }) = call.commands.get(i)
    {
        return Ok((id, volume));
    }
    entity::entity(world, id)
        .and_then(|e| world.get::<Voxels>(e))
        .map(|vox| (id, vox))
        .ok_or_else(|| missing(v))
}

/// The call's staged copy of the volume, made at its first edit.
fn staged<'c>(
    call: &'c mut CallState,
    world: &World,
    v: &Value<'_>,
) -> Result<&'c mut Voxels, ScriptError> {
    let (id, current) = volume(call, world, v)?;
    if !call.voxels.contains_key(&id) {
        let copy = Box::new(current.clone());
        call.voxels.insert(id, call.commands.len());
        call.commands.push(Command::Voxels { id, volume: copy });
    }
    match call.voxels.get(&id).and_then(|&i| call.commands.get_mut(i)) {
        Some(Command::Voxels { volume, .. }) => Ok(volume),
        _ => Err(ScriptError::new(
            "sim.internal",
            "a staged volume lost its command",
            ErrorPhase::Run,
        )),
    }
}

fn int(v: &Value<'_>, what: &str) -> Result<i32, ScriptError> {
    v.as_number()
        .and_then(|x| to_i32(x).ok())
        .ok_or_else(|| range(format!("{what} must be a whole number in the i32 range")))
}

fn cell<'js>(ctx: &Ctx<'js>, args: &[Value<'js>], at: usize) -> Result<Cell, ScriptError> {
    Ok([
        int(&arg(ctx, args, at), "x")?,
        int(&arg(ctx, args, at + 1), "y")?,
        int(&arg(ctx, args, at + 2), "z")?,
    ])
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

fn cell_of(v: &Value<'_>, what: &str) -> Result<Cell, ScriptError> {
    let t = triple(v, what)?;
    let mut out = [0; 3];
    for a in 0..3 {
        out[a] = to_i32(t[a]).map_err(|_| range(format!("{what} must hold whole numbers")))?;
    }
    Ok(out)
}

fn block(v: &Value<'_>) -> Result<u8, ScriptError> {
    v.as_number()
        .and_then(|x| to_i32(x).ok())
        .and_then(|x| u8::try_from(x).ok())
        .ok_or_else(|| range("a block is a palette index from 0 to 255".into()))
}

fn edited(e: amoris_contract::Problem) -> ScriptError {
    ScriptError::new(&e.code, e.message, ErrorPhase::Run)
}

/// `voxelsGet(volume, x, y, z)`: the block at a cell, 0 for air.
pub fn get<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let at = cell(ctx, args, 1)?;
    let v = arg(ctx, args, 0);
    let b = with_call(ctx, "voxels.get", |call, world| {
        Ok(volume(call, world, &v)?.1.get(at))
    })?;
    Ok(Value::new_int(ctx.clone(), i32::from(b)))
}

/// `voxelsBlock(volume, name)`: a block's palette index by name.
pub fn block_named<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let v = arg(ctx, args, 0);
    let name = arg(ctx, args, 1)
        .as_string()
        .and_then(|s| s.to_string().ok())
        .ok_or_else(|| range("a block is named by a string".into()))?;
    let b = with_call(ctx, "voxels.block", |call, world| {
        let (_, vox) = volume(call, world, &v)?;
        vox.block(&name).ok_or_else(|| {
            let names: Vec<&str> = vox.palette.iter().map(|b| b.name.as_str()).collect();
            range(format!(
                "the palette has no block {name:?}; it has {names:?}"
            ))
        })
    })?;
    Ok(Value::new_int(ctx.clone(), i32::from(b)))
}

/// `voxelsSet(volume, x, y, z, block)`: staged.
pub fn set<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let at = cell(ctx, args, 1)?;
    let b = block(&arg(ctx, args, 4))?;
    let v = arg(ctx, args, 0);
    with_call(ctx, "voxels.set", |call, world| {
        let tick = call.tick.0;
        staged(call, world, &v)?.set(at, b, tick).map_err(edited)?;
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

/// `voxelsFill(volume, lo, hi, block)`: the inclusive box, staged.
pub fn fill<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let lo = cell_of(&arg(ctx, args, 1), "lo")?;
    let hi = cell_of(&arg(ctx, args, 2), "hi")?;
    let b = block(&arg(ctx, args, 3))?;
    let v = arg(ctx, args, 0);
    let changed = with_call(ctx, "voxels.fill", |call, world| {
        let tick = call.tick.0;
        staged(call, world, &v)?
            .fill(lo, hi, b, tick)
            .map_err(edited)
    })?;
    Ok(Value::new_float(ctx.clone(), changed as f64))
}

/// `voxelsRaycast(volume, from, dir, maxM, solidOnly)`: the first cell hit, or `null`.
pub fn raycast<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let from = triple(&arg(ctx, args, 1), "from")?;
    let dir = triple(&arg(ctx, args, 2), "dir")?;
    let max_m = arg(ctx, args, 3)
        .as_number()
        .filter(|x| x.is_finite() && *x >= 0.0)
        .ok_or_else(|| range("maxM must be a finite distance, 0 or more".into()))?;
    let solid_only = arg(ctx, args, 4).as_bool().unwrap_or(false);
    let v = arg(ctx, args, 0);
    let hit = with_call(ctx, "voxels.raycast", |call, world| {
        let (id, vox) = volume(call, world, &v)?;
        // The volume's origin is its Transform's position, as the call sees it.
        let origin = resolve_comp(world, "Transform")
            .ok()
            .and_then(|t| call.overlay.value(world, id, &t))
            .map_or([0.0; 3], |t| [t.nums[0], t.nums[1], t.nums[2]]);
        Ok(vox.raycast(origin, from, dir, max_m, solid_only))
    })?;
    let Some(hit) = hit else {
        return Ok(Value::new_null(ctx.clone()));
    };
    let ints = |c: Cell| -> rquickjs::Result<Value<'js>> {
        let a = Array::new(ctx.clone())?;
        for (i, x) in c.into_iter().enumerate() {
            a.set(i, x)?;
        }
        Ok(a.into_value())
    };
    let o = Object::new(ctx.clone())?;
    o.set("cell", ints(hit.cell)?)?;
    o.set("normal", ints(hit.normal)?)?;
    o.set("block", i32::from(hit.block))?;
    o.set("distanceM", hit.distance_m)?;
    Ok(freeze_new(ctx, o)?)
}
