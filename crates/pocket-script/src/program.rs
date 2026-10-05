//! Programs (docs/spec/script-host.md 8.3 and 9): `instantiate` makes a context, locks it down,
//! evaluates the modules under the load budget and reads the game; `run_system` runs one system as
//! a transaction under its budget and depth limit.

use std::rc::Rc;
use std::sync::Arc;

use bevy_ecs::prelude::World;
use pocket_sim::event::{self, EventKind, NewEvent};
use pocket_sim::registry::ComponentSchema;
use pocket_sim::sim::{
    TickOutput, begin_invocation, commit_invocation, rollback_invocation, system_failed,
};
use pocket_sim::{EntityId, PlainData, RunCondition, SimClock, SystemKey, Tick, TickPhase};
use rquickjs::context::intrinsic::{
    Eval, Json, MapSet, Promise, Proxy, RegExp, RegExpCompiler, TypedArrays,
};
use rquickjs::function::This;
use rquickjs::promise::PromiseState;
use rquickjs::{Context, Ctx, Function, Module, Object, Persistent, Value};

use crate::call::{CallCounts, CallState, apply};
use crate::caught::error_from_value;
use crate::define;
use crate::error::{ErrorPhase, ScriptError, SourceLocation};
use crate::host::{
    HostData, Maps, ScriptHost, begin_budget, discard_jobs, drain_jobs, end_budget, jobs_pending,
    out_of_memory, reset_oom,
};
use crate::loader::PRELUDE_MAP;
use crate::natives::Thrown;
use crate::natives::query::{Spec, prepare};
use crate::sandbox;
use crate::source::{Bundle, CompiledSet};
use crate::sourcemap::LineMap;

/// A system or an executor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemKind {
    System,
    Executor,
}

/// What a program says about one of its systems.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemInfo {
    pub name: String,
    pub key: SystemKey,
    pub when: RunCondition,
    pub doc: String,
    pub kind: SystemKind,
    pub defined_at: Option<SourceLocation>,
}

/// The steps left in a tick for all its script calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickBudget {
    pub steps_left: u64,
}

/// The tick a call runs in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TickInfo {
    pub tick: Tick,
    pub dt: f64,
    pub time: f64,
}

impl TickInfo {
    /// The running tick of a world.
    pub fn of(world: &World) -> TickInfo {
        let c = *world.resource::<SimClock>();
        TickInfo {
            tick: c.tick,
            dt: c.dt(),
            time: c.time(),
        }
    }
}

/// What one system call did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SystemStats {
    /// Steps it used, exact (script-sandbox.md 4.1).
    pub steps: u64,
    pub host_calls: u32,
    pub rows: u32,
    pub cells_written: u32,
    pub commands: u32,
    pub events: u32,
}

/// How a system call ended (script-sandbox.md 4.3 and 5.4).
#[derive(Clone, Debug, PartialEq)]
pub enum SystemOutcome {
    Ok(SystemStats),
    /// Its effects were discarded; the tick goes on.
    Failed {
        error: ScriptError,
        stats: SystemStats,
    },
    /// Platform-dependent (memory, stack) or an engine bug: the world is poisoned.
    Fault {
        error: ScriptError,
    },
}

struct SystemEntry {
    queries: Vec<(String, Spec)>,
    events: Vec<String>,
    def: Persistent<Value<'static>>,
    run: Persistent<Function<'static>>,
}

/// A compiled bundle instantiated in a context of a host: its systems and project components.
/// It holds no game state.
pub struct Program {
    infos: Vec<SystemInfo>,
    entries: Vec<SystemEntry>,
    components: Vec<Arc<ComponentSchema>>,
    set: Arc<CompiledSet>,
    maps: Rc<Maps>,
    make_context: Persistent<Function<'static>>,
    wrap_query: Persistent<Function<'static>>,
    /// The lockdown's freeze roots, `[name, value]` pairs: what `verify_lockdown` walks from.
    roots: Persistent<rquickjs::Array<'static>>,
    ctx_key: usize,
    shared: Rc<crate::host::Shared>,
    /// Compiled with the debugger's trace handler set (docs/spec/debugger.md 3).
    instrumented: bool,
    // Last: the context outlives the values above.
    ctx: Context,
}

impl Drop for Program {
    fn drop(&mut self) {
        self.shared.intrinsics.borrow_mut().remove(&self.ctx_key);
        self.shared.thrown.borrow_mut().clear();
    }
}

/// `script.out_of_memory`: memory ran out somewhere in a call, whether or not its error reached
/// the host (P2).
fn out_of_memory_error(phase: ErrorPhase) -> ScriptError {
    ScriptError::new(
        "script.out_of_memory",
        "The script ran out of memory.",
        phase,
    )
}

fn maps_of(set: &CompiledSet) -> Maps {
    let mut maps = Maps::default();
    maps.by_module.insert(
        crate::resolve::PRELUDE.to_owned(),
        LineMap::parse(PRELUDE_MAP),
    );
    for m in &set.modules {
        maps.by_module
            .insert(m.path.clone(), LineMap::parse(&m.map));
    }
    maps
}

/// Imports a module and runs the job queue until its promise settles.
fn import<'js>(ctx: &Ctx<'js>, specifier: &str) -> Result<Object<'js>, Value<'js>> {
    let promise = Module::import(ctx, specifier.to_owned()).map_err(|_| ctx.catch())?;
    loop {
        match promise.state() {
            PromiseState::Resolved => {
                return promise
                    .result::<Object>()
                    .map_or_else(|| Err(ctx.catch()), |r| r.map_err(|_| ctx.catch()));
            }
            PromiseState::Rejected => {
                return Err(match promise.result::<Value>() {
                    Some(Err(_)) | None => ctx.catch(),
                    Some(Ok(v)) => v,
                });
            }
            PromiseState::Pending => {
                drain_jobs(ctx)?;
                if promise.state() == PromiseState::Pending && !jobs_pending(ctx) {
                    return Err(Value::new_undefined(ctx.clone()));
                }
            }
        }
    }
}

impl ScriptHost {
    /// Instantiates a compiled bundle (script-host.md 8.3): a new context, the lockdown, the
    /// modules evaluated under `load_steps`, the game read and checked against the world's
    /// components. Nothing reaches the world; any failure drops the new context.
    pub fn instantiate(
        &self,
        set: &CompiledSet,
        world: &World,
    ) -> Result<Program, Vec<ScriptError>> {
        let sh = &self.shared;
        let ctx = Context::custom::<(
            Eval,
            RegExpCompiler,
            RegExp,
            Json,
            MapSet,
            TypedArrays,
            Promise,
            Proxy,
        )>(&self.rt)
        .map_err(|e| {
            vec![ScriptError::new(
                "script.out_of_memory",
                format!("No context: {e}."),
                ErrorPhase::Load,
            )]
        })?;
        let set = Arc::new(set.clone());
        let maps = Rc::new(maps_of(&set));
        let hook = sh.debug.borrow().clone();
        let instrumented = hook.as_ref().is_some_and(|h| h.instrument());
        *sh.loading.borrow_mut() = Some(Rc::new((*set).clone()));
        *sh.maps.borrow_mut() = Some(maps.clone());
        *sh.load_error.borrow_mut() = None;
        *sh.panic.borrow_mut() = None;
        sh.rejections.borrow_mut().clear();
        let result = ctx.with(|ctx| -> Result<_, ScriptError> {
            let _ = ctx.store_userdata(HostData(sh.clone()));
            reset_oom(&ctx, sh);
            let ctx_key = crate::js::raw(&ctx) as usize;
            let locked = sandbox::lockdown(&ctx).map_err(|e| {
                let v = ctx.catch();
                let mut err = error_from_value(&ctx, sh, &v, ErrorPhase::Load);
                err.message = format!("The lockdown failed: {e}; {}", err.message);
                err
            })?;
            if out_of_memory(sh) {
                return Err(out_of_memory_error(ErrorPhase::Load));
            }
            sh.intrinsics
                .borrow_mut()
                .insert(ctx_key, Rc::new(locked.intrinsics));
            if sh.iterators.borrow().is_empty() {
                *sh.iterators.borrow_mut() = locked.iterators;
            }
            let roots = Persistent::save(&ctx, locked.roots);
            // The debugger's instrumentation (docs/spec/debugger.md 3): modules compiled while the
            // trace handler is set get a trace call at every statement; it stays set for them.
            if instrumented {
                unsafe { crate::debug::install(crate::js::raw(&ctx), sh, true) };
                sh.instrumenting.set(true);
            }
            let steps = begin_budget(&ctx, sh, sh.limits.load_steps);
            let loaded = import(&ctx, &set.entry)
                .and_then(|ns| Ok((ns, import(&ctx, crate::resolve::PRELUDE)?)));
            end_budget(&ctx, sh, steps);
            sh.instrumenting.set(false);
            let (ns, prelude) = match loaded {
                Ok(x) if !out_of_memory(sh) => x,
                Ok(_) => return Err(out_of_memory_error(ErrorPhase::Load)),
                Err(v) => {
                    if let Some(e) = sh.load_error.borrow_mut().take()
                        && !out_of_memory(sh)
                    {
                        return Err(e);
                    }
                    return Err(error_from_value(&ctx, sh, &v, ErrorPhase::Load));
                }
            };
            if let Some(r) = sh.rejections.borrow_mut().drain(..).next() {
                return Err(r.error);
            }
            let default = match crate::js::own(&ctx, &ns.clone().into_value(), "default") {
                crate::js::Own::Data(v) => v,
                _ => Value::new_undefined(ctx.clone()),
            };
            let game = define::game(&ctx, &default, world)?;
            let helper = |name: &str| -> Result<Persistent<Function<'static>>, ScriptError> {
                let f: Function = prelude.get(name).map_err(|_| {
                    ScriptError::new(
                        "sim.internal",
                        format!("The prelude lacks {name}."),
                        ErrorPhase::Load,
                    )
                })?;
                Ok(Persistent::save(&ctx, f))
            };
            let make_context = helper("__context")?;
            let wrap_query = helper("__query")?;
            let mut infos = Vec::new();
            let mut entries = Vec::new();
            for s in game.systems {
                let defined_at = set.modules.iter().find_map(|m| {
                    let site = m.systems.iter().find(|x| x.name == s.name)?;
                    Some(SourceLocation {
                        file: m.path.clone(),
                        line: site.line,
                        column: site.column,
                    })
                });
                infos.push(SystemInfo {
                    name: s.name,
                    key: s.key,
                    when: s.when,
                    doc: s.doc,
                    kind: SystemKind::System,
                    defined_at,
                });
                entries.push(SystemEntry {
                    queries: s.queries,
                    events: s.events,
                    def: Persistent::save(&ctx, s.def),
                    run: Persistent::save(&ctx, s.run),
                });
            }
            Ok((
                infos,
                entries,
                game.components,
                make_context,
                wrap_query,
                roots,
                ctx_key,
            ))
        });
        if result.is_err() {
            // The failed context's intrinsics and the jobs its modules queued go with it.
            ctx.with(|c| {
                discard_jobs(&c);
                sh.intrinsics
                    .borrow_mut()
                    .remove(&(crate::js::raw(&c) as usize));
            });
            sh.thrown.borrow_mut().clear();
        }
        *sh.loading.borrow_mut() = None;
        *sh.maps.borrow_mut() = None;
        sh.rejections.borrow_mut().clear();
        let (infos, entries, components, make_context, wrap_query, roots, ctx_key) =
            result.map_err(|e| vec![e])?;
        if let Some(h) = hook {
            h.loaded(&set, instrumented);
        }
        Ok(Program {
            infos,
            entries,
            components,
            set,
            maps,
            make_context,
            wrap_query,
            roots,
            ctx_key,
            shared: sh.clone(),
            instrumented,
            ctx,
        })
    }
}

impl Program {
    /// The systems in run order.
    pub fn systems(&self) -> &[SystemInfo] {
        &self.infos
    }

    /// The project components it declares, in registration order.
    pub fn project_components(&self) -> &[Arc<ComponentSchema>] {
        &self.components
    }

    /// The bundle it was instantiated from (versions.md 3.2).
    pub fn bundle(&self) -> &Bundle {
        &self.set.bundle
    }

    /// The compiled set it was instantiated from: what a replay embeds.
    pub fn compiled(&self) -> &CompiledSet {
        &self.set
    }

    /// The compiled set, shared.
    pub fn compiled_arc(&self) -> &Arc<CompiledSet> {
        &self.set
    }

    /// Whether it was compiled with the debugger's trace handler (docs/spec/debugger.md 3).
    pub fn instrumented(&self) -> bool {
        self.instrumented
    }

    /// Runs system `index` as one invocation (script-host.md 5.4): its queries prepared, `run`
    /// called under the smaller of `steps_per_system` and the tick's steps left, the job queue
    /// drained under the same budget, then the column write-backs and commands applied together,
    /// or none of them. A failure emits `script.failed {system, code}` and is reported to the
    /// tick's output; a fault is returned for the caller to poison the world.
    pub fn run_system(
        &self,
        host: &ScriptHost,
        world: &mut World,
        index: usize,
        tick: &TickInfo,
        budget: &mut TickBudget,
    ) -> SystemOutcome {
        let sh = &host.shared;
        let info = &self.infos[index];
        let entry = &self.entries[index];
        let inv = begin_invocation(world, &info.key);
        let serial = sh.serial.get() + 1;
        sh.serial.set(serial);
        *sh.call.borrow_mut() = Some(CallState::new(world, tick.tick, &info.name, serial));
        *sh.maps.borrow_mut() = Some(self.maps.clone());
        *sh.panic.borrow_mut() = None;
        sh.rejections.borrow_mut().clear();
        let allowed = u32::try_from(budget.steps_left.min(u64::from(sh.limits.steps_per_system)))
            .unwrap_or(u32::MAX);
        let mut used = 0;
        let hook = sh.debug.borrow().clone();
        let call_info = crate::debug::CallInfo {
            tick: tick.tick.0,
            system: &info.name,
            index,
            serial,
        };
        let validated = self.ctx.with(|ctx| {
            let steps = begin_budget(&ctx, sh, allowed);
            if let Some(h) = &hook {
                h.call_begin(&call_info);
            }
            let ran = self.call_run(&ctx, info, entry, tick);
            used = end_budget(&ctx, sh, steps);
            if let Some(h) = &hook {
                h.call_returned(crate::js::raw(&ctx), &call_info);
            }
            let mut error = match ran {
                Ok(()) => None,
                Err(v) => Some(error_from_value(&ctx, sh, &v, ErrorPhase::Run)),
            };
            if error.is_none() {
                error = sh.rejections.borrow_mut().drain(..).next().map(|r| r.error);
            }
            if error.is_none() && jobs_pending(&ctx) {
                error = Some(ScriptError::new(
                    "script.async_work",
                    "Jobs were still pending after the call; scripts may not leave work behind.",
                    ErrorPhase::Run,
                ));
            }
            // P2: memory that ran out anywhere in the call faults it, even when a builtin
            // swallowed the error and the script went on (a panic stays a panic).
            if out_of_memory(sh) && error.as_ref().is_none_or(|e| e.code != "sim.internal") {
                error = Some(out_of_memory_error(ErrorPhase::Run));
            }
            // A native that panicked faults the call however the script went on.
            if let Some(message) = sh.panic.borrow().clone()
                && error.as_ref().is_none_or(|e| e.code != "sim.internal")
            {
                error = Some(ScriptError::new(
                    "sim.internal",
                    format!("A native function of the script host panicked: {message}."),
                    ErrorPhase::Run,
                ));
            }
            if error.is_some() {
                // P8: the jobs a failed call queued must not run in the next call.
                discard_jobs(&ctx);
            }
            let call = sh.call.borrow_mut().take();
            let Some(call) = call else {
                return Err(ScriptError::new(
                    "sim.internal",
                    "The call state vanished.",
                    ErrorPhase::Run,
                ));
            };
            if let Some(e) = error {
                return Err(e);
            }
            // SAFETY: JavaScript has stopped; this is the only use of the world now.
            let w: &World = unsafe { &*call.world };
            let cells = crate::natives::query::changed_cells(&ctx, &call, w)?;
            Ok((cells, call))
        });
        let mut stats = SystemStats {
            steps: used,
            ..SystemStats::default()
        };
        budget.steps_left = budget.steps_left.saturating_sub(used);
        let result = validated.and_then(|(cells, call)| {
            let counts: CallCounts = call.counts;
            stats.host_calls = counts.host_calls;
            stats.rows = counts.rows;
            stats.events = counts.events;
            stats.cells_written = u32::try_from(cells.len()).unwrap_or(u32::MAX);
            stats.commands = u32::try_from(call.commands.len()).unwrap_or(u32::MAX);
            apply(world, &cells, call.commands)
        });
        *sh.call.borrow_mut() = None;
        *sh.maps.borrow_mut() = None;
        let outcome = match result {
            Ok(()) => {
                commit_invocation(world, inv);
                SystemOutcome::Ok(stats)
            }
            Err(mut error) => {
                rollback_invocation(world, inv);
                error.detail.tick = Some(tick.tick.0);
                error.detail.system = Some(info.name.clone());
                // A failure no single line caused (a write-back) is placed at the system's run.
                if error.detail.location.is_none() {
                    error.detail.location = info.defined_at.clone();
                }
                if error.is_fault() {
                    SystemOutcome::Fault { error }
                } else {
                    report_failure(world, tick.tick, &info.key, &info.name, &error);
                    SystemOutcome::Failed { error, stats }
                }
            }
        };
        if let Some(h) = &hook {
            h.call_end(&call_info, &outcome);
        }
        outcome
    }

    fn call_run<'js>(
        &self,
        ctx: &Ctx<'js>,
        info: &SystemInfo,
        entry: &SystemEntry,
        tick: &TickInfo,
    ) -> Result<(), Value<'js>> {
        let caught = |_| ctx.catch();
        let call_info = Object::new(ctx.clone()).map_err(caught)?;
        call_info.set("tick", tick.tick.to_f64()).map_err(caught)?;
        call_info.set("dt", tick.dt).map_err(caught)?;
        call_info.set("time", tick.time).map_err(caught)?;
        call_info
            .set("system", info.name.as_str())
            .map_err(caught)?;
        call_info
            .set("events", entry.events.clone())
            .map_err(caught)?;
        let make: Function = self.make_context.clone().restore(ctx).map_err(caught)?;
        let context: Value = make.call((call_info,)).map_err(caught)?;
        let results = Object::new(ctx.clone()).map_err(caught)?;
        if !entry.queries.is_empty() {
            let wrap: Function = self.wrap_query.clone().restore(ctx).map_err(caught)?;
            for (name, spec) in &entry.queries {
                let raw = match prepare(ctx, spec) {
                    Ok(v) => v,
                    Err(Thrown::Script(e)) => {
                        let _ = crate::caught::throw(ctx, &e);
                        return Err(ctx.catch());
                    }
                    Err(Thrown::Js(_)) => return Err(ctx.catch()),
                };
                let q: Value = wrap.call((raw,)).map_err(caught)?;
                results.set(name.as_str(), q).map_err(caught)?;
            }
        }
        let results = crate::convert::freeze_new(ctx, results).map_err(caught)?;
        let def: Value = entry.def.clone().restore(ctx).map_err(caught)?;
        let run: Function = entry.run.clone().restore(ctx).map_err(caught)?;
        run.call::<_, ()>((This(def), context, results))
            .map_err(caught)?;
        drain_jobs(ctx)
    }
}

/// Reports a failed system: `sim.system_failed` in the tick's output and the hidden event
/// `script.failed {system, code}`, whose data leaves the location out (it is world state).
pub fn report_failure(
    world: &mut World,
    tick: Tick,
    key: &SystemKey,
    name: &str,
    error: &ScriptError,
) {
    let entity = error.detail.entity.and_then(|x| EntityId::from_f64(x).ok());
    let problem = system_failed(tick, TickPhase::Update, key, entity, &error.to_problem());
    world.resource_mut::<TickOutput>().report(problem);
    let data = PlainData::object(vec![
        ("system".into(), PlainData::String(name.to_owned())),
        ("code".into(), PlainData::String(error.code.clone())),
    ])
    .unwrap_or_default();
    if let Ok(kind) = EventKind::new("script.failed") {
        event::emit(world, NewEvent::new(kind).data(data));
    }
}

impl Program {
    /// Evaluates a JavaScript expression in the program's context outside any system, under
    /// `steps_per_call`, and returns its JSON (tests and tools; not a script call kind of
    /// script-sandbox.md 4.4: no world is reachable).
    pub fn eval_json(
        &self,
        host: &ScriptHost,
        source: &str,
    ) -> Result<serde_json::Value, ScriptError> {
        let sh = &host.shared;
        *sh.maps.borrow_mut() = Some(self.maps.clone());
        let r = self.ctx.with(|ctx| {
            let steps = begin_budget(&ctx, sh, sh.limits.steps_per_call);
            let v: Result<Value, _> = ctx.eval(source.to_owned());
            let out = match v {
                Ok(v) => ctx.json_stringify(v).map_err(|_| ctx.catch()).map(|s| {
                    s.and_then(|s| s.to_string().ok())
                        .unwrap_or_else(|| "null".into())
                }),
                Err(_) => Err(ctx.catch()),
            };
            end_budget(&ctx, sh, steps);
            // Nothing an evaluation queued may run later, in a system's call (P8).
            discard_jobs(&ctx);
            match out {
                Ok(_) if out_of_memory(sh) => Err(out_of_memory_error(ErrorPhase::Run)),
                Ok(text) => Ok(text),
                Err(v) => Err(error_from_value(&ctx, sh, &v, ErrorPhase::Run)),
            }
        });
        *sh.maps.borrow_mut() = None;
        let text = r?;
        Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
    }

    /// The lockdown's verification walk (script-sandbox.md 2.3, step 5): extensible objects,
    /// writable and configurable properties, and setters reachable from the roots the lockdown
    /// froze from (`globalThis` and the hidden intrinsics), and the roots' names.
    pub fn verify_lockdown(&self, host: &ScriptHost) -> Result<serde_json::Value, ScriptError> {
        let sh = &host.shared;
        let text = self.ctx.with(|ctx| {
            let steps = begin_budget(&ctx, sh, sh.limits.steps_per_call);
            let out = (|| -> Result<String, Value> {
                let verify: Function = ctx.eval(crate::sandbox::VERIFY).map_err(|_| ctx.catch())?;
                let roots = self.roots.clone().restore(&ctx).map_err(|_| ctx.catch())?;
                verify.call::<_, String>((roots,)).map_err(|_| ctx.catch())
            })();
            end_budget(&ctx, sh, steps);
            out.map_err(|v| error_from_value(&ctx, sh, &v, ErrorPhase::Run))
        })?;
        Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
    }
}
