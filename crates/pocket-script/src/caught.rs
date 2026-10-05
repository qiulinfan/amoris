//! Exceptions to structured errors (docs/spec/script-sandbox.md 5): the JavaScript error's name and
//! message, its TypeScript stack through the source maps, the host's own code and detail when a
//! native refused (known by the error's identity, never by properties a script could copy), the
//! entity `QueryResult.each` attached, and the faults and limits QuickJS-ng signals with its own
//! uncatchable errors.

use rquickjs::function::This;
use rquickjs::{Ctx, Function, Object, Value};

use crate::error::{
    ErrorPhase, JsError, ScriptError, SourceLocation, StackFrame, hint_for, to_u32,
};
use crate::host::{HostThrow, Shared, out_of_memory, shared};
use crate::js::{self, Own};

/// An own or inherited data property, read without running project code: a getter is read only
/// on an intrinsic (the lockdown's tamed getters return the original value), never on a project
/// object.
fn lookup<'js>(ctx: &Ctx<'js>, sh: &Shared, v: &Value<'js>, key: &str) -> Option<Value<'js>> {
    let intrinsics = sh
        .intrinsics
        .borrow()
        .get(&(js::raw(ctx) as usize))
        .cloned();
    let mut cur = v.clone();
    for _ in 0..32 {
        match js::own(ctx, &cur, key) {
            Own::Data(x) => return Some(x),
            Own::Accessor => {
                if intrinsics
                    .as_ref()
                    .is_some_and(|s| s.contains(&crate::freeze::addr(&cur)))
                {
                    // [[Get]] on the value itself reaches this getter with the value as receiver
                    // (Error.prototype.stack reads the error's own slot).
                    return v.as_object()?.get::<_, Value>(key).ok();
                }
                return None;
            }
            Own::Missing => cur = js::proto(&cur)?,
        }
    }
    None
}

fn lookup_str<'js>(ctx: &Ctx<'js>, sh: &Shared, v: &Value<'js>, key: &str) -> Option<String> {
    lookup(ctx, sh, v, key)
        .and_then(|x| js::text(&x))
        .and_then(Result::ok)
}

/// Parses mapped stack text (`    at fn (file:line:col)`), innermost first.
pub fn frames(stack: &str) -> Vec<StackFrame> {
    let mut out = Vec::new();
    for raw in stack.lines() {
        let Some(rest) = raw.trim().strip_prefix("at ") else {
            continue;
        };
        let (function, location) = match (rest.rfind(" ("), rest.ends_with(')')) {
            (Some(open), true) => (&rest[..open], &rest[open + 2..rest.len() - 1]),
            _ => ("", rest),
        };
        let mut parts = location.rsplitn(3, ':');
        let (Some(col), Some(line), Some(file)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(line), Ok(column)) = (line.parse::<u32>(), col.parse::<u32>()) else {
            continue;
        };
        out.push(StackFrame {
            at: SourceLocation {
                file: file.to_owned(),
                line,
                column,
            },
            function: (!function.is_empty() && function != "<anonymous>")
                .then(|| function.to_owned()),
        });
    }
    out
}

/// The structured error of a thrown value.
pub fn error_from_value<'js>(
    ctx: &Ctx<'js>,
    sh: &Shared,
    v: &Value<'js>,
    phase: ErrorPhase,
) -> ScriptError {
    if let Some(message) = sh.panic.borrow().clone() {
        return ScriptError::new(
            "sim.internal",
            format!("A native function of the script host panicked: {message}."),
            phase,
        );
    }
    let name = lookup_str(ctx, sh, v, "name").unwrap_or_default();
    let message = if v.is_object() {
        lookup_str(ctx, sh, v, "message").unwrap_or_default()
    } else {
        primitive_text(v)
    };
    let stack = lookup_str(ctx, sh, v, "stack").unwrap_or_default();
    let mut frames = frames(&stack);
    frames.retain(|f| f.at.file != "native");
    let location = frames
        .iter()
        .find(|f| f.at.file != crate::resolve::PRELUDE)
        .map(|f| f.at.clone());
    let uncatchable = js::is_uncatchable(v);
    // Memory first: P2 also stops the call through the interrupt handler, and a builtin may have
    // replaced the out-of-memory error with one of its own.
    let (code, msg) = if out_of_memory(sh)
        || (uncatchable && name == "InternalError" && message == "out of memory")
    {
        (
            "script.out_of_memory",
            "The script ran out of memory.".to_owned(),
        )
    } else if sh.exceeded.get() {
        (
            "script.budget_exceeded",
            "The script ran out of its step budget.".to_owned(),
        )
    } else if uncatchable && name == "InternalError" && message == "call depth limit exceeded" {
        (
            "script.call_depth",
            format!(
                "The script nested calls past the limit of {}.",
                sh.limits.max_call_depth
            ),
        )
    } else if uncatchable && name == "RangeError" && message.starts_with("Maximum call stack size")
    {
        (
            "script.stack_overflow",
            "The script overflowed the stack.".to_owned(),
        )
    } else {
        ("", String::new())
    };
    let mut e = if !code.is_empty() {
        ScriptError::new(code, msg, phase)
    } else if let Some(host) = thrown_by_host(ctx, sh, v) {
        let mut e = host;
        e.detail.phase.get_or_insert(phase);
        e
    } else {
        let what = if message.is_empty() {
            name.clone()
        } else {
            message.clone()
        };
        let mut e = ScriptError::new(
            if phase == ErrorPhase::Load {
                "script.load_exception"
            } else {
                "script.exception"
            },
            what,
            phase,
        );
        if !name.is_empty() || v.is_object() {
            e.detail.js_error = Some(JsError {
                name: name.clone(),
                message: message.clone(),
            });
        }
        if e.detail.hint.is_none() {
            e.detail.hint = hint_for(&name, &message);
        }
        e
    };
    if e.detail.entity.is_none()
        && let Some(id) = lookup(ctx, sh, v, "entity").and_then(|x| x.as_number())
    {
        e.detail.entity = Some(id);
    }
    if e.detail.location.is_none() {
        e.detail.location = location;
    }
    e.detail.stack = frames;
    e
}

/// The host's error when `v` is an error a native threw in this call (a script rethrowing it
/// keeps its code; a copy of its properties does not).
fn thrown_by_host<'js>(ctx: &Ctx<'js>, sh: &Shared, v: &Value<'js>) -> Option<ScriptError> {
    if !v.is_object() {
        return None;
    }
    sh.thrown.borrow().iter().find_map(|t| {
        let known = t.value.clone().restore(ctx).ok()?;
        js::same(&known, v).then(|| t.error.clone())
    })
}

fn primitive_text(v: &Value<'_>) -> String {
    if let Some(s) = js::text(v) {
        return s.unwrap_or_else(|()| "a string".into());
    }
    if let Some(n) = v.as_number() {
        return crate::error::num(n).to_string();
    }
    if let Some(b) = v.as_bool() {
        return b.to_string();
    }
    if v.is_null() {
        return "null".into();
    }
    if v.is_undefined() {
        return "undefined".into();
    }
    "a thrown value".into()
}

/// Throws a host refusal as a JavaScript Error with the refusal's message, so the call fails at
/// the calling line; the host remembers the error for the call and maps it back to its code and
/// detail by identity.
pub fn throw<'js>(ctx: &Ctx<'js>, e: &ScriptError) -> rquickjs::Error {
    let made = (|| -> rquickjs::Result<Value<'js>> {
        let ctor: Function = ctx.globals().get("Error")?;
        let error: Object = ctor.call((e.message.clone(),))?;
        Ok(error.into_value())
    })();
    match made {
        Ok(v) => {
            shared(ctx).thrown.borrow_mut().push(HostThrow {
                value: rquickjs::Persistent::save(ctx, v.clone()),
                error: e.clone(),
            });
            ctx.throw(v)
        }
        Err(err) => err,
    }
}

/// `Error.prepareStackTrace`: the stack with every position in TypeScript, prelude frames under
/// `pocket`, natives as `native`.
pub fn prepare_stack<'js>(
    ctx: Ctx<'js>,
    error: Value<'js>,
    sites: Value<'js>,
) -> rquickjs::Result<String> {
    let sh = shared(&ctx);
    let maps = sh.maps.borrow().clone();
    let mut out = String::new();
    if let Some(name) = lookup_str(&ctx, &sh, &error, "name") {
        out.push_str(&name);
        if let Some(m) = lookup_str(&ctx, &sh, &error, "message").filter(|m| !m.is_empty()) {
            out.push_str(": ");
            out.push_str(&m);
        }
        out.push('\n');
    }
    let Some(arr) = sites.as_array() else {
        return Ok(out);
    };
    for i in 0..arr.len() {
        let site: Value = arr.get(i)?;
        let call = |method: &str| -> rquickjs::Result<Value<'js>> {
            let f: Function = site
                .as_object()
                .map_or(Ok(None), |o| o.get(method))?
                .ok_or(rquickjs::Error::Unknown)?;
            f.call((This(site.clone()),))
        };
        let file = call("getFileName")
            .ok()
            .and_then(|v| js::text(&v))
            .and_then(Result::ok);
        let line = call("getLineNumber").ok().and_then(|v| v.as_number());
        let column = call("getColumnNumber").ok().and_then(|v| v.as_number());
        let function = call("getFunctionName")
            .ok()
            .and_then(|v| js::text(&v))
            .and_then(Result::ok);
        let function = function
            .filter(|f| !f.is_empty())
            .unwrap_or_else(|| "<anonymous>".into());
        match (file, line, column) {
            (Some(file), Some(line), Some(column)) if line >= 1.0 => {
                let (l, c) = (to_u32(line as usize), to_u32(column as usize));
                let (l, c) = maps.as_ref().map_or((l, c), |m| m.map(&file, l, c));
                out.push_str(&format!("    at {function} ({file}:{l}:{c})\n"));
            }
            _ => out.push_str(&format!("    at {function} (native)\n")),
        }
    }
    Ok(out)
}
