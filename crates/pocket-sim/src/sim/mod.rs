//! The simulation and its fixed-timestep step (docs/spec/simulation.md 4 and 6).
//!
//! The schedule is a list of systems that `Sim` runs one by one in the total order of
//! simulation.md 4.4, on the calling thread: `sim.begin`, the registered systems by phase and
//! slot, `sim.finish`. Running the list itself (instead of a `bevy_ecs` `Schedule` of chained sets)
//! gives the step a point between systems for the hooks, for catching a panic and naming its
//! system, and for poisoning the world there; the order is total by construction, so there is
//! nothing for ambiguity detection to find (simulation-slice1.md, 7).

mod boundary;
mod float_env;
mod invocation;
mod state;
#[cfg(test)]
mod tests;

pub use boundary::Boundary;
pub use float_env::check as check_float_env;
pub use invocation::{Invocation, begin_invocation, commit_invocation, rollback_invocation};
pub use state::{
    DecisionRequest, Poisoned, TickOutput, TickState, internal, system_failed, world_poisoned,
};

use std::panic::{AssertUnwindSafe, catch_unwind};

use bevy_ecs::prelude::{IntoSystem, World};
use bevy_ecs::system::{RunSystemError, System};
use pocket_contract::{Problem, detail};
use serde_json::json;

use crate::entity::{EntityAllocator, EntityIndex, Name, ReservedIds};
use crate::event::{Event, EventCounter, EventInbox, EventOutbox};
use crate::registry::ComponentRegistry;
use crate::rng::{RngTable, WorldSeed};
use crate::schedule::{ScheduleEntry, StepHooks, SystemCtx, SystemKey, TickPhase, slot};
use crate::time::{RunCondition, SimClock, Tick, TickRate};

/// What a world is created with: its rate (fixed for its whole history) and its seed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SimConfig {
    pub rate: TickRate,
    pub seed: u64,
}

/// What one step reports.
#[derive(Clone, Debug, PartialEq)]
pub struct StepReport {
    /// The tick just completed.
    pub tick: Tick,
    /// The new inbox: the tick's event record (simulation.md 5.3).
    pub events: Vec<Event>,
    /// Decision requests, in request order.
    pub decisions: Vec<DecisionRequest>,
    /// Failed invocations (`sim.system_failed`), in the order they failed.
    pub errors: Vec<Problem>,
}

type ExclusiveFn = Box<dyn FnMut(&mut World, &mut SystemCtx<'_>) + Send + Sync>;

enum Run {
    Begin,
    Finish,
    Bevy(Box<dyn System<In = (), Out = ()>>),
    Exclusive(ExclusiveFn),
}

struct Entry {
    key: SystemKey,
    phase: TickPhase,
    slot: u32,
    seq: u32,
    condition: RunCondition,
    run: Run,
}

/// A world and its schedule. Owned by one thread, the game thread (charter 5.1).
pub struct Sim {
    world: World,
    systems: Vec<Entry>,
    next_seq: u32,
}

impl Sim {
    /// A new world at tick 0 with the given rate and seed (`sim.tick_rate_invalid`,
    /// `rng.seed_invalid`), holding the simulation's resources and the `Name` component's
    /// registration.
    pub fn new(config: SimConfig) -> Result<Sim, Problem> {
        let rate = TickRate::new(config.rate.0)?;
        let seed = WorldSeed::new(config.seed)?;
        let mut world = World::new();
        world.insert_resource(SimClock {
            tick: Tick(0),
            rate,
        });
        world.insert_resource(seed);
        world.insert_resource(EntityAllocator::default());
        world.insert_resource(EventCounter::default());
        world.insert_resource(EventInbox::default());
        world.insert_resource(ComponentRegistry::default());
        crate::persisted::rebuild_derived(&mut world);
        ComponentRegistry::register::<Name>(&mut world, None)?;
        let mut sim = Sim {
            world,
            systems: Vec::new(),
            next_seq: 0,
        };
        sim.push(
            "sim.begin",
            TickPhase::Begin,
            RunCondition::Always,
            Run::Begin,
        )?;
        sim.push(
            "sim.finish",
            TickPhase::Finish,
            RunCondition::Always,
            Run::Finish,
        )?;
        Ok(sim)
    }

    fn push(
        &mut self,
        key: &str,
        phase: TickPhase,
        condition: RunCondition,
        run: Run,
    ) -> Result<(), Problem> {
        let key = SystemKey::new(key)?;
        if self.systems.iter().any(|e| e.key == key) {
            return Err(Problem::new(
                "sim.system_duplicate",
                format!("A system with the key {key} is already in the schedule."),
                detail([("system", json!(key.as_str()))]),
            ));
        }
        let slot = slot(key.as_str(), phase)?;
        let seq = self.next_seq;
        self.next_seq += 1;
        let at = self
            .systems
            .partition_point(|e| (e.slot, e.seq) < (slot, seq));
        self.systems.insert(
            at,
            Entry {
                key,
                phase,
                slot,
                seq,
                condition,
                run,
            },
        );
        Ok(())
    }

    /// Adds a `bevy_ecs` system under `key` in `phase`. Its place is the key's row of the system
    /// order (simulation.md 4.4; `sim.system_phase` if the phase is not that row's), or after the
    /// listed systems of its phase. Its deferred commands apply when it returns.
    pub fn add_system<M>(
        &mut self,
        key: &str,
        phase: TickPhase,
        condition: RunCondition,
        system: impl IntoSystem<(), (), M>,
    ) -> Result<(), Problem> {
        let mut boxed: Box<dyn System<In = (), Out = ()>> =
            Box::new(IntoSystem::into_system(system));
        boxed.initialize(&mut self.world);
        self.push(key, phase, condition, Run::Bevy(boxed))
    }

    /// Adds a system written over the whole world (the script host's `script.update`).
    ///
    /// The closure must keep no state that influences a later tick: state kept outside the world
    /// is not in snapshots, forks or the hash, so a restore or fork would continue differently
    /// (simulation.md 4.1, invariant 4). Caches that are rebuilt identically from the world (the
    /// script host's stateless VM, compiled code) are allowed; anything else goes in the world.
    /// For the same reason `bevy_ecs`'s `Local` is disallowed in tick code (simulation.md 10).
    pub fn add_exclusive(
        &mut self,
        key: &str,
        phase: TickPhase,
        condition: RunCondition,
        system: impl FnMut(&mut World, &mut SystemCtx<'_>) + Send + Sync + 'static,
    ) -> Result<(), Problem> {
        self.push(key, phase, condition, Run::Exclusive(Box::new(system)))
    }

    /// The flattened schedule in run order, `sim.begin` and `sim.finish` included.
    pub fn schedule(&self) -> Vec<ScheduleEntry> {
        self.systems
            .iter()
            .map(|e| ScheduleEntry {
                phase: e.phase,
                key: e.key.clone(),
                condition: e.condition,
            })
            .collect()
    }

    /// The schedule as text, one `phase key condition` line per system: the schedule golden's
    /// form (simulation.md 12, item 1).
    pub fn schedule_listing(&self) -> String {
        self.schedule()
            .iter()
            .map(|e| format!("{} {} {}\n", e.phase.name(), e.key, e.condition.label()))
            .collect()
    }

    /// The clock: the last completed tick, and the rate.
    pub fn clock(&self) -> SimClock {
        *self.world.resource::<SimClock>()
    }

    /// Read-only access at a boundary, for perception, hashing and snapshot publication.
    pub fn world(&self) -> &World {
        &self.world
    }

    /// The whole world, for whole-world operations at a boundary (a restore, which then runs the
    /// rebuild functions, `persisted::rebuild_derived` first). Boundary writes go through
    /// [`Sim::boundary`].
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Mutable access between ticks, for boundary writes (simulation.md 4.2).
    pub fn boundary(&mut self) -> Boundary<'_> {
        Boundary::new(&mut self.world)
    }

    /// Whether the last tick stopped halfway.
    pub fn poisoned(&self) -> Option<&Poisoned> {
        self.world.get_resource::<Poisoned>()
    }

    fn poison(&mut self, tick: Tick, problem: Problem) -> Problem {
        self.world.insert_resource(Poisoned {
            tick,
            problem: problem.clone(),
        });
        problem
    }

    /// Runs exactly one tick. Never blocks, never waits for input, never reads the clock.
    ///
    /// A failed invocation never stops the tick: it is in the report's `errors`. A panic in a
    /// system, a fault a system reports, a broken boundary invariant or a changed floating-point
    /// environment stops the tick there, poisons the world and answers with its problem; a
    /// poisoned world refuses with `sim.world_poisoned` until a snapshot is restored into it.
    pub fn step(&mut self, hooks: &mut dyn StepHooks) -> Result<StepReport, Problem> {
        if let Some(p) = self.poisoned() {
            return Err(world_poisoned(p.tick));
        }
        let last = self.clock().tick;
        let tick = Tick(last.0 + 1);
        if let Err(why) = self.check_boundary("start") {
            let p = internal(tick, TickPhase::Begin, "sim.begin", &why);
            return Err(self.poison(last, p));
        }
        if let Err(p) = float_env::check() {
            return Err(self.poison(last, p));
        }
        // Systems are sorted by slot, and slots ascend with the phases, so one pass visits the
        // phases in order.
        let mut i = 0;
        for phase in TickPhase::ALL {
            hooks.phase(phase, true);
            while i < self.systems.len() && self.systems[i].phase == phase {
                let at = i;
                i += 1;
                if !self.systems[at].condition.runs_on(tick) {
                    continue;
                }
                if let Err(p) = self.run_one(at, tick, hooks) {
                    return Err(self.poison(tick, p));
                }
            }
            hooks.phase(phase, false);
        }
        // bevy_ecs's change ticks wrap after 2^32 system runs unless clamped now and then; they
        // are not world state and drive no logic here (4.5), so this only keeps bevy sound.
        if let Some(check) = self.world.check_change_ticks() {
            for e in &mut self.systems {
                if let Run::Bevy(s) = &mut e.run {
                    s.check_change_tick(check);
                }
            }
        }
        let mut out = self.world.resource_mut::<TickOutput>();
        let errors = std::mem::take(&mut out.errors);
        let decisions = std::mem::take(&mut out.decisions);
        Ok(StepReport {
            tick,
            events: self.world.resource::<EventInbox>().events().to_vec(),
            decisions,
            errors,
        })
    }

    /// Runs system `at` of tick `tick` between its hooks; a panic, a run error or a reported
    /// fault comes back as the problem that poisons the world.
    fn run_one(&mut self, at: usize, tick: Tick, hooks: &mut dyn StepHooks) -> Result<(), Problem> {
        let entry = &mut self.systems[at];
        let world = &mut self.world;
        let phase = entry.phase;
        hooks.system(&entry.key, true);
        let mut ctx = SystemCtx {
            hooks: &mut *hooks,
            key: &entry.key,
            phase,
            tick,
            fault: None,
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| match &mut entry.run {
            Run::Begin => {
                begin(world, tick);
                Ok(())
            }
            Run::Finish => finish(world),
            // A system whose parameters skip it (a `Single` or `Populated` with nothing to
            // match) did not run this tick; that is not a fault (bevy_ecs's own reading).
            Run::Bevy(s) => match s.run((), world) {
                Ok(()) | Err(RunSystemError::Skipped(_)) => Ok(()),
                Err(RunSystemError::Failed(e)) => Err(e.to_string()),
            },
            Run::Exclusive(f) => {
                f(world, &mut ctx);
                Ok(())
            }
        }));
        let fault = ctx.fault.take();
        hooks.system(&entry.key, false);
        let key = entry.key.as_str();
        match outcome {
            Err(panic) => Err(internal(tick, phase, key, &panic_message(&*panic))),
            Ok(Err(message)) => Err(internal(tick, phase, key, &message)),
            Ok(Ok(())) => fault.map_or(Ok(()), Err),
        }
    }

    /// The boundary invariants of simulation.md 4.1 that cost constant time (no invocation or tick
    /// running, an empty outbox and RNG table, no staged structural change, no undelivered tick
    /// output), plus with the feature `invariants` the walk of the index against the world.
    fn check_boundary(&mut self, when: &str) -> Result<(), String> {
        check_boundary(&mut self.world)
            .map_err(|e| format!("boundary invariant at the {when}: {e}"))
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic without a message".to_owned()
    }
}

/// `sim.begin`: the clock to tick n, the RNG table opened with the seed, the boundary's id
/// reservations ended.
fn begin(world: &mut World, tick: Tick) {
    world.resource_mut::<SimClock>().tick = tick;
    world.resource_mut::<ReservedIds>().clear();
    let seed = *world.resource::<WorldSeed>();
    world.resource_mut::<RngTable>().begin_tick(tick, seed);
    world.resource_mut::<TickState>().running = Some(tick);
}

/// `sim.finish`: the outbox becomes the inbox, the RNG table is cleared, the index compacted after
/// confirming every spawn and despawn of the tick happened, and the boundary invariants asserted.
fn finish(world: &mut World) -> Result<(), String> {
    if let Some(key) = world.resource::<TickState>().invocation() {
        return Err(format!(
            "the invocation of {key} was never committed or rolled back"
        ));
    }
    let events = world.resource_mut::<EventOutbox>().take();
    world.resource_mut::<EventInbox>().replace_with_tick(events);
    world.resource_mut::<RngTable>().end_tick();
    world.resource_mut::<ReservedIds>().clear();
    if let Some(why) = world.resource::<EntityIndex>().staged_changes(world) {
        return Err(why);
    }
    world.resource_mut::<EntityIndex>().compact();
    world.resource_mut::<TickState>().running = None;
    // The tick's output is still to be taken by the step's report.
    check_invariants(world)
}

/// The boundary invariants (simulation.md 4.1), and no tick output left undelivered.
pub(crate) fn check_boundary(world: &mut World) -> Result<(), String> {
    check_invariants(world)?;
    let out = world.resource::<TickOutput>();
    if !out.errors.is_empty() || !out.decisions.is_empty() {
        return Err("the last tick's output was not delivered".into());
    }
    Ok(())
}

/// The boundary invariants (simulation.md 4.1).
fn check_invariants(world: &mut World) -> Result<(), String> {
    let state = world.resource::<TickState>();
    if let Some(t) = state.running() {
        return Err(format!("tick {} is still running", t.0));
    }
    if let Some(k) = state.invocation() {
        return Err(format!("the invocation of {k} is still open"));
    }
    if !world.resource::<EventOutbox>().is_empty() {
        return Err("the event outbox is not empty".into());
    }
    if !world.resource::<RngTable>().is_empty() {
        return Err("the RNG table is not empty".into());
    }

    if let Some(why) = world.resource::<EntityIndex>().staged_changes(world) {
        return Err(why);
    }
    #[cfg(any(test, feature = "invariants"))]
    walk(world)?;
    Ok(())
}

/// The walk the feature `invariants` adds: the index against the world, and the inbox's events in
/// ascending sequence, below the counter, all of the tick the world shows.
#[cfg(any(test, feature = "invariants"))]
fn walk(world: &mut World) -> Result<(), String> {
    world
        .resource_scope(|world, index: bevy_ecs::prelude::Mut<EntityIndex>| index.verify(world))?;
    let next = world.resource::<EventCounter>().next();
    let tick = world.resource::<SimClock>().tick;
    let inbox = world.resource::<EventInbox>();
    let mut prev = 0;
    for e in inbox.events() {
        if e.seq.0 <= prev || e.seq.0 >= next {
            return Err(format!("event {} is out of sequence", e.seq.0));
        }
        if e.tick != tick {
            return Err(format!(
                "event {} is of tick {}, not {}",
                e.seq.0, e.tick.0, tick.0
            ));
        }
        prev = e.seq.0;
    }
    if inbox.boundary_from() > inbox.events().len() {
        return Err("the inbox's boundary mark is past its end".into());
    }
    Ok(())
}
