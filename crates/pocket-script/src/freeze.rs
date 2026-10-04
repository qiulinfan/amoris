//! The deep freeze behind the lockdown and the harden epilogue (docs/spec/script-sandbox.md 2.3
//! and 2.4): every object reachable through own property values (string and symbol keys),
//! accessor functions and prototypes is frozen. The lockdown records what it froze, the context's
//! intrinsics, so a module's harden walks only the module's own values; harden refuses state that
//! freezing cannot fix.

use std::collections::BTreeSet;

use rquickjs::{Ctx, Value, qjs};

use crate::js;

/// An object's identity while it is alive.
pub fn addr(v: &Value<'_>) -> usize {
    unsafe { qjs::JS_VALUE_GET_PTR(v.as_raw()) as usize }
}

/// Samples of the classes whose instances keep iteration state: generators and the built-in
/// iterators. Their class ids are runtime-wide.
const SAMPLES: &str = r#"(function () {
    const out = [(function* () {})(), [].values(), new Map().values(), new Set().values(),
                 ""[Symbol.iterator](), "a".matchAll(/a/g)];
    try { out.push([].values().map((x) => x)); } catch (e) {}
    try { out.push(Iterator.from({ next() { return { done: true, value: undefined }; } })); } catch (e) {}
    try { out.push(Iterator.concat()); } catch (e) {}
    return out;
})()"#;

/// QuickJS-ng's class id of ordinary objects (`JS_CLASS_OBJECT`, first in its class enum), which
/// an iterator a builtin implements in JavaScript (`Iterator.zip`) has: never an iteration class.
const CLASS_OBJECT: u32 = 1;

/// The class ids of iteration-state objects, read from fresh samples.
pub fn iterator_classes(ctx: &Ctx<'_>) -> Vec<u32> {
    let mut out = Vec::new();
    if let Ok(list) = ctx.eval::<rquickjs::Array, _>(SAMPLES) {
        for v in list.iter::<Value>().flatten() {
            let id = unsafe { qjs::JS_GetClassID(v.as_raw()) };
            if id != CLASS_OBJECT && !out.contains(&id) {
                out.push(id);
            }
        }
    }
    out
}

/// What a value is when freezing cannot make it stateless.
pub fn stateful(v: &Value<'_>, iterators: &[u32]) -> Option<&'static str> {
    let raw = v.as_raw();
    unsafe {
        if qjs::JS_IsProxy(raw) {
            return Some("a Proxy");
        }
        if qjs::JS_IsMap(raw) {
            return Some("a Map");
        }
        if qjs::JS_IsSet(raw) {
            return Some("a Set");
        }
        if qjs::JS_IsWeakMap(raw) {
            return Some("a WeakMap");
        }
        if qjs::JS_IsWeakSet(raw) {
            return Some("a WeakSet");
        }
        if qjs::JS_IsArrayBuffer(raw)
            || qjs::JS_IsDataView(raw)
            || qjs::JS_GetTypedArrayType(raw) >= 0
        {
            return Some("a buffer or a typed array");
        }
        if qjs::JS_IsPromise(raw) || qjs::JS_IsWeakRef(raw) {
            return Some("a promise or a weak reference");
        }
        if iterators.contains(&qjs::JS_GetClassID(raw)) {
            return Some("an iterator or a generator");
        }
        if qjs::JS_IsRegExp(raw) {
            let flags: String = v
                .as_object()
                .and_then(|o| o.get::<_, String>("flags").ok())
                .unwrap_or_default();
            if flags.contains('g') || flags.contains('y') {
                return Some("a RegExp with the g or y flag (its lastIndex changes)");
            }
        }
    }
    None
}

/// Deep-freezes `roots`, skipping objects in `skip` (already deep-frozen) and adding what it
/// froze to `record`. With `refuse`, the first value freezing cannot fix stops the walk.
pub fn deep_freeze<'js>(
    ctx: &Ctx<'js>,
    roots: Vec<Value<'js>>,
    refuse: Option<&[u32]>,
    skip: &BTreeSet<usize>,
    mut record: Option<&mut BTreeSet<usize>>,
) -> Result<usize, &'static str> {
    let mut seen = BTreeSet::new();
    let mut stack = roots;
    while let Some(v) = stack.pop() {
        if !v.is_object() {
            continue;
        }
        let a = addr(&v);
        if skip.contains(&a) || !seen.insert(a) {
            continue;
        }
        if js::is_proxy(&v) {
            return Err("a Proxy");
        }
        if let Some(iterators) = refuse
            && let Some(what) = stateful(&v, iterators)
        {
            return Err(what);
        }
        if unsafe { qjs::JS_FreezeObject(js::raw(ctx), v.as_raw()) } < 0 {
            let _ = ctx.catch();
            return Err("a value that cannot be frozen");
        }
        if let Some(r) = record.as_deref_mut() {
            r.insert(a);
        }
        if let Some(p) = js::proto(&v) {
            stack.push(p);
        }
        stack.extend(js::own_reachable(ctx, &v));
    }
    Ok(seen.len())
}
