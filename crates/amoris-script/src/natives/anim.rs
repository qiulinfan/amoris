//! `ctx.anim` (docs/spec/animation.md 4): an entity's graph's parameters, montage requests, state
//! and sockets. Parameters and requests are staged until the system returns, as every write;
//! reads see the world as the system started, with this call's own parameters over it.

use amoris_assets::anim::{AnimCommand, AnimState};
use amoris_sim::{EntityId, entity};
use bevy_ecs::prelude::World;
use rquickjs::{Array, Ctx, Object, Value};

use super::world::entity_arg;
use super::{NResult, arg, with_call};
use crate::call::{CallState, Command};
use crate::convert::freeze_new;
use crate::error::{ErrorPhase, ScriptError};

fn range(message: String) -> ScriptError {
    ScriptError::new("anim.range", message, ErrorPhase::Run)
}

fn target(call: &CallState, world: &World, v: &Value<'_>) -> Result<EntityId, ScriptError> {
    entity_arg(v)
        .filter(|&id| call.overlay.alive(world, id))
        .ok_or_else(|| range("the first argument must be a live entity".into()))
}

fn text(v: &Value<'_>, what: &str) -> Result<String, ScriptError> {
    v.as_string()
        .and_then(|s| s.to_string().ok())
        .ok_or_else(|| range(format!("{what} must be a string")))
}

fn state(world: &World, id: EntityId) -> Option<&AnimState> {
    entity::entity(world, id).and_then(|e| world.get::<AnimState>(e))
}

/// `animSet(e, name, value)`: a parameter (a boolean as 1 or 0).
pub fn set<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let e = arg(ctx, args, 0);
    let name = text(&arg(ctx, args, 1), "a parameter's name")?;
    let v = arg(ctx, args, 2);
    let value = match v.as_bool() {
        Some(b) => {
            if b {
                1.0
            } else {
                0.0
            }
        }
        None => v.as_number().filter(|x| x.is_finite()).ok_or_else(|| {
            range(format!(
                "parameter {name} takes a finite number or a boolean"
            ))
        })?,
    };
    with_call(ctx, "anim.set", |call, world| {
        let id = target(call, world, &e)?;
        call.commands.push(Command::Anim {
            id,
            param: Some((name, value)),
            command: None,
        });
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

/// `animParam(e, name)`: a parameter's value, this call's own setting over the world's.
pub fn param<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let e = arg(ctx, args, 0);
    let name = text(&arg(ctx, args, 1), "a parameter's name")?;
    let v = with_call(ctx, "anim.param", |call, world| {
        let id = target(call, world, &e)?;
        let staged = call.commands.iter().rev().find_map(|c| match c {
            Command::Anim {
                id: at,
                param: Some((n, v)),
                ..
            } if *at == id && *n == name => Some(*v),
            _ => None,
        });
        Ok(staged.or_else(|| {
            state(world, id).and_then(|s| s.params.iter().find(|p| p.name == name).map(|p| p.value))
        }))
    })?;
    Ok(Value::new_number(ctx.clone(), v.unwrap_or(0.0)))
}

/// `animCommand(e, op, montage, section, rate, fade)`: a montage request.
pub fn command<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let e = arg(ctx, args, 0);
    let op = text(&arg(ctx, args, 1), "op")?;
    if !["play", "next", "jump", "stop"].contains(&op.as_str()) {
        return Err(range(format!("unknown op {op}")).into());
    }
    let montage = text(&arg(ctx, args, 2), "montage")?;
    let section = text(&arg(ctx, args, 3), "section")?;
    let num = |i: usize, what: &str| {
        arg(ctx, args, i)
            .as_number()
            .filter(|x| x.is_finite() && *x >= 0.0)
            .ok_or_else(|| range(format!("{what} must be a finite number, 0 or more")))
    };
    let (rate, fade) = (num(4, "rate")?, num(5, "fade")?);
    with_call(ctx, "anim.command", |call, world| {
        let id = target(call, world, &e)?;
        call.commands.push(Command::Anim {
            id,
            param: None,
            command: Some(AnimCommand {
                op,
                montage,
                section,
                rate,
                fade,
            }),
        });
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

/// `animState(e)`: each layer's state and time, and the montage playing, or null.
pub fn info<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let e = arg(ctx, args, 0);
    let s = with_call(ctx, "anim.state", |call, world| {
        let id = target(call, world, &e)?;
        Ok(state(world, id).filter(|s| !s.layers.is_empty()).cloned())
    })?;
    let Some(s) = s else {
        return Ok(Value::new_null(ctx.clone()));
    };
    let layers = Array::new(ctx.clone())?;
    for (i, l) in s.layers.iter().enumerate() {
        let o = Object::new(ctx.clone())?;
        o.set("state", l.state.as_str())?;
        o.set("time", l.time)?;
        layers.set(i, freeze_new(ctx, o)?)?;
    }
    let o = Object::new(ctx.clone())?;
    o.set("layers", freeze_new(ctx, layers.into_object())?)?;
    match s
        .slots
        .iter()
        .find(|x| !x.montage.is_empty() && x.out < 0.0)
    {
        Some(m) => {
            let mo = Object::new(ctx.clone())?;
            mo.set("name", m.montage.as_str())?;
            mo.set("section", m.section.as_str())?;
            mo.set("time", m.time)?;
            o.set("montage", freeze_new(ctx, mo)?)?;
        }
        None => o.set("montage", Value::new_null(ctx.clone()))?,
    }
    Ok(freeze_new(ctx, o)?)
}

/// `animSocket(e, joint)`: a socket's place in the world after the last tick, or null.
pub fn socket<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let e = arg(ctx, args, 0);
    let joint = text(&arg(ctx, args, 1), "a joint's name")?;
    let s = with_call(ctx, "anim.socket", |call, world| {
        let id = target(call, world, &e)?;
        Ok(state(world, id).and_then(|s| s.sockets.iter().find(|x| x.joint == joint).cloned()))
    })?;
    let Some(s) = s else {
        return Ok(Value::new_null(ctx.clone()));
    };
    let list = |v: &[f64]| -> rquickjs::Result<Value<'js>> {
        let a = Array::new(ctx.clone())?;
        for (i, x) in v.iter().enumerate() {
            a.set(i, *x)?;
        }
        Ok(a.into_value())
    };
    let o = Object::new(ctx.clone())?;
    o.set("position", list(&s.position)?)?;
    o.set("rotation", list(&s.rotation)?)?;
    Ok(freeze_new(ctx, o)?)
}

/// Applies a staged parameter or request to the entity's state (inserting it before the graph's
/// first evaluation: the engine keeps what was set then).
pub(crate) fn apply(
    world: &mut World,
    id: EntityId,
    param: Option<(String, f64)>,
    command: Option<AnimCommand>,
) {
    let Some(e) = entity::entity(world, id) else {
        return;
    };
    if world.get::<AnimState>(e).is_none() {
        world.entity_mut(e).insert(AnimState::default());
    }
    let Some(mut s) = world.get_mut::<AnimState>(e) else {
        return;
    };
    if let Some((name, value)) = param {
        match s.params.iter_mut().find(|p| p.name == name) {
            Some(p) => {
                if p.value != value {
                    p.value = value;
                }
            }
            None => s
                .params
                .push(amoris_assets::anim::AnimParam { name, value }),
        }
    }
    if let Some(c) = command {
        s.commands.push(c);
    }
}
