//! `ctx.anim` (docs/spec/animation.md 4, 5a and 6a): an entity's graph's parameters, montage
//! requests, state, sockets, morph target weights and IK goals. Parameters, requests, weight overrides and IK goals are staged
//! until the system returns, as every write; reads see the world as the system started, with this
//! call's own parameters and overrides over it.

use amoris_assets::anim::{AnimCommand, AnimIk, AnimState, IkGoal, MorphWeight, MorphWeights};
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

/// `animWeight(e, name)`: a morph target's weight after the last evaluation, this call's own
/// override over it, or null when the entity has no target of that name.
pub fn weight<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let e = arg(ctx, args, 0);
    let name = text(&arg(ctx, args, 1), "a target's name")?;
    let v = with_call(ctx, "anim.weight", |call, world| {
        let id = target(call, world, &e)?;
        let last = entity::entity(world, id)
            .and_then(|x| world.get::<MorphWeights>(x))
            .and_then(|m| m.weights.iter().find(|w| w.name == name).map(|w| w.weight));
        let staged = call.commands.iter().rev().find_map(|c| match c {
            Command::Morph {
                id: at,
                name: n,
                weight: Some(w),
            } if *at == id && *n == name => Some(Some(*w)),
            Command::Morph {
                id: at,
                name: n,
                weight: None,
            } if *at == id && *n == name => Some(None),
            _ => None,
        });
        Ok(match staged {
            Some(Some(w)) if last.is_some() => Some(w),
            _ => last,
        })
    })?;
    Ok(match v {
        Some(w) => Value::new_number(ctx.clone(), w),
        None => Value::new_null(ctx.clone()),
    })
}

/// `animSetWeight(e, name, weight)`: holds every morph target of that name at `weight` from the
/// next evaluation on; null gives it back to the graph.
pub fn set_weight<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let e = arg(ctx, args, 0);
    let name = text(&arg(ctx, args, 1), "a target's name")?;
    let v = arg(ctx, args, 2);
    let weight = if v.is_null() || v.is_undefined() {
        None
    } else {
        Some(
            v.as_number()
                .filter(|x| x.is_finite())
                .ok_or_else(|| range(format!("target {name} takes a finite number or null")))?,
        )
    };
    with_call(ctx, "anim.setWeight", |call, world| {
        let id = target(call, world, &e)?;
        call.commands.push(Command::Morph { id, name, weight });
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

/// Applies a staged weight override, or its release, to the entity's `MorphWeights` (inserting
/// it: the graph's evaluation keeps the overrides and fills in the weights).
pub(crate) fn apply_weight(world: &mut World, id: EntityId, name: String, weight: Option<f64>) {
    let Some(e) = entity::entity(world, id) else {
        return;
    };
    if world.get::<MorphWeights>(e).is_none() {
        if weight.is_none() {
            return;
        }
        world.entity_mut(e).insert(MorphWeights::default());
    }
    let Some(mut m) = world.get_mut::<MorphWeights>(e) else {
        return;
    };
    let at = m.overrides.iter().position(|o| o.name == name);
    match (at, weight) {
        (Some(i), Some(w)) => {
            if m.overrides[i].weight != w {
                m.overrides[i].weight = w;
            }
        }
        (None, Some(w)) => m.overrides.push(MorphWeight { name, weight: w }),
        (Some(i), None) => {
            m.overrides.remove(i);
        }
        (None, None) => {}
    }
}

/// An optional point: null or undefined, or three finite numbers.
fn point(v: &Value<'_>, what: &str) -> Result<Option<[f64; 3]>, ScriptError> {
    if v.is_null() || v.is_undefined() {
        return Ok(None);
    }
    let bad = || {
        range(format!(
            "{what} must be [x, y, z] of finite numbers, or null"
        ))
    };
    let a = v.as_array().ok_or_else(bad)?;
    if a.len() != 3 {
        return Err(bad());
    }
    let mut out = [0.0; 3];
    for (i, x) in out.iter_mut().enumerate() {
        *x = a
            .get::<Value<'_>>(i)
            .ok()
            .and_then(|x| x.as_number())
            .filter(|x| x.is_finite())
            .ok_or_else(bad)?;
    }
    Ok(Some(out))
}

/// `animIk(e, node)`: the IK node's goal as set, this call's own over it, or null.
pub fn ik<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let e = arg(ctx, args, 0);
    let node = text(&arg(ctx, args, 1), "an IK node's name")?;
    let g = with_call(ctx, "anim.ik", |call, world| {
        let id = target(call, world, &e)?;
        let staged = call.commands.iter().rev().find_map(|c| match c {
            Command::Ik {
                id: at,
                goal,
                release,
            } if *at == id && goal.node == node => Some((!*release).then(|| goal.clone())),
            _ => None,
        });
        Ok(staged.unwrap_or_else(|| {
            entity::entity(world, id)
                .and_then(|x| world.get::<AnimIk>(x))
                .and_then(|k| k.goals.iter().rev().find(|g| g.node == node).cloned())
        }))
    })?;
    let Some(g) = g else {
        return Ok(Value::new_null(ctx.clone()));
    };
    let list = |v: Option<[f64; 3]>| -> rquickjs::Result<Value<'js>> {
        let Some(v) = v else {
            return Ok(Value::new_null(ctx.clone()));
        };
        let a = Array::new(ctx.clone())?;
        for (i, x) in v.iter().enumerate() {
            a.set(i, *x)?;
        }
        freeze_new(ctx, a.into_object())
    };
    let o = Object::new(ctx.clone())?;
    o.set("target", list(g.target)?)?;
    o.set("pole", list(g.pole)?)?;
    o.set("weight", g.weight)?;
    Ok(freeze_new(ctx, o)?)
}

/// `animSetIk(e, node, release, target, pole, weight)`: the IK node's goal from the next
/// evaluation on, or its release.
pub fn set_ik<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let e = arg(ctx, args, 0);
    let node = text(&arg(ctx, args, 1), "an IK node's name")?;
    let release = arg(ctx, args, 2).as_bool().unwrap_or(false);
    let target_at = point(&arg(ctx, args, 3), "an IK goal's target")?;
    let pole = point(&arg(ctx, args, 4), "an IK goal's pole")?;
    let weight = arg(ctx, args, 5)
        .as_number()
        .filter(|x| x.is_finite() && (0.0..=1.0).contains(x))
        .ok_or_else(|| range("an IK goal's weight must be a number from 0 to 1".into()))?;
    with_call(ctx, "anim.setIk", |call, world| {
        let id = target(call, world, &e)?;
        call.commands.push(Command::Ik {
            id,
            goal: IkGoal {
                node,
                target: target_at,
                pole,
                weight,
            },
            release,
        });
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

/// Applies a staged IK goal, or its release, to the entity's `AnimIk` (inserting it: `anim.ik`
/// keeps the goals and fills in the feet's state).
pub(crate) fn apply_ik(world: &mut World, id: EntityId, goal: IkGoal, release: bool) {
    let Some(e) = entity::entity(world, id) else {
        return;
    };
    if world.get::<AnimIk>(e).is_none() {
        if release {
            return;
        }
        world.entity_mut(e).insert(AnimIk::default());
    }
    let Some(mut k) = world.get_mut::<AnimIk>(e) else {
        return;
    };
    let at = k.goals.iter().position(|g| g.node == goal.node);
    match (at, release) {
        (Some(i), false) => {
            if k.goals[i] != goal {
                k.goals[i] = goal;
            }
        }
        (None, false) => k.goals.push(goal),
        (Some(i), true) => {
            k.goals.remove(i);
        }
        (None, true) => {}
    }
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
