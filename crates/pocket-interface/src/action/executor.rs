//! Executors and the system that runs them (shared/contract/actions.md, Executors): an executor
//! carries out one intent kind; it sees only its seat's view and acts only through its seat's
//! controls on its own channels. `interface.intents`, in the `Control` phase, delivers the waiting
//! pulses, fails every active intent whose deadline has come (Rust and script intents alike), then
//! runs the Rust executors in `IntentId` order; their control writes reach this tick's forces.

use bevy_ecs::prelude::{Mut, World};
use pocket_contract::Problem;
use pocket_sim::{PlainData, SimClock, Tick};

use super::catalog::{ActionCatalog, deliver_pulses, write_control};
use super::defs::IntentDef;
use super::lifecycle::{self, End};
use super::state::{
    ControlValue, IntentInstance, IntentStatus, IntentTable, ResolvedTarget, SeatRow, seats_with,
};
use super::view::ActorView;

/// What `accept` decides from: the canonical parameters, the resolved target and the seat's view.
pub struct AcceptCx<'a> {
    pub params: &'a PlainData,
    pub target: Option<&'a ResolvedTarget>,
    pub view: &'a dyn ActorView,
    pub catalog: &'a ActionCatalog,
    /// Seconds per tick.
    pub dt: f64,
}

/// An accepted start: the executor's initial state and progress, and warnings for the caller.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Accepted {
    pub state: PlainData,
    pub progress: Vec<(String, PlainData)>,
    pub warnings: Vec<Problem>,
}

/// What one tick of an executor sees and may change.
pub struct TickCx<'a, 'c> {
    pub instance: &'a IntentInstance,
    pub state: &'a mut PlainData,
    pub view: &'a dyn ActorView,
    pub controls: &'a mut ControlWriter<'c>,
    pub catalog: &'a ActionCatalog,
    pub dt: f64,
}

/// One tick's outcome.
#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Continue {
        progress: Vec<(String, PlainData)>,
    },
    /// The goal is met: succeed, or hold if `keep`.
    Reached {
        progress: Vec<(String, PlainData)>,
    },
    /// The goal is met and the intent ends regardless.
    Succeeded {
        progress: Vec<(String, PlainData)>,
    },
    Failed {
        problem: Problem,
        progress: Vec<(String, PlainData)>,
    },
}

/// The code that carries out one intent kind.
pub trait IntentExecutor: Send + Sync {
    fn def(&self) -> &IntentDef;
    /// The channels an instance occupies: the declaration's, unless its parameters add one
    /// (`sail_to` takes `sail` too with `trim: "auto"`).
    fn channels(&self, params: &PlainData) -> Vec<String> {
        let _ = params;
        self.def().channels.clone()
    }
    /// At acceptance: check the parameters against what the seat perceives, and give the initial
    /// state and progress, or refuse with problems (nothing is applied then).
    fn accept(&self, cx: &AcceptCx<'_>) -> Result<Accepted, Vec<Problem>>;
    /// Every tick while active or holding, in `IntentId` order.
    fn tick(&self, cx: &mut TickCx<'_, '_>) -> Step;
}

/// The controls an executor may write: its seat's, on its own channels. A write to another
/// control, or one that does not fit, breaks the executor's contract and fails the intent with
/// `internal.error`; writes are applied when the executor returns.
pub struct ControlWriter<'a> {
    catalog: &'a ActionCatalog,
    channels: Vec<String>,
    writes: Vec<(String, ControlValue)>,
    broke: Option<String>,
}

impl<'a> ControlWriter<'a> {
    pub fn new(catalog: &'a ActionCatalog, channels: Vec<String>) -> ControlWriter<'a> {
        ControlWriter {
            catalog,
            channels,
            writes: Vec::new(),
            broke: None,
        }
    }

    /// Latches `value` on the control `name`.
    pub fn set(&mut self, name: &str, value: ControlValue) {
        let Some(def) = self.catalog.control(name) else {
            self.broke = Some(format!("wrote the undeclared control {name}"));
            return;
        };
        if !self.channels.contains(&def.channel) {
            self.broke = Some(format!(
                "wrote {name} on channel {} outside its channels {:?}",
                def.channel, self.channels
            ));
            return;
        }
        if let Err(why) = def.fits(&value) {
            self.broke = Some(format!("wrote {name} a value that is not {why}"));
            return;
        }
        self.writes.retain(|(n, _)| n != name);
        self.writes.push((name.to_owned(), value));
    }

    /// Latches a number on an axis control.
    pub fn axis(&mut self, name: &str, x: f64) {
        self.set(name, ControlValue::Number(x));
    }

    /// The writes made, in order, or why the contract broke.
    pub fn finish(self) -> Result<Vec<(String, ControlValue)>, String> {
        match self.broke {
            Some(why) => Err(why),
            None => Ok(self.writes),
        }
    }
}

/// The channels an instance of `intent` with `params` occupies.
pub fn channels_of(catalog: &ActionCatalog, intent: &str, params: &PlainData) -> Vec<String> {
    match catalog.intent(intent) {
        Some(e) => match &e.executor {
            Some(x) => x.channels(params),
            None => e.def.channels.clone(),
        },
        None => Vec::new(),
    }
}

/// The deadline of an intent started at `started` with `timeout_s` at `rate` ticks a second:
/// `started + ceil(timeout_s * rate)`.
pub fn deadline(started: Tick, timeout_s: f64, rate: u32) -> Tick {
    let ticks =
        pocket_sim::num::to_tick((timeout_s * f64::from(rate)).ceil()).map_or(u64::MAX, |t| t.0);
    Tick(started.0.saturating_add(ticks))
}

/// `interface.intents` (simulation.md 4.4, row 2).
pub fn run_intents(world: &mut World) -> Result<(), Problem> {
    if !world.contains_resource::<ActionCatalog>() || !world.contains_resource::<IntentTable>() {
        return Ok(());
    }
    world.resource_scope(|world, catalog: Mut<ActionCatalog>| run(world, &catalog))
}

fn run(world: &mut World, catalog: &ActionCatalog) -> Result<(), Problem> {
    deliver_pulses(world, catalog)?;
    let tick = world.resource::<SimClock>().tick;
    let dt = world.resource::<SimClock>().dt();
    let live: Vec<u64> = world
        .resource::<IntentTable>()
        .by_id
        .values()
        .filter(|i| i.status.is_live())
        .map(|i| i.id)
        .collect();
    // Deadlines first, Rust and script intents alike: a deadline reached fails the intent before
    // its executor runs in that tick.
    for &id in &live {
        let timed_out = world
            .resource::<IntentTable>()
            .by_id
            .get(&id)
            .and_then(|i| {
                (i.status == IntentStatus::Active && i.deadline_tick.is_some_and(|d| tick >= d))
                    .then(|| {
                        let t = super::catalog::param_f64(&i.params, "timeout_s").unwrap_or(0.0);
                        pocket_contract::codes::intent_timeout(&i.intent, t)
                    })
            });
        if let Some(p) = timed_out {
            lifecycle::end(world, id, End::Failed(p));
        }
    }
    if live.is_empty() {
        return Ok(());
    }
    let rows = seats_with(world, catalog)?;
    for id in live {
        let Some(instance) = world.resource::<IntentTable>().by_id.get(&id).cloned() else {
            continue;
        };
        if !instance.status.is_live() {
            continue;
        }
        let Some(exec) = catalog
            .intent(&instance.intent)
            .and_then(|e| e.executor.clone())
        else {
            continue;
        };
        let Some(row) = rows
            .iter()
            .find(|r| r.id == instance.seat && r.body == instance.actor)
        else {
            lifecycle::broke_contract(world, id, "the intent's seat has no body any more");
            continue;
        };
        let (step, writes, state) = tick_one(world, catalog, &*exec, &instance, row, dt);
        apply_tick(world, catalog, id, instance.actor, step, writes, state)?;
    }
    Ok(())
}

type Writes = Result<Vec<(String, ControlValue)>, String>;

fn tick_one(
    world: &World,
    catalog: &ActionCatalog,
    exec: &dyn IntentExecutor,
    instance: &IntentInstance,
    row: &SeatRow,
    dt: f64,
) -> (Step, Writes, PlainData) {
    let view = (catalog.view)(world, row);
    let mut state = instance.state.clone();
    let mut writer = ControlWriter::new(catalog, exec.channels(&instance.params));
    let step = {
        let mut cx = TickCx {
            instance,
            state: &mut state,
            view: &*view,
            controls: &mut writer,
            catalog,
            dt,
        };
        exec.tick(&mut cx)
    };
    (step, writer.finish(), state)
}

fn apply_tick(
    world: &mut World,
    catalog: &ActionCatalog,
    id: u64,
    actor: pocket_sim::EntityId,
    step: Step,
    writes: Writes,
    state: PlainData,
) -> Result<(), Problem> {
    let writes = match writes {
        Ok(w) => w,
        Err(why) => {
            lifecycle::broke_contract(world, id, &why);
            return Ok(());
        }
    };
    // A write the actor's body cannot take (it lost its `Boat`) fails the intent, never the tick,
    // and none of the tick's writes is made.
    if let Some((name, _)) = writes.iter().find(|(n, _)| {
        catalog
            .control(n)
            .is_some_and(|d| !d.binding.writable(world, actor))
    }) {
        let report = format!("its body cannot take the control {name}");
        lifecycle::broke_contract(world, id, &report);
        return Ok(());
    }
    for (name, value) in &writes {
        if let Some(def) = catalog.control(name)
            && let Err(p) = write_control(world, def, actor, value)
        {
            let report = format!("writing {name} on its body failed: {}", p.message);
            lifecycle::broke_contract(world, id, &report);
            return Ok(());
        }
    }
    if let Some(i) = world.resource_mut::<IntentTable>().by_id.get_mut(&id) {
        i.state = state;
    }
    match step {
        Step::Continue { progress } => lifecycle::set_progress_with(world, catalog, id, progress),
        Step::Reached { progress } => {
            lifecycle::set_progress_with(world, catalog, id, progress);
            lifecycle::reached(world, id);
        }
        Step::Succeeded { progress } => {
            lifecycle::set_progress_with(world, catalog, id, progress);
            lifecycle::end(world, id, End::Succeeded);
        }
        Step::Failed { problem, progress } => {
            lifecycle::set_progress_with(world, catalog, id, progress);
            lifecycle::end(world, id, End::Failed(problem));
        }
    }
    Ok(())
}
