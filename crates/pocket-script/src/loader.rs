//! The resolver and loader over a `CompiledSet` (docs/spec/script-host.md 8.1): module names inside
//! QuickJS-ng are module paths, so a stack frame names the TypeScript file; nothing touches the
//! file system. A failure is stored as a structured error, since rquickjs only reports that
//! loading failed, and nothing here may panic across QuickJS-ng's C frames.

use std::panic::{AssertUnwindSafe, catch_unwind};

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::{Ctx, Error, Module};

use crate::error::{ErrorPhase, ScriptError};
use crate::host::shared;
use crate::resolve::{HOST, PRELUDE, Resolved, resolve};

/// The prelude's JavaScript and source map, generated from `src/prelude/pocket.ts`.
pub const PRELUDE_JS: &str = include_str!("prelude/pocket.js");
pub const PRELUDE_MAP: &str = include_str!("prelude/pocket.js.map");

/// The loader of the set being instantiated (`Shared::loading`).
#[derive(Clone, Copy)]
pub struct SetLoader;

fn fail(ctx: &Ctx<'_>, e: ScriptError) -> Error {
    let message = e.message.clone();
    let sh = shared(ctx);
    let mut stored = sh.load_error.borrow_mut();
    if stored.is_none() {
        *stored = Some(e);
    }
    Error::new_loading_message("module", message)
}

impl Resolver for SetLoader {
    fn resolve<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        let r = catch_unwind(AssertUnwindSafe(|| {
            let sh = shared(ctx);
            let set = sh.loading.borrow().clone();
            let Some(set) = set else {
                return Err(ScriptError::new(
                    "script.module_not_found",
                    format!(
                        "No module '{name}': modules load only while a program is instantiated."
                    ),
                    ErrorPhase::Load,
                ));
            };
            // The host imports the entry by its module path.
            if base.is_empty() && set.module(name).is_some() {
                return Ok(Resolved::Module(name.to_owned()));
            }
            resolve(base, name, set.modules.iter().map(|m| m.path.as_str()))
        }));
        match r {
            Ok(Ok(Resolved::Prelude)) => Ok(PRELUDE.to_owned()),
            Ok(Ok(Resolved::Host)) => Ok(HOST.to_owned()),
            Ok(Ok(Resolved::Module(p))) => Ok(p),
            Ok(Err(e)) => Err(fail(ctx, e)),
            Err(_) => Err(fail(
                ctx,
                ScriptError::new(
                    "sim.internal",
                    "The module resolver panicked.",
                    ErrorPhase::Load,
                ),
            )),
        }
    }
}

impl Loader for SetLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js, Declared>> {
        if name == HOST {
            return Module::declare_def::<crate::natives::HostModule, _>(ctx.clone(), name);
        }
        if name == PRELUDE {
            return Module::declare(ctx.clone(), name, PRELUDE_JS);
        }
        let sh = shared(ctx);
        let js = sh
            .loading
            .borrow()
            .as_ref()
            .and_then(|s| s.module(name).map(|m| m.js.clone()));
        match js {
            Some(js) => Module::declare(ctx.clone(), name, js),
            None => Err(fail(
                ctx,
                ScriptError::new(
                    "script.module_not_found",
                    format!("The compiled scripts hold no module {name}."),
                    ErrorPhase::Load,
                ),
            )),
        }
    }
}
