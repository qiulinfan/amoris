//! Random streams (docs/spec/script-host.md 5.6; rng.md): handles over the world's per-tick stream
//! table, so scripts and Rust draw identical numbers; a handle used after its call is
//! `rng.stream_expired`, and `Math.random()` is the system stream's `next()`.

use pocket_contract::Problem;
use pocket_sim::rng::{KeyPart, StreamKey, StreamKind};
use pocket_sim::{EntityId, Pcg32, RngTable};
use rquickjs::{Ctx, TypedArray, Value};
use serde_json::json;

use super::{NResult, Thrown, arg, with_call};
use crate::call::CallState;
use crate::error::{ErrorPhase, ScriptError};
use crate::host::{charge_host_bytes, shared};
use crate::js::{self, Own};

const HANDLE_BITS: u32 = 20;
/// The most parts a stream key has.
const MAX_KEY_PARTS: usize = 64;
/// The most weights `weighted` takes and the most items `shuffle` permutes: refused before the
/// host allocates anything for them (script-sandbox.md 4.3).
const MAX_ITEMS: usize = 1 << 24;

fn from_problem(p: Problem) -> ScriptError {
    let mut e = ScriptError::new(&p.code, p.message, ErrorPhase::Run);
    e.detail.extra = p.detail;
    e
}

fn handle(call: &CallState, index: usize) -> f64 {
    let h = (call.serial << HANDLE_BITS) | u64::try_from(index).unwrap_or(0);
    #[allow(clippy::cast_precision_loss)] // serial < 2^33, so h < 2^53: exact
    let x = h as f64;
    x
}

/// The generator a handle names, in this call's tick.
fn stream<'w>(
    call: &CallState,
    world: &'w mut bevy_ecs::prelude::World,
    h: &Value<'_>,
) -> Result<&'w mut Pcg32, ScriptError> {
    let expired = || {
        ScriptError::new(
            "rng.stream_expired",
            "This random stream belonged to an earlier call; ask ctx for it again.",
            ErrorPhase::Run,
        )
    };
    let raw = h
        .as_number()
        .filter(|x| x.fract() == 0.0 && *x >= 0.0 && *x < 9.007_199_254_740_992e15)
        .ok_or_else(expired)?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked above
    let raw = raw as u64;
    if raw >> HANDLE_BITS != call.serial {
        return Err(expired());
    }
    let index = usize::try_from(raw & ((1 << HANDLE_BITS) - 1)).unwrap_or(usize::MAX);
    let key = *call.rng.get(index).ok_or_else(expired)?;
    world
        .resource_mut::<RngTable>()
        .into_inner()
        .stream(key)
        .map_err(from_problem)
}

/// An owned key part.
enum Part {
    Int(i64),
    Str(String),
    Entity(EntityId),
}

fn key_invalid(index: usize, what: &str) -> ScriptError {
    ScriptError::new(
        "rng.key_invalid",
        format!("Key part {index} ({what}) is not a key part: integers within ±(2^53 - 1), strings, or ctx.part.entity(e)."),
        ErrorPhase::Run,
    )
    .with("index", json!(index))
}

fn parts<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> Result<Vec<Part>, ScriptError> {
    let len = js::dense_array_len(ctx, v, MAX_KEY_PARTS)
        .map_err(|_| key_invalid(0, "not a list of at most 64 parts"))?;
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        let x = match js::own(ctx, v, &i.to_string()) {
            Own::Data(x) => x,
            _ => return Err(key_invalid(i, "a getter")),
        };
        if let Some(n) = x.as_number() {
            if n.fract() != 0.0 || n.abs() > 9_007_199_254_740_991.0 {
                return Err(key_invalid(i, &crate::error::num(n).to_string()));
            }
            #[allow(clippy::cast_possible_truncation)] // integral and within ±2^53
            out.push(Part::Int(n as i64));
        } else if let Some(t) = js::text(&x) {
            out.push(Part::Str(t.map_err(|()| {
                key_invalid(i, "a string with a lone surrogate")
            })?));
        } else {
            let id = match js::own(ctx, &x, "__pocketEntityPart") {
                Own::Data(e) => e.as_number().and_then(|e| EntityId::from_f64(e).ok()),
                _ => None,
            };
            out.push(Part::Entity(id.ok_or_else(|| {
                key_invalid(i, "not a number, a string or an entity part")
            })?));
        }
    }
    Ok(out)
}

fn borrowed(parts: &[Part]) -> Vec<KeyPart<'_>> {
    parts
        .iter()
        .map(|p| match p {
            Part::Int(i) => KeyPart::Int(*i),
            Part::Str(s) => KeyPart::Str(s),
            Part::Entity(e) => KeyPart::Entity(*e),
        })
        .collect()
}

fn open<'js>(
    ctx: &Ctx<'js>,
    key: impl FnOnce(&str, &RngTable) -> Result<StreamKey, Problem>,
) -> NResult<'js> {
    let h = with_call(ctx, "a random stream", |call, world| {
        let k = key(&call.key, world.resource::<RngTable>()).map_err(from_problem)?;
        call.rng.push(k);
        Ok(handle(call, call.rng.len() - 1))
    })?;
    Ok(Value::new_number(ctx.clone(), h))
}

pub fn system<'js>(ctx: &Ctx<'js>, _args: &[Value<'js>]) -> NResult<'js> {
    open(ctx, |sys, table| table.key(StreamKind::System(sys)))
}

pub fn entity<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let e = arg(ctx, args, 0);
    let id = e
        .as_number()
        .and_then(|x| EntityId::from_f64(x).ok())
        .ok_or_else(|| key_invalid(0, "not an entity id"))?;
    open(ctx, move |sys, table| {
        table.key(StreamKind::Entity(sys, id))
    })
}

pub fn named<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let p = parts(ctx, &arg(ctx, args, 0))?;
    let b = borrowed(&p);
    open(ctx, |sys, table| table.key(StreamKind::Named(sys, &b)))
}

pub fn timeless<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let p = parts(ctx, &arg(ctx, args, 0))?;
    let b = borrowed(&p);
    open(ctx, |_sys, table| table.key(StreamKind::Timeless(&b)))
}

fn draw<'js, R>(
    ctx: &Ctx<'js>,
    args: &[Value<'js>],
    f: impl FnOnce(&mut Pcg32) -> Result<R, ScriptError>,
) -> Result<R, Thrown> {
    let h = arg(ctx, args, 0);
    Ok(with_call(ctx, "a random stream", |call, world| {
        f(stream(call, world, &h)?)
    })?)
}

fn number(v: &Value<'_>) -> f64 {
    v.as_number().unwrap_or(f64::NAN)
}

fn integer(v: &Value<'_>, op: &str) -> Result<i64, ScriptError> {
    let x = number(v);
    if x.fract() != 0.0 || !x.is_finite() || x.abs() > 9_007_199_254_740_991.0 {
        return Err(ScriptError::new(
            "rng.bound_invalid",
            format!("{op}: the bounds must be whole numbers within ±(2^53 - 1)."),
            ErrorPhase::Run,
        ));
    }
    #[allow(clippy::cast_possible_truncation)] // integral and within ±2^53
    Ok(x as i64)
}

pub fn next<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let x = draw(ctx, args, |g| Ok(g.next_f64()))?;
    Ok(Value::new_number(ctx.clone(), x))
}

pub fn int<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let lo = integer(&arg(ctx, args, 1), "int")?;
    let hi = integer(&arg(ctx, args, 2), "int")?;
    let x = draw(ctx, args, |g| g.int(lo, hi).map_err(from_problem))?;
    #[allow(clippy::cast_precision_loss)] // within ±2^53
    Ok(Value::new_number(ctx.clone(), x as f64))
}

pub fn range<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let (lo, hi) = (number(&arg(ctx, args, 1)), number(&arg(ctx, args, 2)));
    let x = draw(ctx, args, |g| g.range(lo, hi).map_err(from_problem))?;
    Ok(Value::new_number(ctx.clone(), x))
}

pub fn chance<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let p = number(&arg(ctx, args, 1));
    let b = draw(ctx, args, |g| g.chance(p).map_err(from_problem))?;
    Ok(Value::new_bool(ctx.clone(), b))
}

fn length(v: &Value<'_>) -> usize {
    let x = number(v);
    if x.is_finite() && x >= 0.0 && x.fract() == 0.0 && x < 4_294_967_296.0 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // checked above
        let n = x as u32;
        usize::try_from(n).unwrap_or(0)
    } else {
        0
    }
}

pub fn pick<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let n = length(&arg(ctx, args, 1));
    let i = draw(ctx, args, |g| g.pick(n).map_err(from_problem))?;
    Ok(Value::new_number(
        ctx.clone(),
        f64::from(u32::try_from(i).unwrap_or(0)),
    ))
}

fn too_many(op: &str, n: f64) -> ScriptError {
    ScriptError::new(
        "rng.bound_invalid",
        format!(
            "{op} takes at most {MAX_ITEMS} items; got {}.",
            crate::error::num(n)
        ),
        ErrorPhase::Run,
    )
}

/// `shuffle`'s permutation of `items.length` indices. The length is a script value, so it is
/// bounded and charged to the call before the host allocates; the permutation is copied into
/// QuickJS-ng's memory, where `memory_bytes` counts it.
pub fn shuffle<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let raw = number(&arg(ctx, args, 1));
    let n = length(&arg(ctx, args, 1));
    if n > MAX_ITEMS {
        return Err(too_many("shuffle", raw).into());
    }
    // The host's permutation and the script's copy of it.
    charge_host_bytes(&shared(ctx), 16 * n as u64, "shuffle")?;
    let perm = draw(ctx, args, |g| {
        let mut idx: Vec<f64> = (0..n)
            .map(|i| f64::from(u32::try_from(i).unwrap_or(0)))
            .collect();
        g.shuffle(&mut idx);
        Ok(idx)
    })?;
    Ok(TypedArray::<f64>::new_copy(ctx.clone(), &perm)?.into_value())
}

pub fn weighted<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let w = arg(ctx, args, 1);
    let len = match js::dense_array_len(ctx, &w, MAX_ITEMS) {
        Ok(n) => n,
        Err(js::NotDense::TooLong(n)) => return Err(too_many("weighted", n).into()),
        Err(js::NotDense::Shape) => 0,
    };
    charge_host_bytes(&shared(ctx), 8 * len as u64, "weighted")?;
    let mut weights = Vec::with_capacity(len);
    for i in 0..len {
        weights.push(match js::own(ctx, &w, &i.to_string()) {
            Own::Data(x) => number(&x),
            _ => f64::NAN,
        });
    }
    let i = draw(ctx, args, |g| g.weighted(&weights).map_err(from_problem))?;
    Ok(Value::new_number(
        ctx.clone(),
        f64::from(u32::try_from(i).unwrap_or(0)),
    ))
}

pub fn normal<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let (mean, sd) = (number(&arg(ctx, args, 1)), number(&arg(ctx, args, 2)));
    let x = draw(ctx, args, |g| Ok(g.normal(mean, sd)))?;
    Ok(Value::new_number(ctx.clone(), x))
}

pub fn fill<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let out = arg(ctx, args, 1);
    let Some(array) = TypedArray::<f64>::from_value(out).ok() else {
        return Err(ScriptError::new(
            "rng.bound_invalid",
            "fill takes a Float64Array.",
            ErrorPhase::Run,
        )
        .into());
    };
    let n = array.len();
    charge_host_bytes(&shared(ctx), 8 * n as u64, "fill")?;
    let values = draw(ctx, args, |g| {
        let mut buf = vec![0.0; n];
        g.fill(&mut buf);
        Ok(buf)
    })?;
    // SAFETY: no JavaScript runs while the buffer is written; a detached array has no bytes.
    if let Some(raw) = array.as_raw() {
        let len = raw.len().min(values.len() * 8);
        let dst = unsafe { std::slice::from_raw_parts_mut(raw.as_ptr().cast::<u8>(), len) };
        for (chunk, v) in dst.as_chunks_mut::<8>().0.iter_mut().zip(&values) {
            chunk.copy_from_slice(&v.to_ne_bytes());
        }
    }
    Ok(Value::new_undefined(ctx.clone()))
}

/// `Math.random()`: the system stream's `next()`, only inside a system.
pub fn math_random<'js>(ctx: &Ctx<'js>, _args: &[Value<'js>]) -> NResult<'js> {
    let sh = shared(ctx);
    if sh.call.borrow().is_none() {
        return Err(ScriptError::new(
            "script.random_outside_system",
            "Math.random() draws from a system's stream; it works only inside a system's run.",
            ErrorPhase::Run,
        )
        .into());
    }
    drop(sh);
    let x = with_call(ctx, "Math.random", |call, world| {
        let key = call.key.clone();
        world
            .resource_mut::<RngTable>()
            .into_inner()
            .system(&key)
            .map(Pcg32::next_f64)
            .map_err(from_problem)
    })?;
    Ok(Value::new_number(ctx.clone(), x))
}
