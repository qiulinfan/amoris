//! PCE bytes as JSON and back, directed by a resolved format (docs/spec/versions.md 8.1): structs
//! are objects keyed by field name, enums externally tagged, options `null` or the value, sequences
//! and tuples arrays, maps with string keys objects and any other an array of `[key, value]` pairs,
//! bytes arrays of numbers, `u128` and `i128` decimal strings, floats numbers with their bits.
//! The way back accepts an integral number (`5.0` as well as `5`) for an integer field within its
//! range and takes an integer for a float field as its exact double.

use serde_json::{Map, Number, Value};

use super::{ResolvedFormat as F, ResolvedVariant};
use crate::pce::{Decoder, MAX_DEPTH, PceError, write_uleb};

/// Why a conversion failed.
#[derive(Clone, Debug, PartialEq)]
pub enum JsonError {
    /// The bytes do not decode.
    Pce(PceError),
    /// A NaN or an infinity at this JSON Pointer path, which JSON cannot carry.
    Nonfinite(String),
    /// A value that does not fit the format at this path.
    Shape {
        path: String,
        expected: String,
        found: String,
    },
}

impl From<PceError> for JsonError {
    fn from(e: PceError) -> JsonError {
        JsonError::Pce(e)
    }
}

/// Appends a JSON Pointer segment (RFC 6901).
pub fn push_segment(path: &mut String, seg: &str) {
    path.push('/');
    path.push_str(&seg.replace('~', "~0").replace('/', "~1"));
}

pub(crate) fn is_container(f: &F) -> bool {
    matches!(
        f,
        F::UnitStruct | F::NewTypeStruct(_) | F::TupleStruct(_) | F::Struct(_) | F::Enum(_)
    )
}

/// Whether PCE counts `f` as a compound value around what it holds (`pce::MAX_DEPTH`) as soon as
/// it starts; an option counts only around the value it holds, an enum around a variant's content.
/// Reading counts on the decoder and writing with a depth of its own, so the conversion refuses
/// what the encoding refuses, before recursion through `Back` can exhaust the stack.
pub(crate) fn counted(f: &F) -> bool {
    matches!(
        f,
        F::Seq(_)
            | F::Map { .. }
            | F::Tuple(_)
            | F::TupleStruct(_)
            | F::TupleArray { .. }
            | F::NewTypeStruct(_)
            | F::Struct(_)
    )
}

/// The depth inside one more compound value while writing, refused past `MAX_DEPTH`.
fn deeper(depth: u32, path: &str) -> Result<u32, JsonError> {
    if depth >= MAX_DEPTH {
        return Err(JsonError::Shape {
            path: path.to_owned(),
            expected: format!("at most {MAX_DEPTH} nested values"),
            found: "a deeper value".to_owned(),
        });
    }
    Ok(depth + 1)
}

/// The format a `Back(n)` names.
pub(crate) fn back<'a>(stack: &[&'a F], n: u32) -> Result<&'a F, PceError> {
    let n = n as usize;
    stack
        .len()
        .checked_sub(n + 1)
        .map(|i| stack[i])
        .ok_or_else(|| PceError::noncanonical(0, "a recursive format without its container"))
}

/// A float as JSON; with `strict`, a NaN or an infinity fails, else it renders as a string.
fn float(x: f64, strict: bool, path: &str) -> Result<Value, JsonError> {
    match Number::from_f64(x) {
        Some(n) => Ok(Value::Number(n)),
        None if strict => Err(JsonError::Nonfinite(path.to_owned())),
        None => Ok(Value::String(format!("{x}"))),
    }
}

/// Reads one value of format `f` from `d` as JSON.
pub fn read(f: &F, d: &mut Decoder<'_>, strict: bool) -> Result<Value, JsonError> {
    let mut stack = Vec::new();
    read_at(f, d, strict, &mut stack, &mut String::new())
}

/// Reads a whole byte string of format `f` as JSON.
pub fn to_json(f: &F, bytes: &[u8], strict: bool) -> Result<Value, JsonError> {
    let mut d = Decoder::new(bytes, false);
    let v = read(f, &mut d, strict)?;
    d.finish()?;
    Ok(v)
}

pub(crate) fn read_at<'f>(
    f: &'f F,
    d: &mut Decoder<'_>,
    strict: bool,
    stack: &mut Vec<&'f F>,
    path: &mut String,
) -> Result<Value, JsonError> {
    if let F::Back(n) = f {
        let target = back(stack, *n)?;
        return read_at(target, d, strict, stack, path);
    }
    let count = counted(f);
    if count {
        d.enter()?;
    }
    let pushed = is_container(f);
    if pushed {
        stack.push(f);
    }
    let out = read_inner(f, d, strict, stack, path);
    if pushed {
        stack.pop();
    }
    if count {
        d.leave();
    }
    out
}

fn read_list<'f>(
    fs: impl Iterator<Item = &'f F>,
    d: &mut Decoder<'_>,
    strict: bool,
    stack: &mut Vec<&'f F>,
    path: &mut String,
) -> Result<Value, JsonError> {
    let mut out = Vec::new();
    for (i, f) in fs.enumerate() {
        let len = path.len();
        push_segment(path, &i.to_string());
        out.push(read_at(f, d, strict, stack, path)?);
        path.truncate(len);
    }
    Ok(Value::Array(out))
}

fn read_fields<'f>(
    fs: &'f [(String, F)],
    d: &mut Decoder<'_>,
    strict: bool,
    stack: &mut Vec<&'f F>,
    path: &mut String,
) -> Result<Value, JsonError> {
    let mut out = Map::new();
    for (name, f) in fs {
        let len = path.len();
        push_segment(path, name);
        out.insert(name.clone(), read_at(f, d, strict, stack, path)?);
        path.truncate(len);
    }
    Ok(Value::Object(out))
}

/// A value without parts. Kept out of [`read_inner`], which recursion passes through, so that its
/// frame stays small (debug builds give every arm's temporaries their own stack slots).
fn read_leaf(f: &F, d: &mut Decoder<'_>, strict: bool, path: &str) -> Result<Value, JsonError> {
    Ok(match f {
        F::Unit | F::UnitStruct => {
            d.value::<()>()?;
            Value::Null
        }
        F::Bool => Value::Bool(d.value()?),
        F::I8 => Value::from(d.value::<i8>()?),
        F::I16 => Value::from(d.value::<i16>()?),
        F::I32 => Value::from(d.value::<i32>()?),
        F::I64 => Value::from(d.value::<i64>()?),
        F::I128 => Value::String(d.value::<i128>()?.to_string()),
        F::U8 => Value::from(d.value::<u8>()?),
        F::U16 => Value::from(d.value::<u16>()?),
        F::U32 => Value::from(d.value::<u32>()?),
        F::U64 => Value::from(d.value::<u64>()?),
        F::U128 => Value::String(d.value::<u128>()?.to_string()),
        F::F32 => float(f64::from(d.value::<f32>()?), strict, path)?,
        F::F64 => float(d.value::<f64>()?, strict, path)?,
        F::Char => Value::String(d.value::<char>()?.to_string()),
        F::Str => Value::String(d.value::<String>()?),
        F::Bytes => {
            let b: &[u8] = d.value()?;
            Value::Array(b.iter().map(|x| Value::from(*x)).collect())
        }
        _ => {
            return Err(
                PceError::noncanonical(d.pos(), "a value with parts read as a leaf").into(),
            );
        }
    })
}

/// One value with parts. Each kind is a function of its own, so that recursion, which passes
/// through here, keeps small frames (bounded by `pce::MAX_DEPTH`, but debug builds spend kilobytes
/// on every frame that holds many `?`).
fn read_inner<'f>(
    f: &'f F,
    d: &mut Decoder<'_>,
    strict: bool,
    stack: &mut Vec<&'f F>,
    path: &mut String,
) -> Result<Value, JsonError> {
    match f {
        F::Option(x) => read_option(x, d, strict, stack, path),
        F::Seq(x) => read_seq(x, d, strict, stack, path),
        F::Map { key, value } => read_map((key, value), d, strict, stack, path),
        F::Tuple(xs) | F::TupleStruct(xs) => read_list(xs.iter(), d, strict, stack, path),
        F::TupleArray { content, size } => {
            read_list((0..*size).map(|_| &**content), d, strict, stack, path)
        }
        F::NewTypeStruct(x) => read_at(x, d, strict, stack, path),
        F::Struct(fs) => read_fields(fs, d, strict, stack, path),
        F::Enum(variants) => read_enum(variants, d, strict, stack, path),
        F::Back(_) => unreachable!("handled by read_at"),
        _ => read_leaf(f, d, strict, path),
    }
}

fn read_option<'f>(
    x: &'f F,
    d: &mut Decoder<'_>,
    strict: bool,
    stack: &mut Vec<&'f F>,
    path: &mut String,
) -> Result<Value, JsonError> {
    let tag: u8 = d.value()?;
    match tag {
        0 => Ok(Value::Null),
        1 => {
            d.enter()?;
            let v = read_at(x, d, strict, stack, path);
            d.leave();
            v
        }
        _ => Err(PceError::noncanonical(d.pos(), "an option tag").into()),
    }
}

fn read_seq<'f>(
    x: &'f F,
    d: &mut Decoder<'_>,
    strict: bool,
    stack: &mut Vec<&'f F>,
    path: &mut String,
) -> Result<Value, JsonError> {
    let n = d.uleb()?;
    let mut out = Vec::new();
    for i in 0..n {
        let len = path.len();
        push_segment(path, &i.to_string());
        out.push(read_at(x, d, strict, stack, path)?);
        path.truncate(len);
    }
    Ok(Value::Array(out))
}

fn read_map<'f>(
    (key, value): (&'f F, &'f F),
    d: &mut Decoder<'_>,
    strict: bool,
    stack: &mut Vec<&'f F>,
    path: &mut String,
) -> Result<Value, JsonError> {
    let n = d.uleb()?;
    let string_keys = matches!(key, F::Str);
    let mut obj = Map::new();
    let mut pairs = Vec::new();
    for _ in 0..n {
        let k = read_at(key, d, strict, stack, path)?;
        let len = path.len();
        push_segment(
            path,
            &k.as_str().map_or_else(|| k.to_string(), str::to_owned),
        );
        let v = read_at(value, d, strict, stack, path)?;
        path.truncate(len);
        match (string_keys, k) {
            (true, Value::String(s)) => {
                obj.insert(s, v);
            }
            (_, k) => pairs.push(Value::Array(vec![k, v])),
        }
    }
    Ok(if string_keys {
        Value::Object(obj)
    } else {
        Value::Array(pairs)
    })
}

fn read_enum<'f>(
    variants: &'f [(u32, String, ResolvedVariant)],
    d: &mut Decoder<'_>,
    strict: bool,
    stack: &mut Vec<&'f F>,
    path: &mut String,
) -> Result<Value, JsonError> {
    let at = d.pos();
    let i = d.uleb()?;
    let (_, name, v) = variants
        .iter()
        .find(|(j, _, _)| u64::from(*j) == i)
        .ok_or_else(|| PceError::noncanonical(at, format!("variant index {i}")))?;
    if matches!(v, ResolvedVariant::Unit) {
        return Ok(Value::String(name.clone()));
    }
    let len = path.len();
    push_segment(path, name);
    d.enter()?;
    let r = match v {
        ResolvedVariant::NewType(x) => read_at(x, d, strict, stack, path),
        ResolvedVariant::Tuple(xs) => read_list(xs.iter(), d, strict, stack, path),
        ResolvedVariant::Struct(fs) => read_fields(fs, d, strict, stack, path),
        ResolvedVariant::Unit => Ok(Value::Null),
    };
    d.leave();
    path.truncate(len);
    let mut m = Map::new();
    m.insert(name.clone(), r?);
    Ok(Value::Object(m))
}

fn kind(v: &Value) -> String {
    match v {
        Value::Null => "null".into(),
        Value::Bool(_) => "a boolean".into(),
        Value::Number(n) => format!("the number {n}"),
        Value::String(_) => "a string".into(),
        Value::Array(a) => format!("an array of {}", a.len()),
        Value::Object(_) => "an object".into(),
    }
}

fn shape(path: &str, expected: &str, v: &Value) -> JsonError {
    JsonError::Shape {
        path: path.to_owned(),
        expected: expected.to_owned(),
        found: kind(v),
    }
}

/// An integral number within `min..=max` (numeric.md 8: `5.0` is 5; nothing rounds or wraps).
fn integer(v: &Value, min: i128, max: i128, what: &str, path: &str) -> Result<i128, JsonError> {
    let n = match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(i128::from(i))
            } else if let Some(u) = n.as_u64() {
                Some(i128::from(u))
            } else {
                n.as_f64()
                    .filter(|x| x.is_finite() && *x == x.trunc() && x.abs() < 1.8e19)
                    .map(|x| {
                        #[allow(clippy::cast_possible_truncation)] // integral and below 2^64
                        let i = x as i128;
                        i
                    })
            }
        }
        Value::String(s) if matches!(what, "i128" | "u128") => s.parse::<i128>().ok(),
        _ => None,
    };
    n.filter(|n| (min..=max).contains(n))
        .ok_or_else(|| shape(path, &format!("an integral {what}"), v))
}

/// Writes `v` as the PCE of format `f`.
pub fn from_json(f: &F, v: &Value, out: &mut Vec<u8>) -> Result<(), JsonError> {
    write_at(f, v, out, &mut Vec::new(), &mut String::new(), 0)
}

/// Writes a value without parts; kept out of [`write_at`] for the size of its frame, as
/// [`read_leaf`].
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)] // ranges checked by `integer`; integer-to-double conversions checked exact
fn write_leaf(f: &F, v: &Value, out: &mut Vec<u8>, path: &mut String) -> Result<(), JsonError> {
    match f {
        F::Unit | F::UnitStruct => {
            if !v.is_null() {
                return Err(shape(path, "null", v));
            }
        }
        F::Bool => out.push(u8::from(
            v.as_bool().ok_or_else(|| shape(path, "a boolean", v))?,
        )),
        F::I8 => out.extend_from_slice(&(integer(v, -128, 127, "i8", path)? as i8).to_le_bytes()),
        F::I16 => out.extend_from_slice(
            &(integer(v, i16::MIN.into(), i16::MAX.into(), "i16", path)? as i16).to_le_bytes(),
        ),
        F::I32 => out.extend_from_slice(
            &(integer(v, i32::MIN.into(), i32::MAX.into(), "i32", path)? as i32).to_le_bytes(),
        ),
        F::I64 => out.extend_from_slice(
            &(integer(v, i64::MIN.into(), i64::MAX.into(), "i64", path)? as i64).to_le_bytes(),
        ),
        F::I128 => {
            out.extend_from_slice(&integer(v, i128::MIN, i128::MAX, "i128", path)?.to_le_bytes())
        }
        F::U8 => out.push(integer(v, 0, 255, "u8", path)? as u8),
        F::U16 => out.extend_from_slice(
            &(integer(v, 0, u16::MAX.into(), "u16", path)? as u16).to_le_bytes(),
        ),
        F::U32 => out.extend_from_slice(
            &(integer(v, 0, u32::MAX.into(), "u32", path)? as u32).to_le_bytes(),
        ),
        F::U64 => out.extend_from_slice(
            &(integer(v, 0, u64::MAX.into(), "u64", path)? as u64).to_le_bytes(),
        ),
        F::U128 => {
            let n = match v {
                Value::String(s) => s.parse::<u128>().ok(),
                Value::Number(n) => n.as_u64().map(u128::from),
                _ => None,
            };
            let n = n.ok_or_else(|| shape(path, "an integral u128", v))?;
            out.extend_from_slice(&n.to_le_bytes());
        }
        F::F32 | F::F64 => {
            let Value::Number(n) = v else {
                return Err(shape(path, "a number", v));
            };
            let x = if let Some(u) = n.as_u64() {
                let x = u as f64;
                (x as u64 == u && x < 1.8e19).then_some(x)
            } else if let Some(i) = n.as_i64() {
                let x = i as f64;
                (x as i64 == i).then_some(x)
            } else {
                n.as_f64()
            };
            let x = x.ok_or_else(|| shape(path, "a number exact as a double", v))?;
            if !x.is_finite() {
                return Err(JsonError::Nonfinite(path.clone()));
            }
            if matches!(f, F::F32) {
                let y = x as f32;
                if f64::from(y).to_bits() != x.to_bits() {
                    return Err(shape(path, "a number exact as an f32", v));
                }
                out.extend_from_slice(&y.to_bits().to_le_bytes());
            } else {
                out.extend_from_slice(&x.to_bits().to_le_bytes());
            }
        }
        F::Char => {
            let s = v
                .as_str()
                .ok_or_else(|| shape(path, "a one-character string", v))?;
            let mut cs = s.chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) => out.extend_from_slice(&u32::from(c).to_le_bytes()),
                _ => return Err(shape(path, "a one-character string", v)),
            }
        }
        F::Str => {
            let s = v.as_str().ok_or_else(|| shape(path, "a string", v))?;
            write_uleb(out, s.len() as u64);
            out.extend_from_slice(s.as_bytes());
        }
        F::Bytes => {
            let a = v
                .as_array()
                .ok_or_else(|| shape(path, "an array of bytes", v))?;
            write_uleb(out, a.len() as u64);
            for (i, x) in a.iter().enumerate() {
                let len = path.len();
                push_segment(path, &i.to_string());
                out.push(integer(x, 0, 255, "u8", path)? as u8);
                path.truncate(len);
            }
        }
        _ => return Err(shape(path, "a value without parts", v)),
    }
    Ok(())
}

fn write_at<'f>(
    f: &'f F,
    v: &Value,
    out: &mut Vec<u8>,
    stack: &mut Vec<&'f F>,
    path: &mut String,
    depth: u32,
) -> Result<(), JsonError> {
    if let F::Back(n) = f {
        let target = back(stack, *n)?;
        return write_at(target, v, out, stack, path, depth);
    }
    // `depth` counts the compound values around `f`; inside a counted `f` its parts are deeper.
    let depth = if counted(f) {
        deeper(depth, path)?
    } else {
        depth
    };
    let pushed = is_container(f);
    if pushed {
        stack.push(f);
    }
    let r = write_inner(f, v, out, stack, path, depth);
    if pushed {
        stack.pop();
    }
    r
}

/// One value, each kind with parts a function of its own (small frames, as [`read_inner`]).
fn write_inner<'f>(
    f: &'f F,
    v: &Value,
    out: &mut Vec<u8>,
    stack: &mut Vec<&'f F>,
    path: &mut String,
    depth: u32,
) -> Result<(), JsonError> {
    match f {
        F::Option(x) => write_option(x, v, out, stack, path, depth),
        F::Seq(x) => write_seq(x, v, out, stack, path, depth),
        F::Map { key, value } => write_map((key, value), v, out, stack, path, depth),
        F::Tuple(xs) | F::TupleStruct(xs) => {
            write_fixed(xs.iter(), xs.len(), v, out, stack, path, depth)
        }
        F::TupleArray { content, size } => {
            let n = usize::try_from(*size).unwrap_or(usize::MAX);
            write_fixed((0..n).map(|_| &**content), n, v, out, stack, path, depth)
        }
        F::NewTypeStruct(x) => write_at(x, v, out, stack, path, depth),
        F::Struct(fs) => write_struct(fs, v, out, stack, path, depth),
        F::Enum(variants) => write_enum(variants, v, out, stack, path, depth),
        F::Back(_) => unreachable!("handled by write_at"),
        _ => write_leaf(f, v, out, path),
    }
}

fn write_option<'f>(
    x: &'f F,
    v: &Value,
    out: &mut Vec<u8>,
    stack: &mut Vec<&'f F>,
    path: &mut String,
    depth: u32,
) -> Result<(), JsonError> {
    if v.is_null() {
        out.push(0);
        return Ok(());
    }
    out.push(1);
    write_at(x, v, out, stack, path, deeper(depth, path)?)
}

fn write_seq<'f>(
    x: &'f F,
    v: &Value,
    out: &mut Vec<u8>,
    stack: &mut Vec<&'f F>,
    path: &mut String,
    depth: u32,
) -> Result<(), JsonError> {
    let a = v.as_array().ok_or_else(|| shape(path, "an array", v))?;
    write_uleb(out, a.len() as u64);
    write_items(a.iter().map(|e| (x, e)), out, stack, path, depth)
}

fn write_map<'f>(
    (key, value): (&'f F, &'f F),
    v: &Value,
    out: &mut Vec<u8>,
    stack: &mut Vec<&'f F>,
    path: &mut String,
    depth: u32,
) -> Result<(), JsonError> {
    match v {
        Value::Object(m) if matches!(key, F::Str) => {
            write_uleb(out, m.len() as u64);
            for (k, e) in m {
                write_at(key, &Value::String(k.clone()), out, stack, path, depth)?;
                let len = path.len();
                push_segment(path, k);
                write_at(value, e, out, stack, path, depth)?;
                path.truncate(len);
            }
        }
        Value::Array(pairs) => {
            write_uleb(out, pairs.len() as u64);
            for (i, p) in pairs.iter().enumerate() {
                let len = path.len();
                push_segment(path, &i.to_string());
                match p.as_array().map(Vec::as_slice) {
                    Some([k, e]) => {
                        write_at(key, k, out, stack, path, depth)?;
                        write_at(value, e, out, stack, path, depth)?;
                    }
                    _ => return Err(shape(path, "a [key, value] pair", p)),
                }
                path.truncate(len);
            }
        }
        _ => return Err(shape(path, "a map", v)),
    }
    Ok(())
}

fn write_enum<'f>(
    variants: &'f [(u32, String, ResolvedVariant)],
    v: &Value,
    out: &mut Vec<u8>,
    stack: &mut Vec<&'f F>,
    path: &mut String,
    depth: u32,
) -> Result<(), JsonError> {
    let (name, inner) = match v {
        Value::String(s) => (s.as_str(), None),
        Value::Object(m) if m.len() == 1 => {
            let (k, e) = m.iter().next().ok_or_else(|| shape(path, "a variant", v))?;
            (k.as_str(), Some(e))
        }
        _ => return Err(shape(path, "a variant name or {name: value}", v)),
    };
    let (i, _, var) = variants
        .iter()
        .find(|(_, n, _)| n == name)
        .ok_or_else(|| shape(path, "a known variant", v))?;
    write_uleb(out, u64::from(*i));
    let len = path.len();
    push_segment(path, name);
    match (var, inner) {
        (ResolvedVariant::Unit, None) => {}
        (ResolvedVariant::NewType(x), Some(e)) => {
            write_at(x, e, out, stack, path, deeper(depth, path)?)?
        }
        (ResolvedVariant::Tuple(xs), Some(e)) => {
            let d = deeper(depth, path)?;
            write_fixed(xs.iter(), xs.len(), e, out, stack, path, d)?;
        }
        (ResolvedVariant::Struct(fs), Some(e)) => {
            write_struct(fs, e, out, stack, path, deeper(depth, path)?)?
        }
        _ => return Err(shape(path, "the variant's fields", v)),
    }
    path.truncate(len);
    Ok(())
}

fn write_items<'f, 'v>(
    items: impl Iterator<Item = (&'f F, &'v Value)>,
    out: &mut Vec<u8>,
    stack: &mut Vec<&'f F>,
    path: &mut String,
    depth: u32,
) -> Result<(), JsonError> {
    for (i, (f, e)) in items.enumerate() {
        let len = path.len();
        push_segment(path, &i.to_string());
        write_at(f, e, out, stack, path, depth)?;
        path.truncate(len);
    }
    Ok(())
}

fn write_fixed<'f>(
    fs: impl Iterator<Item = &'f F>,
    n: usize,
    v: &Value,
    out: &mut Vec<u8>,
    stack: &mut Vec<&'f F>,
    path: &mut String,
    depth: u32,
) -> Result<(), JsonError> {
    let a = v
        .as_array()
        .filter(|a| a.len() == n)
        .ok_or_else(|| shape(path, &format!("an array of {n}"), v))?;
    write_items(fs.zip(a.iter()), out, stack, path, depth)
}

fn write_struct<'f>(
    fs: &'f [(String, F)],
    v: &Value,
    out: &mut Vec<u8>,
    stack: &mut Vec<&'f F>,
    path: &mut String,
    depth: u32,
) -> Result<(), JsonError> {
    let m = v.as_object().ok_or_else(|| shape(path, "an object", v))?;
    if let Some(extra) = m.keys().find(|k| !fs.iter().any(|(n, _)| n == *k)) {
        let len = path.len();
        push_segment(path, extra);
        let e = shape(path, "no such field", &m[extra]);
        path.truncate(len);
        return Err(e);
    }
    for (name, f) in fs {
        let len = path.len();
        push_segment(path, name);
        let e = m.get(name).ok_or_else(|| JsonError::Shape {
            path: path.clone(),
            expected: "this field".into(),
            found: "nothing".into(),
        })?;
        write_at(f, e, out, stack, path, depth)?;
        path.truncate(len);
    }
    Ok(())
}
