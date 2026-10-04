//! `console` (docs/spec/script-host.md 5.7): lines go to the session log only, formatted by a walk
//! in the host that never invokes a getter, `toJSON` or `toString`, so neither the log limit nor a
//! build without a session log changes a result. The walk is bounded: it stops after
//! `MAX_VALUES` values or `MAX_BYTES` bytes of text, checked as it goes, since it runs in Rust where
//! the step budget cannot stop it.

use rquickjs::{Ctx, Value, qjs};

use super::NResult;
use crate::host::{LogLine, shared};
use crate::js::{self, Own};

/// Values one console call formats before it stops.
pub const MAX_VALUES: u32 = 1000;
/// Bytes of text one console line holds before it stops.
pub const MAX_BYTES: usize = 4096;
/// What a line that stopped ends with.
pub const ELLIPSIS: &str = "\u{2026}";

/// One line being formatted, with what is left of its bounds.
struct Line {
    out: String,
    values: u32,
    full: bool,
}

impl Line {
    fn new() -> Line {
        Line {
            out: String::new(),
            values: 0,
            full: false,
        }
    }

    /// Appends text; false once the line is full.
    fn push(&mut self, s: &str) -> bool {
        if self.full {
            return false;
        }
        if self.out.len() + s.len() > MAX_BYTES {
            let mut cut = MAX_BYTES - self.out.len();
            while !s.is_char_boundary(cut) {
                cut -= 1;
            }
            self.out.push_str(&s[..cut]);
            self.full = true;
            return false;
        }
        self.out.push_str(s);
        true
    }

    /// Counts one more value; false once there were too many.
    fn visit(&mut self) -> bool {
        if self.full {
            return false;
        }
        self.values += 1;
        if self.values > MAX_VALUES {
            self.full = true;
        }
        !self.full
    }

    fn finish(mut self) -> String {
        if self.full {
            self.out.push_str(ELLIPSIS);
        }
        self.out
    }
}

/// Formats a console call's arguments as one line, running no project code, within the bounds.
pub fn format<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> String {
    let mut line = Line::new();
    for (i, a) in args.iter().enumerate() {
        if line.full || (i > 0 && !line.push(" ")) {
            break;
        }
        walk(ctx, a, 0, false, &mut line);
    }
    line.finish()
}

/// The text of a number or a BigInt through ECMAScript's `Number::toString` (no project code runs).
fn number_text<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> String {
    let s = unsafe { Value::from_raw(ctx.clone(), qjs::JS_ToString(js::raw(ctx), v.as_raw())) };
    let text = js::text(&s).and_then(Result::ok).unwrap_or_default();
    if v.is_big_int() {
        format!("{text}n")
    } else {
        text
    }
}

fn walk<'js>(ctx: &Ctx<'js>, v: &Value<'js>, depth: u32, nested: bool, line: &mut Line) {
    if !line.visit() {
        return;
    }
    if v.is_undefined() {
        line.push("undefined");
        return;
    }
    if v.is_null() {
        line.push("null");
        return;
    }
    if let Some(b) = v.as_bool() {
        line.push(if b { "true" } else { "false" });
        return;
    }
    if let Some(t) = js::text(v) {
        let t = t.unwrap_or_else(|()| "\u{FFFD}".into());
        if nested {
            line.push(&format!("{t:?}"));
        } else {
            line.push(&t);
        }
        return;
    }
    if v.is_number() || v.is_big_int() {
        line.push(&number_text(ctx, v));
        return;
    }
    let simple = if v.is_symbol() {
        Some("[symbol]")
    } else if js::is_proxy(v) {
        Some("[proxy]")
    } else if v.is_function() {
        Some("[function]")
    } else if depth >= 4 {
        Some(if v.is_array() { "[array]" } else { "[object]" })
    } else {
        None
    };
    if let Some(s) = simple {
        line.push(s);
        return;
    }
    let item = |key: &str, line: &mut Line| match js::own(ctx, v, key) {
        Own::Data(x) => walk(ctx, &x, depth + 1, true, line),
        Own::Accessor => {
            if line.visit() {
                line.push("[getter]");
            }
        }
        Own::Missing => {
            if line.visit() {
                line.push("undefined");
            }
        }
    };
    if v.is_array() {
        // By index up to `length`, so a huge array is never listed whole.
        let len = match js::own(ctx, v, "length") {
            Own::Data(x) => x.as_number().unwrap_or(0.0),
            _ => 0.0,
        };
        line.push("[");
        let mut i = 0u32;
        while f64::from(i) < len && !line.full {
            if i > 0 && !line.push(", ") {
                break;
            }
            item(&i.to_string(), line);
            i += 1;
        }
        line.push("]");
        return;
    }
    let keys = js::own_keys(ctx, v, true);
    if keys.is_empty() {
        line.push("{}");
        return;
    }
    line.push("{ ");
    for (i, k) in keys.iter().enumerate() {
        if line.full || (i > 0 && !line.push(", ")) {
            break;
        }
        if !line.push(k) || !line.push(": ") {
            break;
        }
        item(k, line);
    }
    line.push(" }");
}

fn write<'js>(ctx: &Ctx<'js>, level: &str, args: &[Value<'js>]) -> NResult<'js> {
    let sh = shared(ctx);
    let (tick, system) = sh
        .call
        .borrow()
        .as_ref()
        .map_or((0, None), |c| (c.tick.0, Some(c.name.clone())));
    if sh.log_tick.get() != tick {
        sh.log_tick.set(tick);
        sh.log_count.set(0);
    }
    let n = sh.log_count.get();
    sh.log_count.set(n.saturating_add(1));
    if n < sh.limits.log_lines_per_tick {
        // What the walk allocates in QuickJS-ng (key lists, number strings) is transient and
        // bounded by the walk; it must not meet the memory limit, or a line written could fault a
        // call that a line counted would not (script-host.md 5.7).
        let rt = unsafe { qjs::JS_GetRuntime(js::raw(ctx)) };
        unsafe { qjs::JS_SetMemoryLimit(rt, 0) };
        let text = format(ctx, args);
        unsafe { qjs::JS_SetMemoryLimit(rt, sh.limits.memory_bytes as _) };
        sh.log.borrow_mut().push(LogLine {
            tick,
            system,
            level: level.to_owned(),
            location: None,
            text,
        });
    }
    Ok(Value::new_undefined(ctx.clone()))
}

pub fn log<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    write(ctx, "log", args)
}

pub fn info<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    write(ctx, "info", args)
}

pub fn warn<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    write(ctx, "warn", args)
}

pub fn error<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    write(ctx, "error", args)
}

pub fn debug<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    write(ctx, "debug", args)
}
