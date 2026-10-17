//! The harden epilogue's native (docs/spec/script-sandbox.md 2.4): after a module evaluates, every
//! value it holds at its top level is deep-frozen, and state freezing cannot fix is refused with
//! `script.module_state`. `freeze` is the same walk for a script's own values.

use std::collections::BTreeSet;
use std::rc::Rc;

use rquickjs::{Ctx, Value};
use serde_json::json;

use super::{NResult, Thrown, arg};
use crate::error::{ErrorPhase, ScriptError};
use crate::freeze::deep_freeze;
use crate::host::shared;
use crate::js::{self, Own};

/// The context's intrinsics (frozen by the lockdown) and the iteration-state classes.
fn known(ctx: &Ctx<'_>) -> (Rc<BTreeSet<usize>>, Vec<u32>) {
    let sh = shared(ctx);
    let key = js::raw(ctx) as usize;
    let skip = sh
        .intrinsics
        .borrow()
        .get(&key)
        .cloned()
        .unwrap_or_default();
    let iterators = sh.iterators.borrow().clone();
    (skip, iterators)
}

/// `harden(file, namespace, bindings, lines)`: the epilogue every module ends with.
pub fn harden<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let file = js::text(&arg(ctx, args, 0))
        .and_then(Result::ok)
        .unwrap_or_default();
    let (ns, bindings, lines) = (arg(ctx, args, 1), arg(ctx, args, 2), arg(ctx, args, 3));
    let mut named: Vec<(String, Value<'js>)> = Vec::new();
    for key in js::own_keys(ctx, &bindings, true) {
        if let Own::Data(v) = js::own(ctx, &bindings, &key) {
            named.push((key, v));
        }
    }
    for key in js::own_keys(ctx, &ns, false) {
        if let Own::Data(v) = js::own(ctx, &ns, &key) {
            named.push((
                if key == "default" {
                    "export default".into()
                } else {
                    key
                },
                v,
            ));
        }
    }
    let (skip, iterators) = known(ctx);
    for (name, value) in named {
        if let Err(what) = deep_freeze(ctx, vec![value], Some(&iterators), &skip, None) {
            let line = match js::own(ctx, &lines, &name) {
                Own::Data(l) => l.as_number().unwrap_or(1.0),
                _ => 1.0,
            };
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // a line number
            let line = line.clamp(1.0, 4_294_967_295.0) as u32;
            let e = ScriptError::new(
                "script.module_state",
                format!("{file}: '{name}' holds {what}, which cannot be frozen; keep the data in a component"),
                ErrorPhase::Load,
            )
            .at(&file, line, 1)
            .with("binding", json!(name));
            return Err(Thrown::Script(e));
        }
    }
    Ok(Value::new_undefined(ctx.clone()))
}

/// `freeze(value)`: deep-freezes a value and returns it.
pub fn freeze<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let v = arg(ctx, args, 0);
    let (skip, _) = known(ctx);
    deep_freeze(ctx, vec![v.clone()], None, &skip, None).map_err(|what| {
        Thrown::Script(ScriptError::new(
            "script.bad_data",
            format!("freeze cannot freeze {what}."),
            ErrorPhase::Run,
        ))
    })?;
    Ok(v)
}
