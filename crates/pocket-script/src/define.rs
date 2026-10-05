//! Reading the game definition (docs/spec/script-host.md 4.2 and 7.3): the entry module's default
//! export is a `game(...)` value; its components become `ComponentSchema`s of origin Project, its
//! systems the program's systems in array order. Any other key in any builder is refused with
//! `script.unknown_key` and the nearest key.

use std::sync::Arc;

use bevy_ecs::prelude::World;
use pocket_sim::registry::{ComponentOrigin, ComponentSchema, FieldType, FieldValue};
use pocket_sim::{ComponentRegistry, EntityId, RunCondition, SystemKey, Tick};
use rquickjs::{Ctx, Function, Value};
use serde_json::json;

use crate::error::{ErrorPhase, ScriptError};
use crate::js::{self, Own};
use crate::natives::query::{Spec, parse_spec};

/// A system as the program declares it.
pub struct SystemDef<'js> {
    pub name: String,
    pub key: SystemKey,
    pub doc: String,
    pub when: RunCondition,
    pub queries: Vec<(String, Spec)>,
    pub events: Vec<String>,
    pub def: Value<'js>,
    pub run: Function<'js>,
}

/// The game a program declares.
pub struct GameDef<'js> {
    pub components: Vec<Arc<ComponentSchema>>,
    pub systems: Vec<SystemDef<'js>>,
}

fn bad(message: String) -> ScriptError {
    ScriptError::new("script.bad_definition", message, ErrorPhase::Load)
}

fn unknown_key(what: &str, key: &str, known: &[&str]) -> ScriptError {
    ScriptError::new(
        "script.unknown_key",
        format!("{what} has no key '{key}'."),
        ErrorPhase::Load,
    )
    .with("key", json!(key))
    .suggest(pocket_contract::suggest::suggest_names(
        key,
        known.iter().copied(),
    ))
}

/// An object's own enumerable data properties in own-key order; a getter is refused.
fn entries<'js>(
    ctx: &Ctx<'js>,
    v: &Value<'js>,
    what: &str,
) -> Result<Vec<(String, Value<'js>)>, ScriptError> {
    if !v.is_object() || js::is_proxy(v) {
        return Err(bad(format!("{what} must be an object.")));
    }
    let mut out = Vec::new();
    for k in js::own_keys(ctx, v, true) {
        match js::own(ctx, v, &k) {
            Own::Data(x) => out.push((k, x)),
            _ => {
                return Err(bad(format!(
                    "{what}'s '{k}' is a getter; give plain values."
                )));
            }
        }
    }
    Ok(out)
}

/// The most items a list of the game definition holds.
const MAX_ITEMS: usize = 4096;

fn items<'js>(ctx: &Ctx<'js>, v: &Value<'js>, what: &str) -> Result<Vec<Value<'js>>, ScriptError> {
    let len = match js::dense_array_len(ctx, v, MAX_ITEMS) {
        Ok(n) => n,
        Err(js::NotDense::TooLong(n)) => {
            return Err(bad(format!(
                "{what} has {} items; at most {MAX_ITEMS} are allowed.",
                crate::error::num(n)
            )));
        }
        Err(js::NotDense::Shape) => return Err(bad(format!("{what} must be an array."))),
    };
    (0..len)
        .map(|i| match js::own(ctx, v, &i.to_string()) {
            Own::Data(x) => Ok(x),
            _ => Err(bad(format!("{what} has a getter at {i}."))),
        })
        .collect()
}

fn text(v: &Value<'_>, what: &str) -> Result<String, ScriptError> {
    match js::text(v) {
        Some(Ok(t)) => Ok(t),
        _ => Err(bad(format!("{what} must be a string."))),
    }
}

/// The builder value's tag and payload: `{__pocket: tag, ...}`.
fn tagged<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> Option<(String, Vec<(String, Value<'js>)>)> {
    let e = entries(ctx, v, "a builder value").ok()?;
    let tag = e
        .iter()
        .find(|(k, _)| k == "__pocket")
        .and_then(|(_, t)| js::text(t)?.ok())?;
    Some((tag, e))
}

fn number(v: &Value<'_>, what: &str) -> Result<f64, ScriptError> {
    v.as_number()
        .ok_or_else(|| bad(format!("{what} must be a number.")))
}

fn vector<'js>(
    ctx: &Ctx<'js>,
    v: &Value<'js>,
    parts: &[&str],
    what: &str,
) -> Result<Vec<f64>, ScriptError> {
    let mut out = Vec::new();
    for p in parts {
        match js::own(ctx, v, p) {
            Own::Data(x) => out.push(number(&x, &format!("{what}.{p}"))?),
            _ => return Err(bad(format!("{what} needs the part '{p}'."))),
        }
    }
    Ok(out)
}

/// One field builder's value: its type and default.
fn field<'js>(
    ctx: &Ctx<'js>,
    v: &Value<'js>,
    what: &str,
) -> Result<(FieldType, FieldValue, String), ScriptError> {
    let Some((tag, e)) = tagged(ctx, v) else {
        return Err(bad(format!(
            "{what} must be made by a field builder, such as field.f64(0, \"doc\")."
        )));
    };
    if tag != "field" {
        return Err(bad(format!("{what} must be made by a field builder.")));
    }
    let get = |k: &str| e.iter().find(|(n, _)| n == k).map(|(_, x)| x.clone());
    let ty = text(
        &get("type").unwrap_or_else(|| Value::new_undefined(ctx.clone())),
        what,
    )?;
    let doc = get("doc")
        .and_then(|d| js::text(&d)?.ok())
        .unwrap_or_default();
    let value = get("value").unwrap_or_else(|| Value::new_undefined(ctx.clone()));
    let int = |lo: f64, hi: f64| -> Result<f64, ScriptError> {
        let x = number(&value, &format!("{what}'s default"))?;
        if x.fract() != 0.0 || x < lo || x > hi {
            return Err(bad(format!(
                "{what}'s default {x} is not a whole number from {lo} to {hi}."
            )));
        }
        Ok(x)
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked by `int`
    let parsed = match ty.as_str() {
        "f64" => (FieldType::F64, FieldValue::F64(number(&value, what)?)),
        "i32" => (
            FieldType::I32,
            FieldValue::I32(int(f64::from(i32::MIN), f64::from(i32::MAX))? as i32),
        ),
        "u32" => (
            FieldType::U32,
            FieldValue::U32(int(0.0, f64::from(u32::MAX))? as u32),
        ),
        "tick" => (
            FieldType::Tick,
            FieldValue::Tick(Tick(int(0.0, Tick::MAX.to_f64())? as u64)),
        ),
        "bool" => (
            FieldType::Bool,
            FieldValue::Bool(
                value
                    .as_bool()
                    .ok_or_else(|| bad(format!("{what}'s default must be true or false.")))?,
            ),
        ),
        "str" => (
            FieldType::Str,
            FieldValue::Str(text(&value, &format!("{what}'s default"))?.into()),
        ),
        "entity" => (FieldType::Entity, FieldValue::Entity(None::<EntityId>)),
        "vec2" => {
            let v = vector(ctx, &value, &["x", "y"], what)?;
            (FieldType::Vec2, FieldValue::Vec2([v[0], v[1]]))
        }
        "vec3" => {
            let v = vector(ctx, &value, &["x", "y", "z"], what)?;
            (FieldType::Vec3, FieldValue::Vec3([v[0], v[1], v[2]]))
        }
        "vec4" | "quat" => {
            let v = vector(ctx, &value, &["x", "y", "z", "w"], what)?;
            let a = [v[0], v[1], v[2], v[3]];
            if ty == "quat" {
                (FieldType::Quat, FieldValue::Quat(a))
            } else {
                (FieldType::Vec4, FieldValue::Vec4(a))
            }
        }
        "enum" => {
            let names: Vec<String> = items(
                ctx,
                &get("variants").unwrap_or_else(|| Value::new_undefined(ctx.clone())),
                &format!("{what}'s variants"),
            )?
            .iter()
            .map(|n| text(n, &format!("{what}'s variants")))
            .collect::<Result<_, _>>()?;
            let d = text(&value, &format!("{what}'s default"))?;
            let i = names.iter().position(|n| *n == d).ok_or_else(|| {
                bad(format!(
                    "{what}'s default '{d}' is not one of its variants."
                ))
            })?;
            let variants: Arc<[Box<str>]> = names.into_iter().map(String::into_boxed_str).collect();
            (
                FieldType::Enum(variants),
                FieldValue::Enum(u32::try_from(i).unwrap_or(0)),
            )
        }
        other => return Err(bad(format!("{what} has an unknown field type '{other}'."))),
    };
    Ok((parsed.0, parsed.1, doc))
}

fn component<'js>(
    ctx: &Ctx<'js>,
    v: &Value<'js>,
    world: &World,
) -> Result<ComponentSchema, ScriptError> {
    let Some((tag, e)) = tagged(ctx, v) else {
        return Err(bad(
            "game({components}) takes values made by component(name, {...}).".into(),
        ));
    };
    if tag != "component" {
        return Err(bad(
            "game({components}) takes values made by component(name, {...}).".into(),
        ));
    }
    let name = text(
        &e.iter()
            .find(|(k, _)| k == "name")
            .map(|x| x.1.clone())
            .unwrap_or_else(|| Value::new_undefined(ctx.clone())),
        "A component's name",
    )?;
    let def = e
        .iter()
        .find(|(k, _)| k == "def")
        .map(|x| x.1.clone())
        .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
    let (mut version, mut doc, mut fields) = (None, String::new(), Vec::new());
    for (k, x) in entries(ctx, &def, &format!("component {name}"))? {
        match k.as_str() {
            "version" => version = Some(number(&x, &format!("{name}.version"))?),
            "doc" => doc = text(&x, &format!("{name}.doc"))?,
            "fields" => {
                for (fname, fv) in entries(ctx, &x, &format!("{name}.fields"))? {
                    let (ty, default, fdoc) = field(ctx, &fv, &format!("{name}.{fname}"))?;
                    fields.push((fname, ty, default, fdoc));
                }
            }
            "migrate" => {}
            other => {
                return Err(unknown_key(
                    &format!("component {name}"),
                    other,
                    &["version", "doc", "fields", "migrate"],
                ));
            }
        }
    }
    let version = version.filter(|v| v.fract() == 0.0 && *v >= 1.0 && *v <= f64::from(u32::MAX));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked above
    let version = version.ok_or_else(|| {
        bad(format!(
            "component {name} needs a version, a whole number from 1."
        ))
    })? as u32;
    if let Some(entry) = world
        .get_resource::<ComponentRegistry>()
        .and_then(|r| r.get(&name))
        && entry.origin == pocket_sim::registry::ComponentOrigin::Engine
    {
        return Err(ScriptError::new(
            "script.component_name",
            format!("'{name}' is an engine component; a project component may not shadow it."),
            ErrorPhase::Load,
        )
        .component(&name));
    }
    let borrowed: Vec<(&str, FieldType, FieldValue, &str)> = fields
        .iter()
        .map(|(n, t, d, doc)| (n.as_str(), t.clone(), d.clone(), doc.as_str()))
        .collect();
    ComponentSchema::new(&name, ComponentOrigin::Project, version, &doc, borrowed).map_err(|p| {
        let code = if p.code == "sim.component_name_invalid" {
            "script.component_name"
        } else {
            "script.bad_definition"
        };
        ScriptError::new(code, p.message, ErrorPhase::Load).component(&name)
    })
}

fn condition<'js>(ctx: &Ctx<'js>, v: &Value<'js>, name: &str) -> Result<RunCondition, ScriptError> {
    if v.is_undefined() {
        return Ok(RunCondition::Always);
    }
    if let Some(Ok(t)) = js::text(v) {
        return if t == "start" {
            Ok(RunCondition::Start)
        } else {
            Err(bad(format!(
                "system {name}'s when is \"start\" or {{every, offset}}."
            )))
        };
    }
    let (mut every, mut offset) = (None, 0.0);
    for (k, x) in entries(ctx, v, &format!("system {name}'s when"))? {
        match k.as_str() {
            "every" => every = Some(number(&x, "when.every")?),
            "offset" => offset = number(&x, "when.offset")?,
            other => return Err(unknown_key("when", other, &["every", "offset"])),
        }
    }
    let ok = |x: f64| x.fract() == 0.0 && (0.0..9.0e15).contains(&x);
    match every {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked by `ok`
        Some(p) if ok(p) && ok(offset) => {
            RunCondition::every(p as u64, offset as u64).map_err(|e| bad(e.message))
        }
        _ => Err(bad(format!(
            "system {name}'s when.every must be a whole number from 1."
        ))),
    }
}

fn system<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> Result<SystemDef<'js>, ScriptError> {
    let Some((tag, e)) = tagged(ctx, v) else {
        return Err(bad(
            "game({systems}) takes values made by system({...}).".into()
        ));
    };
    if tag == "executor" {
        return Err(bad(
            "Executors carry out intents, which arrive with the player interface (slice 2).".into(),
        ));
    }
    if tag != "system" {
        return Err(bad(
            "game({systems}) takes values made by system({...}).".into()
        ));
    }
    let def = e
        .iter()
        .find(|(k, _)| k == "def")
        .map(|x| x.1.clone())
        .unwrap_or_else(|| Value::new_undefined(ctx.clone()));
    let fields = entries(ctx, &def, "system({...})")?;
    let get = |k: &str| fields.iter().find(|(n, _)| n == k).map(|(_, x)| x.clone());
    for (k, _) in &fields {
        if !matches!(
            k.as_str(),
            "name" | "phase" | "doc" | "when" | "queries" | "events" | "run"
        ) {
            return Err(unknown_key(
                "system({...})",
                k,
                &["name", "phase", "doc", "when", "queries", "events", "run"],
            ));
        }
    }
    let undef = || Value::new_undefined(ctx.clone());
    let name = text(&get("name").unwrap_or_else(undef), "A system's name")?;
    let valid = name.len() <= 56
        && name.bytes().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_');
    if !valid {
        return Err(bad(format!(
            "'{name}' is not a system name: ^[a-z][a-z0-9_]*$, at most 56 bytes."
        )));
    }
    let phase = get("phase")
        .map(|p| text(&p, "phase"))
        .transpose()?
        .unwrap_or_else(|| "update".into());
    if phase != "update" {
        return Err(bad(format!(
            "system {name}'s phase is \"update\", the only script phase of slice 1."
        )));
    }
    let doc = get("doc")
        .map(|d| text(&d, "doc"))
        .transpose()?
        .unwrap_or_default();
    let when = condition(ctx, &get("when").unwrap_or_else(undef), &name)?;
    let mut queries = Vec::new();
    if let Some(q) = get("queries") {
        for (qname, spec) in entries(ctx, &q, &format!("system {name}'s queries"))? {
            let mut s = parse_spec(ctx, &spec).map_err(|mut e| {
                e.detail.phase = Some(ErrorPhase::Load);
                e
            })?;
            s.with.dedup();
            queries.push((qname, s));
        }
    }
    let events = match get("events") {
        Some(ev) => items(ctx, &ev, "events")?
            .iter()
            .map(|x| text(x, "an event kind"))
            .collect::<Result<_, _>>()?,
        None => Vec::new(),
    };
    let run = get("run")
        .and_then(|r| r.into_function())
        .ok_or_else(|| bad(format!("system {name} needs run(ctx, queries).")))?;
    let key = SystemKey::new(format!("script:{name}")).map_err(|p| bad(p.message))?;
    Ok(SystemDef {
        name,
        key,
        doc,
        when,
        queries,
        events,
        def,
        run,
    })
}

/// Reads and checks the entry's default export.
pub fn game<'js>(
    ctx: &Ctx<'js>,
    default: &Value<'js>,
    world: &World,
) -> Result<GameDef<'js>, ScriptError> {
    let not_game = || {
        ScriptError::new(
            "script.entry_not_game",
            "The entry module's default export must be game({...}) from \"pocket\".",
            ErrorPhase::Load,
        )
    };
    let Some((tag, e)) = tagged(ctx, default) else {
        return Err(not_game());
    };
    if tag != "game" {
        return Err(not_game());
    }
    let def = e
        .iter()
        .find(|(k, _)| k == "def")
        .map(|x| x.1.clone())
        .ok_or_else(not_game)?;
    let mut out = GameDef {
        components: Vec::new(),
        systems: Vec::new(),
    };
    for (k, v) in entries(ctx, &def, "game({...})")? {
        match k.as_str() {
            "components" => {
                for c in items(ctx, &v, "components")? {
                    let schema = component(ctx, &c, world)?;
                    if out.components.iter().any(|s| s.name == schema.name) {
                        return Err(ScriptError::new(
                            "script.component_name",
                            format!("The component '{}' is declared twice.", schema.name),
                            ErrorPhase::Load,
                        )
                        .component(&schema.name));
                    }
                    out.components.push(Arc::new(schema));
                }
            }
            "systems" => {
                for s in items(ctx, &v, "systems")? {
                    let s = system(ctx, &s)?;
                    if out.systems.iter().any(|x| x.name == s.name) {
                        return Err(ScriptError::new(
                            "script.system_duplicate",
                            format!("Two systems are named '{}'.", s.name),
                            ErrorPhase::Load,
                        )
                        .with("system", json!(s.name)));
                    }
                    out.systems.push(s);
                }
            }
            "retired" => {}
            other => {
                return Err(unknown_key(
                    "game({...})",
                    other,
                    &["components", "systems", "retired"],
                ));
            }
        }
    }
    // Every component a query names is the engine's or this game's.
    let known = |n: &str| {
        out.components.iter().any(|c| &*c.name == n)
            || world
                .get_resource::<ComponentRegistry>()
                .and_then(|r| r.get(n))
                .is_some_and(|e| e.script.is_some())
    };
    for s in &out.systems {
        for (_, q) in &s.queries {
            for n in q.with.iter().chain(&q.without) {
                if !known(n) {
                    let mut names: Vec<String> =
                        out.components.iter().map(|c| c.name.to_string()).collect();
                    if let Some(r) = world.get_resource::<ComponentRegistry>() {
                        names.extend(
                            r.entries()
                                .iter()
                                .filter(|e| e.script.is_some())
                                .map(|e| e.name.to_string()),
                        );
                    }
                    return Err(ScriptError::new(
                        "script.unknown_component",
                        format!(
                            "system {} queries '{n}', which is not a component scripts can use.",
                            s.name
                        ),
                        ErrorPhase::Load,
                    )
                    .component(n)
                    .suggest(pocket_contract::suggest::suggest_names(
                        n,
                        names.iter().map(String::as_str),
                    )));
                }
            }
        }
    }
    Ok(out)
}
