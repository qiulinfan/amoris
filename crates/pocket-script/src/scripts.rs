//! A world's scripts in the schedule (docs/spec/script-host.md 9; hot-update.md 5): the
//! `script.update` system in the `Update` phase runs the program's systems in order, each between
//! the step's hooks, and a swap at a boundary replaces the program whole or not at all.
//!
//! The host is not `Send`, and `Sim` keeps only `Send` systems, so the host and the program live
//! in the world as non-send data (`Scripts`), which the game thread that built them alone touches
//! (bevy_ecs panics on any access to non-send data from another thread). They are code, not state:
//! persistence declares them ignored, and a fork or a restore is a fresh `Sim` on its own thread
//! with `install` and `swap` of the same compiled set.

use std::panic::{AssertUnwindSafe, catch_unwind};

use bevy_ecs::prelude::World;
use pocket_contract::Problem;
use pocket_sim::registry::{ComponentOrigin, ComponentSchema};
use pocket_sim::{ComponentRegistry, ContentHash, RunCondition, Sim, SystemCtx, TickPhase};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::access::{ScriptAccess, register_project, set_project_schema};
use crate::error::{ErrorPhase, ScriptError};
use crate::host::{ScriptHost, ScriptLimits};
use crate::program::{Program, SystemOutcome, SystemStats, TickBudget, TickInfo};
use crate::source::CompiledSet;

/// The host and the installed program of one world, plus what the last tick's systems did.
pub struct Scripts {
    /// First, so it drops before the host whose runtime its context lives in.
    pub program: Option<Program>,
    /// The bundle the last swap replaced: what `scripts.revert` reinstalls.
    pub previous: Option<CompiledSet>,
    /// Per system run in the last tick: its name and outcome.
    pub last_tick: Vec<(String, SystemOutcome)>,
    pub host: ScriptHost,
}

/// Registers this crate's world data with persistence: the accessors and the host are code.
pub fn declare<R: pocket_sim::persisted::RegisterPersisted>(r: &mut R) {
    r.ignore::<ScriptAccess>("component accessors the engine and the script host register")
        .ignore::<Scripts>("the script host and its compiled program: code, rebuilt per world");
}

/// Installs a host with `limits` into `sim`'s world (no program yet) and the `script.update`
/// system into its schedule. The world must stay on this thread.
pub fn install(sim: &mut Sim, limits: ScriptLimits) -> Result<(), Problem> {
    let host = ScriptHost::new(limits).map_err(|e| e.to_problem())?;
    let world = sim.world_mut();
    world.get_resource_or_insert_with(ScriptAccess::default);
    world.insert_non_send(Scripts {
        host,
        program: None,
        previous: None,
        last_tick: Vec::new(),
    });
    sim.add_exclusive(
        "script.update",
        TickPhase::Update,
        RunCondition::Always,
        update,
    )
}

/// `script.update`: the program's systems due this tick, in order, under the tick's budget. A
/// panic in the host while a system runs (an engine bug outside any native, such as an accessor
/// reached while a declared query is prepared) faults that system with `sim.internal`; the
/// scripts go back into the world whatever happens.
fn update(world: &mut World, sys: &mut SystemCtx<'_>) {
    let Some(mut scripts) = world.remove_non_send::<Scripts>() else {
        return;
    };
    scripts.last_tick.clear();
    reinstrument(world, &mut scripts);
    if let Some(program) = &scripts.program {
        let tick = TickInfo::of(world);
        let mut budget = TickBudget {
            steps_left: scripts.host.limits().steps_per_tick,
        };
        for (i, info) in program.systems().iter().enumerate() {
            if !info.when.runs_on(tick.tick) {
                continue;
            }
            sys.hooks().system(&info.key, true);
            let ran = catch_unwind(AssertUnwindSafe(|| {
                program.run_system(&scripts.host, world, i, &tick, &mut budget)
            }));
            let outcome = ran.unwrap_or_else(|panic| {
                scripts.host.abandon_call();
                let message = crate::natives::panic_message(panic.as_ref());
                let mut error = ScriptError::new(
                    "sim.internal",
                    format!(
                        "The script host panicked while {} ran: {message}.",
                        info.name
                    ),
                    ErrorPhase::Run,
                );
                error.detail.tick = Some(tick.tick.0);
                error.detail.system = Some(info.name.clone());
                error.detail.location = info.defined_at.clone();
                SystemOutcome::Fault { error }
            });
            sys.hooks().system(&info.key, false);
            let fault = match &outcome {
                SystemOutcome::Fault { error } => Some(error.to_problem()),
                _ => None,
            };
            scripts.last_tick.push((info.name.clone(), outcome));
            if let Some(p) = fault {
                sys.fault(p);
                break;
            }
        }
    }
    world.insert_non_send(scripts);
}

/// Instantiates the installed program again when the debugger wants it instrumented and it is not,
/// or the other way round (docs/spec/debugger.md 3). No script of this tick has run yet and programs
/// hold no state, so this is a reload at the boundary; a failure (which a program that loaded once
/// should not meet) keeps the installed program.
fn reinstrument(world: &World, scripts: &mut Scripts) {
    let Some(hook) = scripts.host.debugger() else {
        return;
    };
    let Some(program) = &scripts.program else {
        return;
    };
    if hook.instrument() == program.instrumented() {
        return;
    }
    let set = program.compiled_arc().clone();
    if let Ok(p) = scripts.host.instantiate(&set, world) {
        scripts.program = Some(p);
    }
}

/// A project component's change across a swap (hot-update.md 6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaChange {
    Added { name: String, version: u32 },
    Unchanged { name: String },
}

/// The systems a swap added, removed and kept.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemsDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub kept: Vec<String>,
}

/// Whether a swap happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwapOutcome {
    Applied,
    /// The bundle hash equals the installed one and the swap was not forced.
    Unchanged,
}

/// What a swap did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapReport {
    pub outcome: SwapOutcome,
    pub bundle: ContentHash,
    pub previous: Option<ContentHash>,
    pub systems: SystemsDiff,
    pub components: Vec<SchemaChange>,
}

/// Whether two schemas have the same shape: names, types and order of the fields (docs and
/// defaults are not part of a component's fingerprint, hot-update.md 6).
fn same_shape(a: &ComponentSchema, b: &ComponentSchema) -> bool {
    a.fields.len() == b.fields.len()
        && a.fields
            .iter()
            .zip(b.fields.iter())
            .all(|(x, y)| x.name == y.name && x.ty == y.ty)
}

fn refused(code: &str, message: String, component: &str) -> ScriptError {
    ScriptError::new(code, message, ErrorPhase::Load).component(component)
}

/// Plans the project components' changes, or refuses (hot-update.md 6).
fn plan(world: &World, program: &Program) -> Result<Vec<SchemaChange>, ScriptError> {
    let registry = world.get_resource::<ComponentRegistry>();
    let mut out = Vec::new();
    for c in program.project_components() {
        match registry
            .and_then(|r| r.get(&c.name))
            .and_then(|e| e.script.clone().map(|s| (e.origin, s)))
        {
            None => out.push(SchemaChange::Added {
                name: c.name.to_string(),
                version: c.version,
            }),
            Some((ComponentOrigin::Engine, _)) => {
                return Err(refused(
                    "script.component_name",
                    format!("'{}' is an engine component.", c.name),
                    &c.name,
                ));
            }
            Some((ComponentOrigin::Project, live)) => {
                if c.version == live.version && same_shape(c, &live) {
                    out.push(SchemaChange::Unchanged {
                        name: c.name.to_string(),
                    });
                } else if c.version == live.version {
                    return Err(refused(
                        "version.unbumped",
                        format!(
                            "{}'s fields changed but its version is still {}; bump it and add a migrate step.",
                            c.name, c.version
                        ),
                        &c.name,
                    ));
                } else if c.version < live.version {
                    return Err(refused(
                        "version.downgrade",
                        format!(
                            "{} goes from version {} back to {}.",
                            c.name, live.version, c.version
                        ),
                        &c.name,
                    ));
                } else {
                    // Slice 1: the live world's migration (versions.md 8.5) is not built yet.
                    return Err(refused(
                        "migrate.missing_step",
                        format!("{} goes from version {} to {}; migrating the live world is not available yet, so restart instead.", c.name, live.version, c.version),
                        &c.name,
                    )
                    .with("from", json!(live.version))
                    .with("to", json!(c.version)));
                }
            }
        }
    }
    if let Some(r) = registry {
        for e in r
            .entries()
            .iter()
            .filter(|e| e.origin == ComponentOrigin::Project)
        {
            if !program
                .project_components()
                .iter()
                .any(|c| c.name == e.name)
            {
                return Err(refused(
                    "migrate.missing_removal",
                    format!(
                        "The new scripts drop the component {}; name it in game({{retired}}) to remove it.",
                        e.name
                    ),
                    &e.name,
                ));
            }
        }
    }
    Ok(out)
}

/// Swaps the world's scripts at a boundary (hot-update.md 5): instantiates `set`, plans its
/// project components against the live registry, registers the added ones and installs the
/// program, keeping the previous bundle. Any failure leaves the installed program and the world as
/// they were. An unchanged bundle is a no-op unless `force` (a reload, hot-update.md 9).
pub fn swap(
    world: &mut World,
    set: &CompiledSet,
    force: bool,
) -> Result<SwapReport, Vec<ScriptError>> {
    let Some(mut scripts) = world.remove_non_send::<Scripts>() else {
        return Err(vec![ScriptError::new(
            "sim.internal",
            "This world has no script host; install one first.",
            ErrorPhase::Load,
        )]);
    };
    let result = swap_in(world, &mut scripts, set, force);
    world.insert_non_send(scripts);
    result
}

fn swap_in(
    world: &mut World,
    scripts: &mut Scripts,
    set: &CompiledSet,
    force: bool,
) -> Result<SwapReport, Vec<ScriptError>> {
    let previous = scripts.program.as_ref().map(|p| p.bundle().hash);
    if !force && previous == Some(set.bundle.hash) {
        return Ok(SwapReport {
            outcome: SwapOutcome::Unchanged,
            bundle: set.bundle.hash,
            previous,
            systems: SystemsDiff::default(),
            components: Vec::new(),
        });
    }
    let program = scripts.host.instantiate(set, world)?;
    let components = plan(world, &program).map_err(|e| vec![e])?;
    for c in program.project_components() {
        if components
            .iter()
            .any(|ch| matches!(ch, SchemaChange::Added { name, .. } if *name == *c.name))
        {
            register_project(world, (**c).clone())
                .map_err(|p| vec![ScriptError::new(&p.code, p.message, ErrorPhase::Load)])?;
        }
    }
    // Docs and defaults are not part of a component's fingerprint: an unchanged component takes
    // the candidate's, so components inserted after the swap get the new defaults (hot-update.md 6).
    for c in program.project_components() {
        if components
            .iter()
            .any(|ch| matches!(ch, SchemaChange::Unchanged { name } if *name == *c.name))
        {
            set_project_schema(world, c.clone());
        }
    }
    let old: Vec<String> = scripts
        .program
        .as_ref()
        .map(|p| p.systems().iter().map(|s| s.name.clone()).collect())
        .unwrap_or_default();
    let new: Vec<String> = program.systems().iter().map(|s| s.name.clone()).collect();
    let systems = SystemsDiff {
        added: new.iter().filter(|n| !old.contains(n)).cloned().collect(),
        removed: old.iter().filter(|n| !new.contains(n)).cloned().collect(),
        kept: new.iter().filter(|n| old.contains(n)).cloned().collect(),
    };
    if let Some(p) = scripts.program.take() {
        scripts.previous = Some(p.compiled().clone());
    }
    scripts.program = Some(program);
    Ok(SwapReport {
        outcome: SwapOutcome::Applied,
        bundle: set.bundle.hash,
        previous,
        systems,
        components,
    })
}

/// The outcomes of the last tick's script systems.
pub fn last_tick(world: &World) -> Vec<(String, SystemOutcome)> {
    world
        .get_non_send::<Scripts>()
        .map(|s| s.last_tick.clone())
        .unwrap_or_default()
}

/// The steps each script system used in the last tick, in run order.
pub fn last_steps(world: &World) -> Vec<(String, u64)> {
    last_tick(world)
        .into_iter()
        .map(|(n, o)| {
            let steps = match o {
                SystemOutcome::Ok(SystemStats { steps, .. })
                | SystemOutcome::Failed {
                    stats: SystemStats { steps, .. },
                    ..
                } => steps,
                SystemOutcome::Fault { .. } => 0,
            };
            (n, steps)
        })
        .collect()
}

/// Gives the world's scripts a new host on the same thread and instantiates the installed bundle in
/// it: the check that nothing of the old host carries over (reload equivalence). It cannot serve a
/// world moved to another thread, whose non-send `Scripts` bevy_ecs refuses to touch there, and a
/// forked or restored world has no `Scripts` at all: such a world is a fresh `Sim` on its own
/// thread with `install` and `swap` of the same compiled set (script-host.md 14, choice 7).
pub fn rehost(world: &mut World, limits: ScriptLimits) -> Result<(), Vec<ScriptError>> {
    let Some(old) = world.remove_non_send::<Scripts>() else {
        return Err(vec![ScriptError::new(
            "sim.internal",
            "This world has no script host.",
            ErrorPhase::Load,
        )]);
    };
    let host = match ScriptHost::new(limits) {
        Ok(h) => h,
        Err(e) => {
            world.insert_non_send(old);
            return Err(vec![e]);
        }
    };
    // The debugger stays attached to the world.
    *host.shared.debug.borrow_mut() = old.host.debugger();
    let program = match old
        .program
        .as_ref()
        .map(|p| host.instantiate(p.compiled(), world))
    {
        Some(Ok(p)) => Some(p),
        Some(Err(e)) => {
            world.insert_non_send(old);
            return Err(e);
        }
        None => None,
    };
    let previous = old.previous.clone();
    drop(old);
    world.insert_non_send(Scripts {
        host,
        program,
        previous,
        last_tick: Vec::new(),
    });
    Ok(())
}
