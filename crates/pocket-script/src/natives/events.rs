//! Events and intents (docs/spec/script-host.md 5.5): `ctx.emit` stages an event appended at the
//! commit, `ctx.events` reads the inbox's events of the kinds a system declares.

use pocket_sim::event::{EventInbox, EventKind, EventSeq, NewEvent};
use pocket_sim::{EntityId, Event, PlainData};
use rquickjs::{Array, Ctx, Object, Value};
use serde_json::json;

use super::{NResult, arg, with_call};
use crate::call::Command;
use crate::convert::{freeze_new, is_plain_object, plain_from_js, plain_to_js};
use crate::error::{ErrorPhase, ScriptError};
use crate::js::{self, Own};

/// Kind prefixes the engine and the contract reserve (simulation.md 5; shared/contract).
pub const RESERVED: &[&str] = &[
    "entity.",
    "component.",
    "script.",
    "scripts.",
    "sim.",
    "intent.",
    "contact.",
    "physics.",
    "interface.",
    "time.",
    "decision.",
    "perception.",
];

pub fn emit<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let (k, d, o) = (arg(ctx, args, 0), arg(ctx, args, 1), arg(ctx, args, 2));
    let kind = match js::text(&k) {
        Some(Ok(t)) => t,
        _ => {
            return Err(ScriptError::new(
                "sim.event_kind_invalid",
                "An event kind is a string such as \"crate.taken\".",
                ErrorPhase::Run,
            )
            .into());
        }
    };
    let kind =
        EventKind::new(kind).map_err(|p| ScriptError::new(&p.code, p.message, ErrorPhase::Run))?;
    if let Some(prefix) = RESERVED.iter().find(|p| kind.as_str().starts_with(**p)) {
        return Err(ScriptError::new(
            "script.event_reserved",
            format!("Event kinds starting with '{prefix}' are the engine's; name the event after your game."),
            ErrorPhase::Run,
        )
        .with("kind", json!(kind.as_str()))
        .into());
    }
    let data = if d.is_undefined() {
        PlainData::Null
    } else {
        plain_from_js(ctx, &d)?
    };
    let mut event = NewEvent::new(kind).data(data);
    if !(o.is_undefined() || o.is_null()) {
        if !is_plain_object(ctx, &o) {
            return Err(ScriptError::new(
                "script.bad_data",
                "emit's options are an object such as { subject: e }.",
                ErrorPhase::Run,
            )
            .into());
        }
        for key in js::own_keys(ctx, &o, true) {
            let v = match js::own(ctx, &o, &key) {
                Own::Data(v) => v,
                _ => {
                    return Err(ScriptError::new(
                        "script.bad_data",
                        format!("emit's option '{key}' is a getter."),
                        ErrorPhase::Run,
                    )
                    .into());
                }
            };
            match key.as_str() {
                "subject" => {
                    if v.is_undefined() || v.is_null() {
                        continue;
                    }
                    let id = v
                        .as_number()
                        .and_then(|x| EntityId::from_f64(x).ok())
                        .ok_or_else(|| {
                            ScriptError::new(
                                "script.entity_missing",
                                "An event's subject is an entity.",
                                ErrorPhase::Run,
                            )
                        })?;
                    event = event.subject(id);
                }
                "cause" => {
                    if v.is_undefined() || v.is_null() {
                        continue;
                    }
                    let seq = v
                        .as_number()
                        .filter(|x| x.fract() == 0.0 && *x >= 1.0 && *x <= 9_007_199_254_740_991.0)
                        .ok_or_else(|| {
                            ScriptError::new(
                                "script.bad_data",
                                "An event's cause is the sequence number of an earlier event.",
                                ErrorPhase::Run,
                            )
                        })?;
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    // checked above
                    let seq = seq as u64;
                    event = event.cause(EventSeq(seq));
                }
                other => {
                    return Err(ScriptError::new(
                        "script.unknown_key",
                        format!("emit has no option '{other}'."),
                        ErrorPhase::Run,
                    )
                    .suggest(pocket_contract::suggest::suggest_names(
                        other,
                        ["subject", "cause"],
                    ))
                    .into());
                }
            }
        }
    }
    with_call(ctx, "ctx.emit", |call, _world| {
        call.counts.events = call.counts.events.saturating_add(1);
        call.commands.push(Command::Emit(event));
        Ok(())
    })?;
    Ok(Value::new_undefined(ctx.clone()))
}

fn wanted(kinds: &[String], kind: &str) -> bool {
    kinds.iter().any(|k| {
        if k.ends_with('.') {
            kind.starts_with(k.as_str())
        } else {
            k == kind
        }
    })
}

fn event_to_js<'js>(ctx: &Ctx<'js>, e: &Event) -> rquickjs::Result<Value<'js>> {
    let o = Object::new(ctx.clone())?;
    #[allow(clippy::cast_precision_loss)] // sequence numbers and ticks stay below 2^53
    {
        o.set("seq", e.seq.0 as f64)?;
        o.set("tick", e.tick.0 as f64)?;
        o.set("kind", e.kind.as_str())?;
        o.set(
            "subject",
            e.subject.map_or_else(
                || Value::new_null(ctx.clone()),
                |s| Value::new_number(ctx.clone(), s.to_f64()),
            ),
        )?;
        o.set(
            "cause",
            e.cause.map_or_else(
                || Value::new_null(ctx.clone()),
                |c| Value::new_number(ctx.clone(), c.0 as f64),
            ),
        )?;
    }
    o.set("data", plain_to_js(ctx, &e.data)?)?;
    freeze_new(ctx, o)
}

/// `ctx.events`: the inbox's events of the declared kinds (or prefixes ending in "."), in
/// sequence order.
pub fn events<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let k = arg(ctx, args, 0);
    let mut kinds = Vec::new();
    for i in 0..js::dense_array_len(ctx, &k, 4096).unwrap_or(0) {
        if let Own::Data(x) = js::own(ctx, &k, &i.to_string())
            && let Some(Ok(t)) = js::text(&x)
        {
            kinds.push(t);
        }
    }
    let list = if kinds.is_empty() {
        Vec::new()
    } else {
        with_call(ctx, "ctx.events", |_call, world| {
            Ok(world
                .resource::<EventInbox>()
                .events()
                .iter()
                .filter(|e| wanted(&kinds, e.kind.as_str()))
                .cloned()
                .collect::<Vec<_>>())
        })?
    };
    let a = Array::new(ctx.clone())?;
    for (i, e) in list.iter().enumerate() {
        a.set(i, event_to_js(ctx, e)?)?;
    }
    Ok(freeze_new(ctx, a.into_object())?)
}

/// `ctx.intent`: script intents arrive with the player interface (slice 2).
pub fn intent<'js>(_ctx: &Ctx<'js>, _args: &[Value<'js>]) -> NResult<'js> {
    Err(ScriptError::new(
        "script.restricted",
        "ctx.intent is not available yet: intents arrive with the player interface (slice 2).",
        ErrorPhase::Run,
    )
    .into())
}
