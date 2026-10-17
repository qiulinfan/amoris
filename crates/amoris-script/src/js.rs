//! Reading JavaScript values without running JavaScript: own properties through their descriptors
//! (a getter is reported, never called), own keys in ECMAScript's own-key order, and the checks
//! that keep a Proxy's traps from running (script-host.md 5.8; script-sandbox.md 4.3: a native
//! reads its arguments before it borrows the world).

use rquickjs::{Ctx, Value, qjs};

/// The raw context of a `Ctx`.
pub fn raw(ctx: &Ctx<'_>) -> *mut qjs::JSContext {
    ctx.as_raw().as_ptr()
}

/// What an own property holds.
pub enum Own<'js> {
    Missing,
    Data(Value<'js>),
    /// A getter or setter: never invoked.
    Accessor,
}

/// Whether a value is a Proxy, whose traps would run project code.
pub fn is_proxy(v: &Value<'_>) -> bool {
    v.is_object() && unsafe { qjs::JS_IsProxy(v.as_raw()) }
}

struct AtomGuard(*mut qjs::JSContext, qjs::JSAtom);

impl Drop for AtomGuard {
    fn drop(&mut self) {
        unsafe { qjs::JS_FreeAtom(self.0, self.1) };
    }
}

fn atom(ctx: &Ctx<'_>, key: &str) -> AtomGuard {
    let c = raw(ctx);
    let a = unsafe { qjs::JS_NewAtomLen(c, key.as_ptr().cast(), key.len() as _) };
    AtomGuard(c, a)
}

/// The own property `key` of `obj` (an object that is not a Proxy), read from its descriptor.
pub fn own<'js>(ctx: &Ctx<'js>, obj: &Value<'js>, key: &str) -> Own<'js> {
    if !obj.is_object() || is_proxy(obj) {
        return Own::Missing;
    }
    let a = atom(ctx, key);
    let mut desc = qjs::JSPropertyDescriptor {
        flags: 0,
        value: qjs::JS_UNDEFINED,
        getter: qjs::JS_UNDEFINED,
        setter: qjs::JS_UNDEFINED,
    };
    let found = unsafe { qjs::JS_GetOwnProperty(raw(ctx), &mut desc, obj.as_raw(), a.1) };
    if found <= 0 {
        if found < 0 {
            let _ = ctx.catch();
        }
        return Own::Missing;
    }
    // Take ownership of the three values so they are freed.
    let value = unsafe { Value::from_raw(ctx.clone(), desc.value) };
    let getter = unsafe { Value::from_raw(ctx.clone(), desc.getter) };
    let setter = unsafe { Value::from_raw(ctx.clone(), desc.setter) };
    let flags = u32::try_from(desc.flags).unwrap_or(0);
    if flags & qjs::JS_PROP_GETSET != 0 || !getter.is_undefined() || !setter.is_undefined() {
        Own::Accessor
    } else {
        Own::Data(value)
    }
}

/// The own string keys of `obj` (not a Proxy) in own-key order: integer-like keys ascending, then
/// the others in insertion order; enumerable ones only when asked.
pub fn own_keys(ctx: &Ctx<'_>, obj: &Value<'_>, enumerable_only: bool) -> Vec<String> {
    if !obj.is_object() || is_proxy(obj) {
        return Vec::new();
    }
    let c = raw(ctx);
    let mut tab: *mut qjs::JSPropertyEnum = std::ptr::null_mut();
    let mut len: u32 = 0;
    let mut flags = qjs::JS_GPN_STRING_MASK;
    if enumerable_only {
        flags |= qjs::JS_GPN_ENUM_ONLY;
    }
    let r = unsafe { qjs::JS_GetOwnPropertyNames(c, &mut tab, &mut len, obj.as_raw(), flags as _) };
    if r < 0 {
        let _ = ctx.catch();
        return Vec::new();
    }
    let mut out = Vec::with_capacity(len as usize);
    for i in 0..len as usize {
        let a = unsafe { (*tab.add(i)).atom };
        let s = unsafe { Value::from_raw(ctx.clone(), qjs::JS_AtomToString(c, a)) };
        if let Some(text) = s.as_string().and_then(|s| s.to_string().ok()) {
            out.push(text);
        }
    }
    unsafe { qjs::JS_FreePropertyEnum(c, tab, len) };
    out
}

/// How many own string keys `obj` (not a Proxy) has, enumerable ones only when asked, without
/// making a string of any.
pub fn own_key_count(ctx: &Ctx<'_>, obj: &Value<'_>, enumerable_only: bool) -> usize {
    if !obj.is_object() || is_proxy(obj) {
        return 0;
    }
    let c = raw(ctx);
    let mut tab: *mut qjs::JSPropertyEnum = std::ptr::null_mut();
    let mut len: u32 = 0;
    let mut flags = qjs::JS_GPN_STRING_MASK;
    if enumerable_only {
        flags |= qjs::JS_GPN_ENUM_ONLY;
    }
    let r = unsafe { qjs::JS_GetOwnPropertyNames(c, &mut tab, &mut len, obj.as_raw(), flags as _) };
    if r < 0 {
        let _ = ctx.catch();
        return 0;
    }
    unsafe { qjs::JS_FreePropertyEnum(c, tab, len) };
    len as usize
}

/// Why a value is not the dense array a native takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NotDense {
    /// Not an Array, a Proxy, or an array with holes or extra own string keys.
    Shape,
    /// A dense-looking array whose `length` is past the caller's bound: refused before its keys
    /// are listed, so a huge array costs the host nothing (script-sandbox.md 4.3).
    TooLong(f64),
}

/// The length of a dense array of at most `max` elements: an Array whose own string keys are
/// exactly its indices and `length`. The `length` is read and bounded first; the keys are then
/// compared as atoms, with no string made per key.
pub fn dense_array_len<'js>(ctx: &Ctx<'js>, v: &Value<'js>, max: usize) -> Result<usize, NotDense> {
    if !v.is_array() || is_proxy(v) {
        return Err(NotDense::Shape);
    }
    let len = match own(ctx, v, "length") {
        Own::Data(x) => x.as_number().ok_or(NotDense::Shape)?,
        _ => return Err(NotDense::Shape),
    };
    if !(len >= 0.0 && len.fract() == 0.0) {
        return Err(NotDense::Shape);
    }
    if len > f64::from(u32::try_from(max).unwrap_or(u32::MAX)) {
        return Err(NotDense::TooLong(len));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked above
    let len = len as usize;
    let c = raw(ctx);
    let mut tab: *mut qjs::JSPropertyEnum = std::ptr::null_mut();
    let mut count: u32 = 0;
    let flags = qjs::JS_GPN_STRING_MASK;
    let r = unsafe { qjs::JS_GetOwnPropertyNames(c, &mut tab, &mut count, v.as_raw(), flags as _) };
    if r < 0 {
        let _ = ctx.catch();
        return Err(NotDense::Shape);
    }
    let length = atom(ctx, "length");
    let dense = count as usize == len + 1
        && (0..len).all(|i| {
            let index = AtomGuard(c, unsafe {
                qjs::JS_NewAtomUInt32(c, u32::try_from(i).unwrap_or(u32::MAX))
            });
            unsafe { (*tab.add(i)).atom == index.1 }
        })
        && unsafe { (*tab.add(len)).atom == length.1 };
    unsafe { qjs::JS_FreePropertyEnum(c, tab, count) };
    if dense { Ok(len) } else { Err(NotDense::Shape) }
}

/// The prototype of a non-Proxy object, without running project code.
pub fn proto<'js>(v: &Value<'js>) -> Option<Value<'js>> {
    if is_proxy(v) {
        return None;
    }
    v.as_object()
        .and_then(|o| o.get_prototype())
        .map(rquickjs::Object::into_value)
}

/// Whether two values are the same object.
pub fn same<'js>(a: &Value<'js>, b: &Value<'js>) -> bool {
    a.is_object() && b.is_object() && a == b
}

/// A string value as Rust text; `None` for a string holding a lone surrogate (QuickJS-ng would
/// encode it as an invalid three-byte sequence, script-host.md 5.8).
pub fn text(v: &Value<'_>) -> Option<Result<String, ()>> {
    let s = v.as_string()?;
    Some(s.to_string().map_err(|_| ()))
}

/// Marks an exception value uncatchable (an Error object; any other value is left as it is).
pub fn make_uncatchable(ctx: &Ctx<'_>, v: &Value<'_>) {
    unsafe { qjs::JS_SetUncatchableError(raw(ctx), v.as_raw()) };
}

/// Whether a caught value is an uncatchable error.
pub fn is_uncatchable(v: &Value<'_>) -> bool {
    unsafe { qjs::JS_IsUncatchableError(v.as_raw()) }
}

/// Defines an own enumerable, writable, configurable data property without calling a setter (a
/// key such as `__proto__` from plain data must not reach `Object.prototype`'s accessor).
pub fn define<'js>(
    ctx: &Ctx<'js>,
    obj: &Value<'js>,
    key: &str,
    value: Value<'js>,
) -> rquickjs::Result<()> {
    let a = atom(ctx, key);
    // JS_DefinePropertyValue takes ownership of the value.
    let raw_value = unsafe { qjs::JS_DupValue(raw(ctx), value.as_raw()) };
    let r = unsafe {
        qjs::JS_DefinePropertyValue(
            raw(ctx),
            obj.as_raw(),
            a.1,
            raw_value,
            qjs::JS_PROP_C_W_E as _,
        )
    };
    if r < 0 {
        return Err(rquickjs::Error::Exception);
    }
    Ok(())
}

/// Every own property's value or accessor functions (string and symbol keys), read from the
/// descriptors: what a deep freeze walks. `obj` must not be a Proxy.
pub fn own_reachable<'js>(ctx: &Ctx<'js>, obj: &Value<'js>) -> Vec<Value<'js>> {
    if !obj.is_object() || is_proxy(obj) {
        return Vec::new();
    }
    let c = raw(ctx);
    let mut tab: *mut qjs::JSPropertyEnum = std::ptr::null_mut();
    let mut len: u32 = 0;
    let flags = qjs::JS_GPN_STRING_MASK | qjs::JS_GPN_SYMBOL_MASK;
    let r = unsafe { qjs::JS_GetOwnPropertyNames(c, &mut tab, &mut len, obj.as_raw(), flags as _) };
    if r < 0 {
        let _ = ctx.catch();
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..len as usize {
        let a = unsafe { (*tab.add(i)).atom };
        let mut desc = qjs::JSPropertyDescriptor {
            flags: 0,
            value: qjs::JS_UNDEFINED,
            getter: qjs::JS_UNDEFINED,
            setter: qjs::JS_UNDEFINED,
        };
        let found = unsafe { qjs::JS_GetOwnProperty(c, &mut desc, obj.as_raw(), a) };
        if found < 0 {
            let _ = ctx.catch();
            continue;
        }
        if found == 0 {
            continue;
        }
        for v in [desc.value, desc.getter, desc.setter] {
            let v = unsafe { Value::from_raw(ctx.clone(), v) };
            if v.is_object() {
                out.push(v);
            }
        }
    }
    unsafe { qjs::JS_FreePropertyEnum(c, tab, len) };
    out
}
