//! The natives of `pocket:host` (docs/spec/script-host.md 8.3), which only the prelude and the
//! harden epilogue import. Every native validates its arguments before it borrows the world,
//! runs under `catch_unwind` (a panic becomes an uncatchable error and a fault, never an unwind
//! through QuickJS-ng's C frames; script-sandbox.md 4.3), and reaches the world through the call
//! state installed for the call (else `script.no_system`).

pub mod console;
pub mod events;
pub mod harden;
pub mod query;
pub mod rng;
pub mod world;

use std::panic::{AssertUnwindSafe, catch_unwind};

use bevy_ecs::prelude::World;
use rquickjs::function::Rest;
use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{Ctx, Exception, Function, Value};

use crate::call::CallState;
use crate::error::{ErrorPhase, ScriptError};
use crate::host::shared;
use crate::js;

/// Why a native stopped.
pub enum Thrown {
    /// A refusal: thrown at the calling line with its code.
    Script(ScriptError),
    /// An engine error rquickjs reports.
    Js(rquickjs::Error),
}

impl From<ScriptError> for Thrown {
    fn from(e: ScriptError) -> Thrown {
        Thrown::Script(e)
    }
}

impl From<rquickjs::Error> for Thrown {
    fn from(e: rquickjs::Error) -> Thrown {
        Thrown::Js(e)
    }
}

pub type NResult<'js> = Result<Value<'js>, Thrown>;

/// A native's body.
pub type Native = for<'js> fn(&Ctx<'js>, &[Value<'js>]) -> NResult<'js>;

/// The natives `pocket:host` exports.
const NATIVES: &[(&str, Native)] = &[
    ("harden", harden::harden),
    ("freeze", harden::freeze),
    ("query", query::query),
    ("single", world::single),
    ("exists", world::exists),
    ("has", world::has),
    ("get", world::get),
    ("set", world::set),
    ("insert", world::insert),
    ("remove", world::remove),
    ("despawn", world::despawn),
    ("spawn", world::spawn),
    ("emit", events::emit),
    ("events", events::events),
    ("intent", events::intent),
    ("rngSystem", rng::system),
    ("rngEntity", rng::entity),
    ("rngNamed", rng::named),
    ("rngTimeless", rng::timeless),
    ("rngNext", rng::next),
    ("rngInt", rng::int),
    ("rngRange", rng::range),
    ("rngChance", rng::chance),
    ("rngPick", rng::pick),
    ("rngShuffle", rng::shuffle),
    ("rngWeighted", rng::weighted),
    ("rngNormal", rng::normal),
    ("rngFill", rng::fill),
];

/// The native module `pocket:host`.
pub struct HostModule;

impl ModuleDef for HostModule {
    fn declare<'js>(decl: &Declarations<'js>) -> rquickjs::Result<()> {
        for (name, _) in NATIVES {
            decl.declare(*name)?;
        }
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> rquickjs::Result<()> {
        for (name, f) in NATIVES {
            exports.export(*name, function(ctx, name, *f)?)?;
        }
        Ok(())
    }
}

/// A JavaScript function running `f` under the natives' rules.
pub fn function<'js>(ctx: &Ctx<'js>, name: &str, f: Native) -> rquickjs::Result<Function<'js>> {
    Function::new(ctx.clone(), move |ctx: Ctx<'js>, args: Rest<Value<'js>>| {
        run(&ctx, f, &args.0)
    })?
    .with_name(name)
}

fn run<'js>(ctx: &Ctx<'js>, f: Native, args: &[Value<'js>]) -> rquickjs::Result<Value<'js>> {
    match catch_unwind(AssertUnwindSafe(|| f(ctx, args))) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(Thrown::Script(e))) => Err(crate::caught::throw(ctx, &e)),
        Ok(Err(Thrown::Js(e))) => Err(e),
        Err(panic) => {
            let message = panic_message(panic.as_ref());
            let sh = shared(ctx);
            sh.panic.borrow_mut().get_or_insert(message.clone());
            match Exception::from_message(ctx.clone(), &format!("native panicked: {message}")) {
                Ok(e) => {
                    let v = e.into_value();
                    js::make_uncatchable(ctx, &v);
                    Err(ctx.throw(v))
                }
                Err(e) => Err(e),
            }
        }
    }
}

/// The text of a panic's payload.
pub fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic without a message".to_owned()
    }
}

/// The argument `i`, or `undefined`.
pub fn arg<'js>(ctx: &Ctx<'js>, args: &[Value<'js>], i: usize) -> Value<'js> {
    args.get(i)
        .cloned()
        .unwrap_or_else(|| Value::new_undefined(ctx.clone()))
}

/// `script.no_system`: a native that needs a running system called outside one.
pub fn no_system(what: &str) -> ScriptError {
    ScriptError::new(
        "script.no_system",
        format!(
            "{what} works only inside a system's run; it was called while modules loaded or outside a tick."
        ),
        ErrorPhase::Run,
    )
}

/// Runs `f` with the call state and the world. No JavaScript may run inside `f`: the world is
/// borrowed from the caller of the system for the length of `f` alone.
pub fn with_call<R>(
    ctx: &Ctx<'_>,
    what: &str,
    f: impl FnOnce(&mut CallState, &mut World) -> Result<R, ScriptError>,
) -> Result<R, ScriptError> {
    let sh = shared(ctx);
    let mut guard = sh.call.borrow_mut();
    let call = guard.as_mut().ok_or_else(|| no_system(what))?;
    call.counts.host_calls = call.counts.host_calls.saturating_add(1);
    // SAFETY: the pointer is the `&mut World` `run_system` holds for the call; it is not used
    // there while JavaScript runs, and `f` runs no JavaScript, so this is the only reference.
    let world = unsafe { &mut *call.world };
    f(call, world)
}
