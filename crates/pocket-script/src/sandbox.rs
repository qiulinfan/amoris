//! The sandbox of every program's context (docs/spec/script-sandbox.md 2): the intrinsics it
//! offers, the globals it removes and replaces, `Math` over the engine's deterministic library,
//! the console, the stack mapper, override taming and the deep freeze of everything reachable from
//! `globalThis` and of the hidden intrinsics no property path from it reaches (`freeze.rs`), before
//! any project module evaluates.

use std::collections::BTreeSet;
use std::ffi::CString;

use rquickjs::function::{Opt, Rest};
use rquickjs::{Coerced, Ctx, Function, Object, Value, qjs};

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

/// A `Math` function QuickJS-ng calls with plain doubles, as it calls its own `Math.sin`.
#[derive(Clone, Copy)]
enum CMath {
    /// `JS_CFUNC_f_f`: one argument.
    Unary(extern "C" fn(f64) -> f64),
    /// `JS_CFUNC_f_f_f`: two.
    Binary(extern "C" fn(f64, f64) -> f64),
}

/// The engine's `Math` function `f` as a QuickJS-ng C function named `name` (docs/bench/script.md,
/// section 7): QuickJS-ng converts the arguments itself (`ToNumber`, so a `valueOf` runs as for any
/// builtin, and a missing argument is `undefined`, NaN) and the result (an integral one becomes an
/// integer value, as rquickjs's conversion did), with no closure trampoline in between. It also
/// pushes a native frame, so an error thrown inside one shows `at sin (native)` in `e.stack`. The
/// functions are `pocket_sim::math`'s, `libm`'s ports of musl over doubles; they run without
/// `catch_unwind`, and a panic in one would abort the process at the `extern "C"` edge rather than
/// unwind through C. The panicking paths found in `libm` are its debug-build checks on paths its
/// invariants rule out, which numeric.md 6's sweeps sample and do not prove: script-sandbox.md 4.3
/// records the exception.
fn c_function<'js>(ctx: &Ctx<'js>, name: &str, f: CMath) -> rquickjs::Result<Value<'js>> {
    let cname = CString::new(name).map_err(|_| rquickjs::Error::Exception)?;
    // SAFETY: QuickJS-ng keeps a C function in a union (`JSCFunctionType`) and calls it through the
    // member `cproto` names: `f_f` takes and returns a double, `f_f_f` takes two. Its own `Math`
    // functions are registered this way (`JS_CFUNC_SPECIAL_DEF`); the pointer is never called
    // through `generic`.
    let (func, length, cproto) = unsafe {
        match f {
            CMath::Unary(f) => (
                std::mem::transmute::<extern "C" fn(f64) -> f64, qjs::JSCFunction>(f),
                1,
                qjs::JSCFunctionEnum_JS_CFUNC_f_f,
            ),
            CMath::Binary(f) => (
                std::mem::transmute::<extern "C" fn(f64, f64) -> f64, qjs::JSCFunction>(f),
                2,
                qjs::JSCFunctionEnum_JS_CFUNC_f_f_f,
            ),
        }
    };
    // SAFETY: a live context; QuickJS-ng copies the name into an atom.
    let raw = unsafe {
        qjs::JS_NewCFunction2(crate::js::raw(ctx), func, cname.as_ptr(), length, cproto, 0)
    };
    // SAFETY: `raw` is a new reference the value takes over.
    let v = unsafe { Value::from_raw(ctx.clone(), raw) };
    if v.is_exception() {
        return Err(rquickjs::Error::Exception);
    }
    Ok(v)
}

macro_rules! c_math {
    ($obj:ident, $ctx:ident, $timed:ident, $($js:literal => |$($x:ident),+| $body:expr),* $(,)?) => {
        $({
            extern "C" fn f($($x: f64),+) -> f64 {
                $body
            }
            // The copy a host that times natives installs (natives::timing).
            extern "C" fn timed_f($($x: f64),+) -> f64 {
                natives::timing::timed(crate::host::NativeKind::Math, || f($($x),+))
            }
            let kind = if $timed {
                c_math!(@kind timed_f, $($x),+)
            } else {
                c_math!(@kind f, $($x),+)
            };
            $obj.set($js, c_function($ctx, $js, kind)?)?;
        })*
    };
    (@kind $f:ident, $x:ident) => { CMath::Unary($f) };
    (@kind $f:ident, $x:ident, $y:ident) => { CMath::Binary($f) };
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
    let timed = crate::host::shared(ctx).times_natives();
    c_math!(m, ctx, timed,
        "sin" => |x| math::sin(x), "cos" => |x| math::cos(x), "tan" => |x| math::tan(x),
        "asin" => |x| math::asin(x), "acos" => |x| math::acos(x), "atan" => |x| math::atan(x),
        "sinh" => |x| math::sinh(x), "cosh" => |x| math::cosh(x), "tanh" => |x| math::tanh(x),
        "asinh" => |x| math::asinh(x), "acosh" => |x| math::acosh(x),
        "atanh" => |x| math::atanh(x), "exp" => |x| math::exp(x), "expm1" => |x| math::expm1(x),
        "log" => |x| math::ln(x), "log1p" => |x| math::ln_1p(x), "log2" => |x| math::log2(x),
        "log10" => |x| math::log10(x), "cbrt" => |x| math::cbrt(x),
        "atan2" => |y, x| math::atan2(y, x), "pow" => |x, y| js_pow(x, y),
    );
    let hypot = |v: Rest<Coerced<f64>>| js_hypot(&v.0.iter().map(|c| c.0).collect::<Vec<_>>());
    let hypot = if timed {
        Function::new(ctx.clone(), move |v: Rest<Coerced<f64>>| {
            natives::timing::timed(crate::host::NativeKind::Math, || hypot(v))
        })?
    } else {
        Function::new(ctx.clone(), hypot)?
    };
    m.set("hypot", hypot.with_name("hypot")?)?;
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
