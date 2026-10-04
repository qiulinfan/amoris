//! The sandbox of every program's context (docs/spec/script-sandbox.md 2): the intrinsics it
//! offers, the globals it removes and replaces, `Math` over the engine's deterministic library,
//! the console, the stack mapper, override taming and the deep freeze of everything reachable from
//! `globalThis` and of the hidden intrinsics no property path from it reaches (`freeze.rs`), before
//! any project module evaluates.

use std::collections::BTreeSet;

use rquickjs::function::{Opt, Rest};
use rquickjs::{Coerced, Ctx, Function, Object, Value};

use pocket_sim::math;

use crate::error::{ErrorPhase, ScriptError};
use crate::freeze;
use crate::natives;

/// The lockdown, as JavaScript run once per context with the natives it installs. It runs before
/// any project module, so it sees only the intrinsics. It returns the hidden intrinsics as
/// `[name, value]` pairs: objects no own property or prototype chain from `globalThis` reaches,
/// which a script still meets through what a builtin returns (`Object.getPrototypeOf([].values())`),
/// and those whose globals it deletes; the deep freeze takes them as roots beside `globalThis`.
const LOCKDOWN: &str = r#"(function lockdown(n) {
    "use strict";
    const g = globalThis;
    const define = (o, k, value) =>
        Object.defineProperty(o, k, { value, writable: false, enumerable: false, configurable: false });
    const hidden = [];
    const sample = (name, f) => {
        let v;
        try { v = f(); } catch (e) { return; }
        if ((typeof v === "object" || typeof v === "function") && v !== null) hidden.push([name, v]);
    };
    const proto = Object.getPrototypeOf;
    const done = { next() { return { done: true, value: undefined }; } };
    sample("%ArrayIteratorPrototype%", () => proto([].values()));
    sample("%MapIteratorPrototype%", () => proto(new Map().values()));
    sample("%SetIteratorPrototype%", () => proto(new Set().values()));
    sample("%StringIteratorPrototype%", () => proto(""[Symbol.iterator]()));
    sample("%RegExpStringIteratorPrototype%", () => proto("a".matchAll(/a/g)));
    sample("%IteratorHelperPrototype%", () => proto([].values().map((x) => x)));
    sample("%WrapForValidIteratorPrototype%", () => proto(Iterator.from(done)));
    sample("%IteratorConcat%", () => proto(Iterator.concat()));
    sample("%IteratorZip%", () => proto(Iterator.zip([])));
    sample("%GeneratorFunction.prototype%", () => proto(function* () {}));
    sample("%GeneratorPrototype%", () => proto(function* () {}).prototype);
    sample("%AsyncFunction.prototype%", () => proto(async function () {}));
    sample("%AsyncGeneratorFunction.prototype%", () => proto(async function* () {}));
    sample("%AsyncGeneratorPrototype%", () => proto(async function* () {}).prototype);
    sample("%AsyncIteratorPrototype%", () => proto(proto(async function* () {}).prototype));
    sample("%Promise%", () => g.Promise);
    sample("%Promise.prototype%", () => g.Promise.prototype);
    sample("%Array.fromAsync%", () => Array.fromAsync);
    sample("%AsyncDisposableStack%", () => g.AsyncDisposableStack);
    sample("%ThrowTypeError%", () => Object.getOwnPropertyDescriptor(Function.prototype, "caller").get);
    // Promise and the builtins that make promises go: Array.fromAsync and AsyncDisposableStack
    // would hand a lint-clean script a promise, its constructor and a job queue that outlives a
    // call (script-sandbox.md 2.2).
    for (const k of ["eval", "Promise", "queueMicrotask", "SharedArrayBuffer", "Atomics", "Date",
                     "performance", "WeakRef", "FinalizationRegistry", "AsyncDisposableStack"]) {
        delete g[k];
    }
    delete Array.fromAsync;
    delete RegExp.prototype.compile;
    const protos = [
        [Object.getPrototypeOf(function () {}), n.refuse.Function],
        [Object.getPrototypeOf(function* () {}), n.refuse.GeneratorFunction],
        [Object.getPrototypeOf(async function () {}), n.refuse.AsyncFunction],
        [Object.getPrototypeOf(async function* () {}), n.refuse.AsyncGeneratorFunction],
    ];
    for (const [proto, refuse] of protos) {
        define(refuse, "prototype", proto);
        Object.defineProperty(proto, "constructor", { value: refuse, writable: true, enumerable: false, configurable: true });
    }
    Object.defineProperty(g, "Function", { value: n.refuse.Function, writable: true, enumerable: false, configurable: true });
    for (const k of Object.keys(n.math)) {
        Object.defineProperty(Math, k, { value: n.math[k], writable: true, enumerable: false, configurable: true });
    }
    Object.defineProperty(g, "console", { value: n.console, writable: true, enumerable: false, configurable: true });
    Error.prepareStackTrace = n.prepareStackTrace;
    Error.stackTraceLimit = 64;
    define(Error, "prepareStackTrace", n.prepareStackTrace);
    define(Error, "stackTraceLimit", 64);
    // Override taming (SES): a data property of a frozen prototype becomes an accessor whose
    // setter defines an own property on the receiver, so `this.name = ...` in an Error subclass
    // and `obj.toString = f` still work once the prototype is frozen.
    const tame = (o, k) => {
        const d = Object.getOwnPropertyDescriptor(o, k);
        if (!d || !("value" in d)) return;
        const value = d.value;
        const get = function () { return value; };
        const set = function (v) {
            if (this === o) throw new TypeError("Cannot assign to read only property '" + String(k) + "' of a frozen prototype");
            Object.defineProperty(this, k, { value: v, writable: true, enumerable: true, configurable: true });
        };
        Object.defineProperty(get, "name", { value: "tamed " + String(k) });
        Object.defineProperty(set, "name", { value: "tamed " + String(k) });
        Object.defineProperty(o, k, { get, set, enumerable: d.enumerable, configurable: false });
    };
    for (const k of ["constructor", "toString", "valueOf", "toLocaleString", "hasOwnProperty",
                     "isPrototypeOf", "propertyIsEnumerable"]) {
        tame(Object.prototype, k);
    }
    for (const k of ["constructor", "toString"]) {
        tame(Function.prototype, k);
        tame(Array.prototype, k);
    }
    for (const name of ["Error", "TypeError", "RangeError", "SyntaxError", "ReferenceError",
                        "EvalError", "URIError", "AggregateError", "InternalError"]) {
        const E = g[name];
        if (typeof E !== "function") continue;
        for (const k of ["constructor", "message", "name", "toString"]) tame(E.prototype, k);
    }
    return hidden;
})"#;

/// The walk of script-sandbox.md 2.3, step 5: every extensible object, writable or configurable
/// property and setter reachable from the roots the deep freeze started from (`globalThis` first,
/// then the hidden intrinsics, as `[name, value]` pairs), as paths. The lockdown test compares the
/// setters with the reviewed list and requires the rest to be empty.
pub const VERIFY: &str = r#"(function verify(roots) {
    const seen = new Set();
    const problems = [];
    const setters = [];
    // globalThis on top: what it reaches is named by its path from it.
    const stack = roots.map(([name, value]) => [value, name]).reverse();
    while (stack.length > 0) {
        const [o, path] = stack.pop();
        if ((typeof o !== "object" && typeof o !== "function") || o === null || seen.has(o)) continue;
        seen.add(o);
        if (Object.isExtensible(o)) problems.push("extensible " + path);
        stack.push([Object.getPrototypeOf(o), path + ".[[Prototype]]"]);
        for (const k of Reflect.ownKeys(o)) {
            const d = Object.getOwnPropertyDescriptor(o, k);
            const p = path + "." + String(k);
            if ("value" in d) {
                if (d.writable) problems.push("writable " + p);
                stack.push([d.value, p]);
            } else {
                if (d.set !== undefined) setters.push(p + " = " + d.set.name);
                stack.push([d.get, p + ".get"]);
                stack.push([d.set, p + ".set"]);
            }
            if (d.configurable) problems.push("configurable " + p);
        }
    }
    return JSON.stringify({ problems: problems.sort(), setters: setters.sort(), objects: seen.size,
                            roots: roots.map(([name]) => name) });
})"#;

fn refuse<'js>(ctx: Ctx<'js>, _args: Rest<Value<'js>>) -> rquickjs::Result<Value<'js>> {
    let e = ScriptError::new(
        "script.code_generation",
        "Code cannot be generated at run time: Function and eval are not available; write the \
         code in a module.",
        ErrorPhase::Run,
    );
    Err(crate::caught::throw(&ctx, &e))
}

macro_rules! unary {
    ($obj:ident, $ctx:ident, $($js:literal => $f:path),* $(,)?) => {
        $( $obj.set($js, Function::new($ctx.clone(), |x: Coerced<f64>| $f(x.0))?.with_name($js)?)?; )*
    };
}

/// `Math.pow` with ECMAScript's special cases over the engine's `pow` (numeric.md 6.3): a NaN
/// exponent gives NaN, and ±1 to an infinite power is NaN, where C's `pow` gives 1.
pub fn js_pow(x: f64, y: f64) -> f64 {
    if y.is_nan() || ((x == 1.0 || x == -1.0) && y.is_infinite()) {
        return f64::NAN;
    }
    math::pow(x, y)
}

/// `Math.hypot` over any number of arguments: an infinity gives +Infinity, else a NaN gives NaN;
/// two arguments go through the engine's `hypot`, others are scaled by the largest magnitude.
pub fn js_hypot(args: &[f64]) -> f64 {
    if args.iter().any(|x| x.is_infinite()) {
        return f64::INFINITY;
    }
    if args.iter().any(|x| x.is_nan()) {
        return f64::NAN;
    }
    match args {
        [] => 0.0,
        [x] => x.abs(),
        [x, y] => math::hypot(*x, *y),
        _ => {
            let m = args.iter().fold(0.0f64, |m, x| math::max(m, x.abs()));
            if m == 0.0 {
                return 0.0;
            }
            let mut sum = 0.0;
            for x in args {
                let r = x / m;
                sum += r * r;
            }
            m * math::sqrt(sum)
        }
    }
}

fn math_object<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<Object<'js>> {
    let m = Object::new(ctx.clone())?;
    unary!(m, ctx,
        "sin" => math::sin, "cos" => math::cos, "tan" => math::tan, "asin" => math::asin,
        "acos" => math::acos, "atan" => math::atan, "sinh" => math::sinh, "cosh" => math::cosh,
        "tanh" => math::tanh, "asinh" => math::asinh, "acosh" => math::acosh,
        "atanh" => math::atanh, "exp" => math::exp, "expm1" => math::expm1, "log" => math::ln,
        "log1p" => math::ln_1p, "log2" => math::log2, "log10" => math::log10, "cbrt" => math::cbrt,
    );
    m.set(
        "atan2",
        Function::new(ctx.clone(), |y: Coerced<f64>, x: Coerced<f64>| {
            math::atan2(y.0, x.0)
        })?
        .with_name("atan2")?,
    )?;
    m.set(
        "pow",
        Function::new(ctx.clone(), |x: Coerced<f64>, y: Coerced<f64>| {
            js_pow(x.0, y.0)
        })?
        .with_name("pow")?,
    )?;
    m.set(
        "hypot",
        Function::new(ctx.clone(), |v: Rest<Coerced<f64>>| {
            js_hypot(&v.0.iter().map(|c| c.0).collect::<Vec<_>>())
        })?
        .with_name("hypot")?,
    )?;
    m.set(
        "random",
        natives::writing(ctx, "random", natives::rng::math_random)?,
    )?;
    Ok(m)
}

fn console_object<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<Object<'js>> {
    let c = Object::new(ctx.clone())?;
    c.set("log", natives::function(ctx, "log", natives::console::log)?)?;
    c.set(
        "info",
        natives::function(ctx, "info", natives::console::info)?,
    )?;
    c.set(
        "warn",
        natives::function(ctx, "warn", natives::console::warn)?,
    )?;
    c.set(
        "error",
        natives::function(ctx, "error", natives::console::error)?,
    )?;
    c.set(
        "debug",
        natives::function(ctx, "debug", natives::console::debug)?,
    )?;
    Ok(c)
}

/// What a lockdown leaves: the objects it froze (the context's intrinsics), the class ids of
/// iteration-state objects, and the roots of the freeze as `[name, value]` pairs (`globalThis`
/// first), which the verification walk starts from.
pub struct Locked<'js> {
    pub intrinsics: BTreeSet<usize>,
    pub iterators: Vec<u32>,
    pub roots: rquickjs::Array<'js>,
}

/// Locks a fresh context down: removes and replaces globals, tames overrides, then deep-freezes
/// everything reachable from `globalThis` and from the hidden intrinsics.
pub fn lockdown<'js>(ctx: &Ctx<'js>) -> rquickjs::Result<Locked<'js>> {
    let n = Object::new(ctx.clone())?;
    let r = Object::new(ctx.clone())?;
    for name in [
        "Function",
        "GeneratorFunction",
        "AsyncFunction",
        "AsyncGeneratorFunction",
    ] {
        r.set(
            name,
            Function::new(ctx.clone(), refuse)?
                .with_name(name)?
                .with_constructor(true),
        )?;
    }
    n.set("refuse", r)?;
    n.set("math", math_object(ctx)?)?;
    n.set("console", console_object(ctx)?)?;
    n.set(
        "prepareStackTrace",
        Function::new(
            ctx.clone(),
            |ctx: Ctx<'js>, e: Value<'js>, s: Opt<Value<'js>>| {
                let sites = s.0.unwrap_or_else(|| Value::new_undefined(ctx.clone()));
                crate::caught::prepare_stack(ctx, e, sites)
            },
        )?
        .with_name("prepareStackTrace")?,
    )?;
    let f: Function = ctx.eval(LOCKDOWN)?;
    let hidden: rquickjs::Array = f.call((n,))?;
    let iterators = freeze::iterator_classes(ctx);
    let roots = rquickjs::Array::new(ctx.clone())?;
    let global = rquickjs::Array::new(ctx.clone())?;
    global.set(0, "globalThis")?;
    global.set(1, ctx.globals())?;
    roots.set(0, global)?;
    let mut values = vec![ctx.globals().into_value()];
    for (i, pair) in hidden.iter::<rquickjs::Array>().enumerate() {
        let pair = pair?;
        values.push(pair.get(1)?);
        roots.set(i + 1, pair)?;
    }
    let mut intrinsics = BTreeSet::new();
    freeze::deep_freeze(ctx, values, None, &BTreeSet::new(), Some(&mut intrinsics))
        .map_err(|what| rquickjs::Error::new_from_js_message("lockdown", "frozen", what))?;
    Ok(Locked {
        intrinsics,
        iterators,
        roots,
    })
}
