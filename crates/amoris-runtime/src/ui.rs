//! The game UI's commands (docs/spec/ui.md 7): `ui.press`, the recorded write a pointer's press on
//! a button becomes in the native window and the browser, which an agent's seat sends too, and
//! `ui.list`, the screen's elements as an agent reads them. Also which elements an invisible stack
//! hides, for these and for the render feed's extraction.

use std::collections::BTreeSet;

use amoris_assets::{UiBar, UiButton, UiDirection, UiImage, UiStack, UiText};
use amoris_contract::{Pointer, Problem, detail};
use amoris_sim::event::{EventKind, NewEvent};
use amoris_sim::{EntityId, EntityIndex, Name, Parent, PlainData};
use bevy_ecs::prelude::World;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::values::{EntityRef, resolve};

/// `ui.press {entity}`.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiPressParams {
    /// The button: its id, name or `Name#id`.
    pub entity: EntityRef,
}

/// `ui.list {}`.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiListParams {}

/// The most elements `ui.list` answers with.
pub const LIST_MAX: usize = 256;

/// Hierarchy steps followed up from an element; deeper ones are not looked at.
const DEPTH: usize = 64;

/// Every entity that is an invisible stack or is under one: its elements are neither drawn nor
/// pressed (ui.md 3, Visibility).
pub fn hidden(world: &World) -> BTreeSet<EntityId> {
    let index = world.resource::<EntityIndex>();
    let off: BTreeSet<EntityId> = index
        .iter()
        .filter(|(_, e)| world.get::<UiStack>(*e).is_some_and(|s| !s.visible))
        .map(|(id, _)| id)
        .collect();
    let mut out = BTreeSet::new();
    if off.is_empty() {
        return out;
    }
    for (id, _) in index.iter() {
        let mut at = id;
        for _ in 0..DEPTH {
            if off.contains(&at) {
                out.insert(id);
                break;
            }
            match index
                .get(at)
                .and_then(|e| world.get::<Parent>(e))
                .map(|p| p.parent)
            {
                Some(p) => at = p,
                None => break,
            }
        }
    }
    out
}

fn refused(code: &str, message: String, entity: EntityId) -> Problem {
    Problem::new(code, message, detail([("entity", json!(entity.get()))]))
}

/// Applies `ui.press` at the boundary: the button's presses counted, a toggle flipped, and
/// `ui.pressed {on, presses}` emitted with it as subject. Answers `{entity, on, presses}` and the
/// canonical recorded form `{"entity": id}`; refused, it changed nothing.
pub fn press(b: &mut amoris_sim::Boundary<'_>, params: &Value) -> Result<(Value, Value), Problem> {
    let p: UiPressParams = crate::decode(params, "ui.press")?;
    let id = resolve(b.world(), &p.entity, &Pointer::root().key("entity"))?;
    let entity = amoris_sim::entity::require(b.world(), id)?;
    let Some(button) = b.world().get::<UiButton>(entity).cloned() else {
        return Err(refused(
            "ui.not_a_button",
            format!("Entity {} has no UiButton to press.", id.get()),
            id,
        ));
    };
    if !button.enabled {
        return Err(refused(
            "ui.disabled",
            format!("The button {} is disabled.", id.get()),
            id,
        ));
    }
    if !button.visible || hidden(b.world()).contains(&id) {
        return Err(refused(
            "ui.hidden",
            format!("The button {} is not on the screen.", id.get()),
            id,
        ));
    }
    let presses = button.presses.saturating_add(1);
    let on = if button.toggle { !button.on } else { button.on };
    if let Some(mut c) = b.world_mut().get_mut::<UiButton>(entity) {
        c.presses = presses;
        c.on = on;
    }
    let data = PlainData::object(vec![
        ("on".into(), PlainData::Bool(on)),
        ("presses".into(), PlainData::Number(f64::from(presses))),
    ])?;
    b.emit(
        NewEvent::new(EventKind::new("ui.pressed")?)
            .subject(id)
            .data(data),
    );
    Ok((
        json!({"entity": id.get(), "on": on, "presses": presses}),
        json!({"entity": id.get()}),
    ))
}

/// `ui.list`: the visible elements in id order (ui.md 7).
pub fn list(world: &World, params: &Value) -> Result<Value, Problem> {
    crate::decode::<UiListParams>(params, "ui.list")?;
    let hidden = hidden(world);
    let mut rows: Vec<(EntityId, Value)> = Vec::new();
    for (eid, e) in world.resource::<EntityIndex>().iter() {
        let id = &eid;
        let (name, parent) = (world.get::<Name>(e), world.get::<Parent>(e));
        let (text, bar, image, button, stack) = (
            world.get::<UiText>(e),
            world.get::<UiBar>(e),
            world.get::<UiImage>(e),
            world.get::<UiButton>(e),
            world.get::<UiStack>(e),
        );
        if hidden.contains(id) {
            continue;
        }
        let base = |kind: &str| {
            let mut m = Map::new();
            m.insert("entity".into(), json!(id.get()));
            m.insert("kind".into(), json!(kind));
            if let Some(n) = name {
                m.insert("name".into(), json!(n.as_str()));
            }
            if let Some(p) = parent {
                m.insert("parent".into(), json!(p.parent.get()));
            }
            m
        };
        if let Some(s) = stack.filter(|s| s.visible) {
            let mut m = base("stack");
            let d = match s.direction {
                UiDirection::Column => "column",
                UiDirection::Row => "row",
            };
            m.insert("direction".into(), json!(d));
            rows.push((*id, Value::Object(m)));
        }
        let label = text.filter(|t| t.visible).map(|t| t.text.clone());
        if let Some(k) = button.filter(|k| k.visible) {
            let mut m = base("button");
            m.insert("label".into(), json!(label.clone().unwrap_or_default()));
            m.insert("enabled".into(), json!(k.enabled));
            m.insert("toggle".into(), json!(k.toggle));
            m.insert("on".into(), json!(k.on));
            m.insert("presses".into(), json!(k.presses));
            rows.push((*id, Value::Object(m)));
        }
        if let Some(i) = image.filter(|i| i.visible && !i.image.is_empty()) {
            let mut m = base("image");
            m.insert("image".into(), json!(i.image));
            rows.push((*id, Value::Object(m)));
        }
        // A button's text is its label, listed with it.
        if button.is_none()
            && let Some(t) = text.filter(|t| t.visible && !t.text.is_empty())
        {
            let mut m = base("text");
            m.insert("text".into(), json!(t.text));
            rows.push((*id, Value::Object(m)));
        }
        if let Some(r) = bar.filter(|r| r.visible) {
            let mut m = base("bar");
            m.insert("value".into(), json!(r.value));
            m.insert("max".into(), json!(r.max));
            rows.push((*id, Value::Object(m)));
        }
    }
    let omitted = rows.len().saturating_sub(LIST_MAX);
    let elements: Vec<Value> = rows.into_iter().take(LIST_MAX).map(|(_, v)| v).collect();
    let mut out = json!({"elements": elements});
    if omitted > 0 {
        out["omitted"] = json!(omitted);
    }
    Ok(out)
}
