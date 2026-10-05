//! Values across the boundary (docs/spec/script-host.md 5.3, 5.8 and 6): patches into component
//! slots with their range checks, column cells, plain data, and component values and events back
//! as frozen JavaScript values. Nothing here runs JavaScript: properties are read through their
//! descriptors, and a getter, a setter or a Proxy is refused.

use std::sync::Arc;

use pocket_sim::num::{self, NumError};
use pocket_sim::registry::{ComponentSchema, FieldType, ProjectValues};
use pocket_sim::{EntityId, PlainData};
use rquickjs::{Array, Ctx, Object, Value};
use serde_json::json;

use crate::access::str_index;
use crate::error::{ErrorPhase, ScriptError, num as jnum};
use crate::js::{self, Own};

/// The longest string a `str` field holds, in UTF-8 bytes.
pub const MAX_STR_BYTES: usize = 1024;
/// The largest plain data value, in bytes of its JSON form.
pub const MAX_DATA_BYTES: usize = 16 * 1024;

/// A patch of a component: changed slots and strings.
#[derive(Clone, Debug, Default)]
pub struct Patch {
    pub nums: Vec<(u16, f64)>,
    pub strs: Vec<(usize, Arc<str>)>,
}

impl Patch {
    pub fn apply(&self, v: &mut ProjectValues) {
        for (slot, x) in &self.nums {
            v.nums[usize::from(*slot)] = *x;
        }
        for (i, s) in &self.strs {
            v.strs[*i] = s.clone();
        }
    }
}

fn write_error(
    code: &str,
    message: String,
    schema: &ComponentSchema,
    field: &str,
    value: serde_json::Value,
) -> ScriptError {
    ScriptError::new(code, message, ErrorPhase::Run)
        .component(&schema.name)
        .field(field)
        .value(value)
}

/// `Object.prototype` of the context.
fn object_prototype<'js>(ctx: &Ctx<'js>) -> Option<Value<'js>> {
    let object = match js::own(ctx, &ctx.globals().into_value(), "Object") {
        Own::Data(v) => v,
        _ => return None,
    };
    match js::own(ctx, &object, "prototype") {
        Own::Data(v) => Some(v),
        _ => None,
    }
}

/// Whether `v` is a plain object: not a Proxy, not an array, its prototype `Object.prototype` or
/// null (a frozen `world.get` copy passes; class instances, typed arrays, maps and dates do not).
pub fn is_plain_object<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> bool {
    if !v.is_object() || v.is_array() || v.is_function() || js::is_proxy(v) {
        return false;
    }
    match js::proto(v) {
        None => true,
        Some(p) if p.is_null() => true,
        Some(p) => object_prototype(ctx).is_some_and(|op| js::same(&op, &p)),
    }
}

fn from_num_error(e: NumError, schema: &ComponentSchema, field: &str, x: f64) -> ScriptError {
    match e {
        NumError::NotFinite => write_error(
            "script.write_not_finite",
            format!(
                "{}.{field} cannot hold {}: only finite numbers enter the world.",
                schema.name,
                jnum(x)
            ),
            schema,
            field,
            jnum(x),
        ),
        NumError::NotInteger => write_error(
            "script.write_not_integer",
            format!(
                "{}.{field} takes a whole number; got {}.",
                schema.name,
                jnum(x)
            ),
            schema,
            field,
            jnum(x),
        )
        .hint("round it first: Math.round, Math.floor or Math.trunc"),
        NumError::OutOfRange { min, max } => write_error(
            "script.write_out_of_range",
            format!(
                "{}.{field} must be from {} to {}; got {}.",
                schema.name,
                jnum(min),
                jnum(max),
                jnum(x)
            ),
            schema,
            field,
            jnum(x),
        ),
    }
}

/// A numeric slot's value from a number written to a field of type `ty` (a column cell or a
/// scalar in a patch). Integers store exactly; `-0` stores 0 in integer fields.
pub fn check_number(
    ty: &FieldType,
    x: f64,
    schema: &ComponentSchema,
    field: &str,
    entity_ok: &dyn Fn(EntityId) -> bool,
) -> Result<f64, ScriptError> {
    let fail = |e| from_num_error(e, schema, field, x);
    match ty {
        FieldType::F64 | FieldType::Vec2 | FieldType::Vec3 | FieldType::Vec4 | FieldType::Quat => {
            num::finite(x).map_err(fail)
        }
        FieldType::I32 => num::to_i32(x).map(f64::from).map_err(fail),
        FieldType::U32 => num::to_u32(x).map(f64::from).map_err(fail),
        FieldType::Tick => num::to_tick(x).map(|t| t.to_f64()).map_err(fail),
        FieldType::Bool => {
            if x == 0.0 || x == 1.0 {
                Ok(if x == 0.0 { 0.0 } else { 1.0 })
            } else {
                Err(write_error(
                    "script.write_type",
                    format!(
                        "{}.{field} is a bool: its column holds 0 or 1, not {}.",
                        schema.name,
                        jnum(x)
                    ),
                    schema,
                    field,
                    jnum(x),
                ))
            }
        }
        FieldType::Enum(variants) => {
            let i = num::to_u32(x).ok().and_then(|i| usize::try_from(i).ok());
            match i {
                Some(i) if i < variants.len() => Ok(f64::from(u32::try_from(i).unwrap_or(0))),
                _ => Err(write_error(
                    "script.write_type",
                    format!(
                        "{}.{field} is an enum of {} variants; {} is not a variant index.",
                        schema.name,
                        variants.len(),
                        jnum(x)
                    ),
                    schema,
                    field,
                    jnum(x),
                )
                .with(
                    "variants",
                    json!(variants.iter().map(|v| &**v).collect::<Vec<_>>()),
                )),
            }
        }
        FieldType::Entity => {
            if x == 0.0 {
                return Ok(0.0);
            }
            match EntityId::from_f64(x) {
                Ok(id) if entity_ok(id) => Ok(id.to_f64()),
                _ => Err(write_error(
                    "script.entity_missing",
                    format!(
                        "{}.{field} names entity {}, which does not exist.",
                        schema.name,
                        jnum(x)
                    ),
                    schema,
                    field,
                    jnum(x),
                )),
            }
        }
        FieldType::Str => Err(write_error(
            "script.write_type",
            format!("{}.{field} is a string and has no column.", schema.name),
            schema,
            field,
            jnum(x),
        )),
    }
}

/// A string value as text, refusing a lone surrogate.
pub fn string_arg(v: &Value<'_>) -> Option<Result<String, ()>> {
    js::text(v)
}

fn parts(ty: &FieldType) -> &'static [&'static str] {
    match ty {
        FieldType::Vec2 => &["x", "y"],
        FieldType::Vec3 => &["x", "y", "z"],
        FieldType::Vec4 | FieldType::Quat => &["x", "y", "z", "w"],
        _ => &[],
    }
}

/// A patch of `schema` from a JavaScript object, checked key by key in own-key order (the first
/// problem is reported); vectors by part.
pub fn patch_from_js<'js>(
    ctx: &Ctx<'js>,
    schema: &ComponentSchema,
    v: &Value<'js>,
    entity_ok: &dyn Fn(EntityId) -> bool,
) -> Result<Patch, ScriptError> {
    let mut patch = Patch::default();
    if v.is_undefined() || v.is_null() {
        return Ok(patch);
    }
    if !is_plain_object(ctx, v) {
        return Err(ScriptError::new(
            "script.bad_data",
            format!(
                "The value for {} must be a plain object of its fields.",
                schema.name
            ),
            ErrorPhase::Run,
        )
        .component(&schema.name));
    }
    for key in js::own_keys(ctx, v, true) {
        let Some(fi) = schema.fields.iter().position(|f| *f.name == *key) else {
            let names = schema.fields.iter().map(|f| &*f.name);
            return Err(ScriptError::new(
                "script.unknown_field",
                format!("{} has no field '{key}'.", schema.name),
                ErrorPhase::Run,
            )
            .component(&schema.name)
            .field(&key)
            .suggest(pocket_contract::suggest::suggest_names(&key, names)));
        };
        let f = &schema.fields[fi];
        let value = match js::own(ctx, v, &key) {
            Own::Data(x) => x,
            _ => {
                return Err(ScriptError::new(
                    "script.bad_data",
                    format!(
                        "{}.{key} is given by a getter; give plain values.",
                        schema.name
                    ),
                    ErrorPhase::Run,
                )
                .component(&schema.name)
                .field(&key));
            }
        };
        let type_error = |what: &str| {
            write_error(
                "script.write_type",
                format!("{}.{} takes {what}.", schema.name, f.name),
                schema,
                &f.name,
                json!(type_name(&value)),
            )
        };
        match &f.ty {
            FieldType::Str => {
                let text = match string_arg(&value) {
                    Some(Ok(t)) => t,
                    Some(Err(())) => return Err(type_error("a string without lone surrogates")),
                    None => return Err(type_error("a string")),
                };
                if text.len() > MAX_STR_BYTES {
                    return Err(write_error(
                        "script.write_too_long",
                        format!(
                            "{}.{} holds at most {MAX_STR_BYTES} bytes; this string has {}.",
                            schema.name,
                            f.name,
                            text.len()
                        ),
                        schema,
                        &f.name,
                        json!(text.len()),
                    ));
                }
                patch.strs.push((str_index(schema, fi), Arc::from(text)));
            }
            FieldType::Bool => {
                let Some(b) = value.as_bool() else {
                    return Err(type_error("true or false"));
                };
                patch.nums.push((f.first_slot, if b { 1.0 } else { 0.0 }));
            }
            FieldType::Enum(variants) => {
                let t = string_arg(&value).and_then(Result::ok);
                let Some(i) = t
                    .as_deref()
                    .and_then(|t| variants.iter().position(|n| **n == *t))
                else {
                    return Err(type_error("one of its variant names").with(
                        "variants",
                        json!(variants.iter().map(|v| &**v).collect::<Vec<_>>()),
                    ));
                };
                patch
                    .nums
                    .push((f.first_slot, f64::from(u32::try_from(i).unwrap_or(0))));
            }
            FieldType::Entity => {
                if value.is_null() {
                    patch.nums.push((f.first_slot, 0.0));
                    continue;
                }
                let Some(x) = value.as_number() else {
                    return Err(type_error("an entity or null"));
                };
                let x = check_number(&f.ty, x, schema, &f.name, entity_ok)?;
                if x == 0.0 {
                    return Err(type_error("an entity or null"));
                }
                patch.nums.push((f.first_slot, x));
            }
            FieldType::Vec2 | FieldType::Vec3 | FieldType::Vec4 | FieldType::Quat => {
                if !is_plain_object(ctx, &value) {
                    return Err(type_error("an object of its parts"));
                }
                let names = parts(&f.ty);
                for part in js::own_keys(ctx, &value, true) {
                    let Some(pi) = names.iter().position(|p| *p == part) else {
                        return Err(ScriptError::new(
                            "script.unknown_field",
                            format!("{}.{} has no part '{part}'.", schema.name, f.name),
                            ErrorPhase::Run,
                        )
                        .component(&schema.name)
                        .field(&format!("{}.{part}", f.name))
                        .suggest(pocket_contract::suggest::suggest_names(
                            &part,
                            names.iter().copied(),
                        )));
                    };
                    let field = format!("{}.{part}", f.name);
                    let x = match js::own(ctx, &value, &part) {
                        Own::Data(x) => x.as_number(),
                        _ => None,
                    };
                    let Some(x) = x else {
                        return Err(write_error(
                            "script.write_type",
                            format!("{}.{field} takes a number.", schema.name),
                            schema,
                            &field,
                            json!(null),
                        ));
                    };
                    let x = check_number(&f.ty, x, schema, &field, entity_ok)?;
                    let slot = f.first_slot + u16::try_from(pi).unwrap_or(0);
                    patch.nums.push((slot, x));
                }
            }
            _ => {
                let Some(x) = value.as_number() else {
                    return Err(type_error("a number"));
                };
                let x = check_number(&f.ty, x, schema, &f.name, entity_ok)?;
                patch.nums.push((f.first_slot, x));
            }
        }
    }
    Ok(patch)
}

fn type_name(v: &Value<'_>) -> &'static str {
    if v.is_undefined() {
        "undefined"
    } else if v.is_null() {
        "null"
    } else if v.is_bool() {
        "boolean"
    } else if v.is_number() {
        "number"
    } else if v.is_string() {
        "string"
    } else if v.is_array() {
        "array"
    } else if v.is_function() {
        "function"
    } else if v.is_object() {
        "object"
    } else {
        "other"
    }
}

fn frozen<'js>(ctx: &Ctx<'js>, o: Object<'js>) -> rquickjs::Result<Value<'js>> {
    let v = o.into_value();
    unsafe { rquickjs::qjs::JS_FreezeObject(js::raw(ctx), v.as_raw()) };
    Ok(v)
}

/// A component's values as a frozen plain copy: fields in declaration order, vectors as
/// `{x, y, z, w}`, enums as variant names, entities as ids or null.
pub fn component_to_js<'js>(
    ctx: &Ctx<'js>,
    schema: &ComponentSchema,
    v: &ProjectValues,
) -> rquickjs::Result<Value<'js>> {
    let o = Object::new(ctx.clone())?;
    let mut s = 0usize;
    for f in &schema.fields {
        let slot = usize::from(f.first_slot);
        let x = v.nums.get(slot).copied().unwrap_or(0.0);
        match &f.ty {
            FieldType::Str => {
                o.set(&*f.name, v.strs.get(s).map_or("", |t| &**t))?;
                s += 1;
            }
            FieldType::Bool => o.set(&*f.name, x != 0.0)?,
            FieldType::Enum(variants) => {
                let i = num::to_u32(x)
                    .ok()
                    .and_then(|i| usize::try_from(i).ok())
                    .unwrap_or(0);
                o.set(&*f.name, variants.get(i).map_or("", |n| &**n))?;
            }
            FieldType::Entity => {
                if x == 0.0 {
                    o.set(&*f.name, Value::new_null(ctx.clone()))?;
                } else {
                    o.set(&*f.name, x)?;
                }
            }
            FieldType::Vec2 | FieldType::Vec3 | FieldType::Vec4 | FieldType::Quat => {
                let p = Object::new(ctx.clone())?;
                for (i, name) in parts(&f.ty).iter().enumerate() {
                    p.set(*name, v.nums.get(slot + i).copied().unwrap_or(0.0))?;
                }
                o.set(&*f.name, frozen(ctx, p)?)?;
            }
            _ => o.set(&*f.name, x)?,
        }
    }
    frozen(ctx, o)
}

fn bad_data(path: &str, why: &str) -> ScriptError {
    ScriptError::new(
        "script.bad_data",
        format!(
            "The data at '{}' {why}.",
            if path.is_empty() { "/" } else { path }
        ),
        ErrorPhase::Run,
    )
    .with("path", json!(path))
}

/// `script.bad_data` for data whose JSON form is already past the limit.
fn too_big() -> ScriptError {
    bad_data(
        "",
        &format!(
            "is more than {MAX_DATA_BYTES} bytes as JSON; data holds at most {MAX_DATA_BYTES}"
        ),
    )
}

/// Adds `n` bytes to the running size of the JSON form, a lower bound of it, so data too large is
/// refused while it is read rather than after the host built and serialized all of it
/// (script-sandbox.md 4.3).
fn grow(size: &mut usize, n: usize) -> Result<(), ScriptError> {
    *size = size.saturating_add(n);
    if *size > MAX_DATA_BYTES {
        return Err(too_big());
    }
    Ok(())
}

/// Plain data from a JavaScript value (script-host.md 5.8).
pub fn plain_from_js<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> Result<PlainData, ScriptError> {
    let out = plain_at(ctx, v, &mut String::new(), 0, &mut 0)?;
    let size = serde_json::to_string(&out.to_json()).map_or(0, |s| s.len());
    if size > MAX_DATA_BYTES {
        return Err(bad_data(
            "",
            &format!("is {size} bytes; data holds at most {MAX_DATA_BYTES}"),
        ));
    }
    Ok(out)
}

fn plain_at<'js>(
    ctx: &Ctx<'js>,
    v: &Value<'js>,
    path: &mut String,
    depth: u32,
    size: &mut usize,
) -> Result<PlainData, ScriptError> {
    if depth > 64 {
        return Err(bad_data(
            path,
            "is nested more than 64 deep (or refers to itself)",
        ));
    }
    if v.is_null() {
        grow(size, 4)?;
        return Ok(PlainData::Null);
    }
    if let Some(b) = v.as_bool() {
        grow(size, 4)?;
        return Ok(PlainData::Bool(b));
    }
    if let Some(x) = v.as_number() {
        grow(size, 1)?;
        return PlainData::number(x).map_err(|_| bad_data(path, "is not a finite number"));
    }
    if let Some(t) = js::text(v) {
        let t = t.map_err(|()| bad_data(path, "holds a lone surrogate"))?;
        grow(size, t.len().saturating_add(2))?;
        return Ok(PlainData::String(t));
    }
    if v.is_undefined() {
        return Err(bad_data(
            path,
            "is undefined: leave the key out, or write null",
        ));
    }
    if js::is_proxy(v) {
        return Err(bad_data(path, "is a Proxy"));
    }
    if v.is_array() {
        // Each element is at least one byte and a separator of the JSON form.
        let len = match js::dense_array_len(ctx, v, MAX_DATA_BYTES / 2) {
            Ok(n) => n,
            Err(js::NotDense::TooLong(_)) => return Err(too_big()),
            Err(js::NotDense::Shape) => {
                return Err(bad_data(path, "is an array with holes or extra properties"));
            }
        };
        grow(size, 2)?;
        let mut items = Vec::with_capacity(len);
        for i in 0..len {
            if i > 0 {
                grow(size, 1)?;
            }
            let el = match js::own(ctx, v, &i.to_string()) {
                Own::Data(x) => x,
                _ => return Err(bad_data(path, &format!("has a getter at index {i}"))),
            };
            let n = path.len();
            path.push_str(&format!("/{i}"));
            items.push(plain_at(ctx, &el, path, depth + 1, size)?);
            path.truncate(n);
        }
        return Ok(PlainData::Array(items));
    }
    if !is_plain_object(ctx, v) {
        return Err(bad_data(
            path,
            "is not plain data (a class instance, a function, a typed array, a map or another object)",
        ));
    }
    grow(size, 2)?;
    // Each entry has its key's quotes and colon, and all but the last a separator: counted, and
    // refused when too many, before the keys are listed (values count as they are read).
    let count = js::own_key_count(ctx, v, true);
    if count > 0 {
        grow(size, count.saturating_mul(4) - 1)?;
    }
    let mut entries = Vec::new();
    for key in js::own_keys(ctx, v, true) {
        grow(size, key.len())?;
        let el = match js::own(ctx, v, &key) {
            Own::Data(x) => x,
            _ => {
                return Err(bad_data(
                    path,
                    &format!("has a getter or setter for '{key}'"),
                ));
            }
        };
        let n = path.len();
        path.push('/');
        path.push_str(&key.replace('~', "~0").replace('/', "~1"));
        let value = plain_at(ctx, &el, path, depth + 1, size)?;
        path.truncate(n);
        entries.push((key, value));
    }
    PlainData::object(entries).map_err(|e| bad_data(path, &e.message))
}

/// Plain data as a frozen JavaScript value.
pub fn plain_to_js<'js>(ctx: &Ctx<'js>, d: &PlainData) -> rquickjs::Result<Value<'js>> {
    Ok(match d {
        PlainData::Null => Value::new_null(ctx.clone()),
        PlainData::Bool(b) => Value::new_bool(ctx.clone(), *b),
        PlainData::Number(x) => Value::new_number(ctx.clone(), *x),
        PlainData::String(s) => rquickjs::String::from_str(ctx.clone(), s)?.into_value(),
        PlainData::Array(items) => {
            let a = Array::new(ctx.clone())?;
            for (i, x) in items.iter().enumerate() {
                a.set(i, plain_to_js(ctx, x)?)?;
            }
            frozen(ctx, a.into_object())?
        }
        PlainData::Object(entries) => {
            let o = Object::new(ctx.clone())?;
            let ov = o.clone().into_value();
            for (k, x) in entries {
                js::define(ctx, &ov, k, plain_to_js(ctx, x)?)?;
            }
            frozen(ctx, o)?
        }
    })
}

/// Freezes a fresh object made by the host.
pub fn freeze_new<'js>(ctx: &Ctx<'js>, o: Object<'js>) -> rquickjs::Result<Value<'js>> {
    frozen(ctx, o)
}
