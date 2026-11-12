//! `world.*` and `ctx.single` (docs/spec/script-host.md 5.3 and 5.4): reads see the overlay of the
//! call's own commands, writes are validated now and staged for the commit, and nothing reaches
//! the world during the call.

use amoris_sim::entity::{self, EntityIndex};
use amoris_sim::{EntityAllocator, EntityId};
use bevy_ecs::prelude::World;
use rquickjs::{Ctx, Value};
use serde_json::json;

use super::{NResult, arg, with_call};
use crate::access::defaults;
use crate::call::{CallState, Command, Comp, not_held, parent_slot, resolve_comp};
use crate::convert::{self, is_plain_object};
use crate::error::{ErrorPhase, ScriptError, num};
use crate::js;

/// An entity argument: a valid id, or `None`.
pub fn entity_arg(v: &Value<'_>) -> Option<EntityId> {
    v.as_number().and_then(|x| EntityId::from_f64(x).ok())
}

/// A component name argument.
fn name_arg(v: &Value<'_>) -> Result<String, ScriptError> {
    match js::text(v) {
        Some(Ok(s)) => Ok(s),
        _ => Err(ScriptError::new(
            "script.unknown_component",
            "A component is named by a string, such as \"Transform\".",
            ErrorPhase::Run,
        )),
    }
}

fn missing(v: &Value<'_>) -> ScriptError {
    let shown = v.as_number().map_or(json!(null), num);
    ScriptError::new(
        "script.entity_missing",
        format!(
            "Entity {shown} does not exist (it was despawned, never spawned, or is not an id)."
        ),
        ErrorPhase::Run,
    )
    .value(shown.clone())
    .with("id", shown)
}

/// The live entity an argument names, in the overlay.
fn live(call: &CallState, world: &World, v: &Value<'_>) -> Result<EntityId, ScriptError> {
    match entity_arg(v) {
        Some(id) if call.overlay.alive(world, id) => Ok(id),
        _ => Err(missing(v)),
    }
}

pub fn exists<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let e = arg(ctx, args, 0);
    let alive = with_call(ctx, "world.exists", |call, world| {
        Ok(entity_arg(&e).is_some_and(|id| call.overlay.alive(world, id)))
    })?;
    Ok(Value::new_bool(ctx.clone(), alive))
}

pub fn has<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let (e, c) = (arg(ctx, args, 0), arg(ctx, args, 1));
    let has = with_call(ctx, "world.has", |call, world| {
        let comp = resolve_comp(world, &name_arg(&c)?)?;
        Ok(entity_arg(&e).is_some_and(|id| call.overlay.value(world, id, &comp).is_some()))
    })?;
    Ok(Value::new_bool(ctx.clone(), has))
}

pub fn get<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let (e, c) = (arg(ctx, args, 0), arg(ctx, args, 1));
    let found = with_call(ctx, "world.get", |call, world| {
        let comp = resolve_comp(world, &name_arg(&c)?)?;
        Ok(entity_arg(&e)
            .and_then(|id| call.overlay.value(world, id, &comp))
            .map(|v| (comp.schema, v)))
    })?;
    match found {
        Some((schema, v)) => Ok(convert::component_to_js(ctx, &schema, &v)?),
        None => Ok(Value::new_undefined(ctx.clone())),
    }
}

fn with_entity(e: ScriptError, id: EntityId) -> ScriptError {
    e.entity(id.to_f64())
}

/// A write of `Parent` that puts `id` under the entity in `slot` of `value`: refused with
/// `script.parent_cycle` when that entity is `id` or is under it as the call's commands so far
/// leave the hierarchy, else followed in the call's hierarchy. The slot holds a live entity's id:
/// the field's own check and `Parent`'s slot rule ran before.
fn reparent(
    call: &mut CallState,
    world: &World,
    id: EntityId,
    value: &amoris_sim::registry::ProjectValues,
    slot: usize,
) -> Result<(), ScriptError> {
    let Ok(parent) = EntityId::from_f64(value.nums[slot]) else {
        return Ok(());
    };
    let h = call.hierarchy(world);
    if let Some(chain) = h.loop_through(id, parent) {
        let p = entity::hierarchy::parent_cycle(id, parent, &chain);
        let chain: Vec<u64> = chain.iter().map(|c| c.get()).collect();
        return Err(with_entity(
            ScriptError::new("script.parent_cycle", p.message, ErrorPhase::Run)
                .component("Parent")
                .field("parent")
                .value(num(parent.to_f64()))
                .with("chain", json!(chain))
                .hint("remove or change the Parent of the entity under it first"),
            id,
        ));
    }
    h.set(id, parent);
    Ok(())
}

pub fn set<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let (e, c, p) = (arg(ctx, args, 0), arg(ctx, args, 1), arg(ctx, args, 2));
    with_call(ctx, "world.set", |call, world| {
        let id = live(call, world, &e)?;
        let comp = resolve_comp(world, &name_arg(&c)?)?;
        let Some(mut value) = call.overlay.value(world, id, &comp) else {
            return Err(with_entity(
                ScriptError::new(
                    "script.component_missing",
                    format!(
                        "Entity {} has no {}; insert it first.",
                        id.get(),
                        comp.schema.name
                    ),
                    ErrorPhase::Run,
                )
                .component(&comp.schema.name)
                .hint(format!(
                    "ctx.world.insert(e, \"{}\", {{...}}) adds a component",
                    comp.schema.name
                )),
                id,
            ));
        };
        let overlay = &call.overlay;
        let patch = convert::patch_from_js(ctx, &comp.schema, &p, &|x| overlay.alive(world, x))
            .map_err(|err| with_entity(err, id))?;
        let parent = parent_slot(world, &comp).map(|slot| (slot, value.nums[slot]));
        match comp.rule {
            // The rule sees the value before and after the patch, and the overlay takes the value
            // as the rule settles it: what the component will read once the patch is written.
            Some(rule) => {
                let before = value.clone();
                patch.apply(&mut value);
                rule(Some(&before), &mut value)
                    .map_err(|r| with_entity(not_held(&comp, r, ErrorPhase::Run), id))?;
            }
            None => patch.apply(&mut value),
        }
        if let Some((slot, before)) = parent
            && value.nums[slot].to_bits() != before.to_bits()
        {
            reparent(call, world, id, &value, slot)?;
        }
        call.overlay.put(id, comp.id, Some(value));
        call.commands.push(Command::Set {
            id,
            comp,
            nums: patch.nums,
            strs: patch.strs,
        });
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

pub fn insert<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let (e, c, p) = (arg(ctx, args, 0), arg(ctx, args, 1), arg(ctx, args, 2));
    with_call(ctx, "world.insert", |call, world| {
        let id = live(call, world, &e)?;
        let comp = resolve_comp(world, &name_arg(&c)?)?;
        if call.overlay.value(world, id, &comp).is_some() {
            return Err(with_entity(
                ScriptError::new(
                    "script.component_present",
                    format!(
                        "Entity {} already has {}; set it instead.",
                        id.get(),
                        comp.schema.name
                    ),
                    ErrorPhase::Run,
                )
                .component(&comp.schema.name),
                id,
            ));
        }
        let overlay = &call.overlay;
        let patch = convert::patch_from_js(ctx, &comp.schema, &p, &|x| overlay.alive(world, x))
            .map_err(|err| with_entity(err, id))?;
        let mut value = defaults(&comp.schema);
        patch.apply(&mut value);
        if let Some(rule) = comp.rule {
            rule(None, &mut value)
                .map_err(|r| with_entity(not_held(&comp, r, ErrorPhase::Run), id))?;
        }
        if let Some(slot) = parent_slot(world, &comp) {
            reparent(call, world, id, &value, slot)?;
        }
        call.overlay.put(id, comp.id, Some(value.clone()));
        call.commands.push(Command::Insert { id, comp, value });
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

pub fn remove<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let (e, c) = (arg(ctx, args, 0), arg(ctx, args, 1));
    with_call(ctx, "world.remove", |call, world| {
        let id = live(call, world, &e)?;
        let comp = resolve_comp(world, &name_arg(&c)?)?;
        if call.overlay.value(world, id, &comp).is_some() {
            if parent_slot(world, &comp).is_some() {
                call.hierarchy(world).clear(id);
            }
            call.overlay.put(id, comp.id, None);
            call.commands.push(Command::Remove { id, comp });
        }
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

pub fn despawn<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let e = arg(ctx, args, 0);
    with_call(ctx, "world.despawn", |call, world| {
        let allocated =
            entity_arg(&e).filter(|id| world.resource::<EntityAllocator>().allocated(*id));
        let Some(id) = allocated else {
            return Err(missing(&e));
        };
        if call.overlay.alive(world, id) {
            // The entities under it go with it, here as at the commit (`entity::despawn_in`).
            let h = call.hierarchy(world);
            let mut gone = h.descendants(id);
            gone.push(id);
            for id in &gone {
                h.clear(*id);
            }
            for id in gone {
                call.overlay.despawn(id);
            }
            call.commands.push(Command::Despawn { id });
        }
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

pub fn spawn<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let spec = arg(ctx, args, 0);
    let id = with_call(ctx, "world.spawn", |call, world| {
        let mut comps: Vec<(Comp, amoris_sim::registry::ProjectValues)> = Vec::new();
        if !spec.is_undefined() {
            if !is_plain_object(ctx, &spec) {
                return Err(ScriptError::new(
                    "script.bad_data",
                    "spawn takes an object of components, such as { Crate: { value: 3 } }.",
                    ErrorPhase::Run,
                ));
            }
            for name in js::own_keys(ctx, &spec, true) {
                let comp = resolve_comp(world, &name)?;
                let p = match js::own(ctx, &spec, &name) {
                    js::Own::Data(v) => v,
                    _ => {
                        return Err(ScriptError::new(
                            "script.bad_data",
                            format!("The value for {name} is given by a getter."),
                            ErrorPhase::Run,
                        )
                        .component(&name));
                    }
                };
                let overlay = &call.overlay;
                let patch =
                    convert::patch_from_js(ctx, &comp.schema, &p, &|x| overlay.alive(world, x))?;
                let mut value = defaults(&comp.schema);
                patch.apply(&mut value);
                // A component's rule runs after its own fields and before the next component.
                if let Some(rule) = comp.rule {
                    rule(None, &mut value).map_err(|r| not_held(&comp, r, ErrorPhase::Run))?;
                }
                comps.push((comp, value));
            }
        }
        let id = entity::reserve(world)
            .map_err(|p| ScriptError::new(&p.code, p.message, ErrorPhase::Run))?;
        // A new entity has nothing under it, so its `Parent` closes no loop.
        for (comp, value) in &comps {
            if let Some(slot) = parent_slot(world, comp)
                && let Ok(parent) = EntityId::from_f64(value.nums[slot])
            {
                call.hierarchy(world).set(id, parent);
            }
        }
        call.overlay.spawn(id, &comps);
        call.commands.push(Command::Spawn { id, comps });
        Ok(id)
    })?;
    Ok(Value::new_number(ctx.clone(), id.to_f64()))
}

pub fn single<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let c = arg(ctx, args, 0);
    let id = with_call(ctx, "ctx.single", |_call, world| {
        let comp = resolve_comp(world, &name_arg(&c)?)?;
        let mut found = Vec::new();
        for (id, e) in world.resource::<EntityIndex>().iter() {
            if world.entity(e).contains_id(comp.id) {
                found.push(id);
                if found.len() > 2 {
                    break;
                }
            }
        }
        match found.as_slice() {
            [one] => Ok(*one),
            _ => {
                let count = world
                    .resource::<EntityIndex>()
                    .iter()
                    .filter(|(_, e)| world.entity(*e).contains_id(comp.id))
                    .count();
                Err(ScriptError::new(
                    "script.single_count",
                    format!(
                        "single(\"{}\") needs exactly one entity with it; there are {count}.",
                        comp.schema.name
                    ),
                    ErrorPhase::Run,
                )
                .component(&comp.schema.name)
                .with("count", json!(count)))
            }
        }
    })?;
    Ok(Value::new_number(ctx.clone(), id.to_f64()))
}
