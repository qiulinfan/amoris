//! The game (docs/spec/architecture.md 4.8; threads.md 3.1 and 5): one world with physics and the
//! project's scripts, built from a [`GameSetup`], driven synchronously. Every change from outside a
//! tick is a command applied at a boundary through [`Game::apply`]: the catalog says what kind it
//! is, a Write that succeeds is handed to the recorder in its canonical form as an input of the next
//! tick, and a refused one changes nothing. The headless and batch forms and every check drive a
//! `Game` directly; the game thread wraps one (`thread`).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use bevy_ecs::prelude::World;
use pocket_contract::{Problem, detail};
use pocket_interface::time::control::Controller;
use pocket_interface::time::play::PlayPacing;
use pocket_link::{Kind, RegistryInfo, Source};
use pocket_persist::replay::{
    Applied, BundleRecord, RebaseCause, RecordOptions, RecordSink, RecordedWrite, Recorder, Replay,
    ReplaySource, ReplaySummary, Stepper, canonical_json,
};
use pocket_persist::{
    FormatTable, Registry, RestoreOptions, Snapshot, SnapshotContext, WorldHash, restore,
};
use pocket_script::scripts::SchemaChange;
use pocket_script::{CompiledSet, ScriptLimits};
use pocket_sim::registry::ComponentSchema;
use pocket_sim::{ContentHash, Event, NoHooks, Sim, SimConfig, StepReport, Tick, TickRate};
use serde_json::{Value, json};

use crate::catalog::{self, Command, CommandFn, NoParams};
use crate::control::{Sampler, StepParams, StepStop};
use crate::edit::{self, Edit, WorldEditOps, WorldEditParams, WorldGetParams, label_of};
use crate::files::{self, ScriptReadParams, ScriptWriteParams};
use crate::history::{Entry, History};
use crate::inspect::{self, WorldQueryParams, WorldSchemaParams, WorldTreeParams};
use crate::project::GameSetup;
use crate::scene::Scene;
use crate::scripts::{
    self, ScriptsApplyParams, ScriptsSwapParams, bundle_record, compiled_from_record,
};
use crate::types::{self, ScriptsTypesParams};
use crate::{decode, engine};

/// A system a game or a test adds to the schedule.
pub type SystemFn = Arc<dyn Fn(&mut Sim) -> Result<(), Problem> + Send + Sync>;

/// What a game adds to the engine's systems and commands; a fork is built with the same.
#[derive(Clone, Default)]
pub struct Extras {
    systems: Vec<SystemFn>,
    commands: Vec<(String, Kind, CommandFn)>,
}

/// Builds a [`Game`].
pub struct GameBuilder {
    setup: Arc<GameSetup>,
    seed: u64,
    extras: Extras,
    project: Option<PathBuf>,
}

impl GameBuilder {
    pub fn new(setup: Arc<GameSetup>) -> GameBuilder {
        GameBuilder {
            setup,
            seed: 1,
            extras: Extras::default(),
            project: None,
        }
    }

    /// The project directory the game was loaded from: `project.*` and `scripts.*` read and write
    /// its files, and `scripts.apply` compiles the scripts as they are on disk.
    pub fn project(mut self, root: impl Into<PathBuf>) -> GameBuilder {
        self.project = Some(root.into());
        self
    }

    pub fn seed(mut self, seed: u64) -> GameBuilder {
        self.seed = seed;
        self
    }

    /// Adds systems to the schedule (a test's defect, a game's Rust rule).
    pub fn system(
        mut self,
        install: impl Fn(&mut Sim) -> Result<(), Problem> + Send + Sync + 'static,
    ) -> GameBuilder {
        self.extras.systems.push(Arc::new(install));
        self
    }

    /// Adds a Write (or Read) command to the catalog. A Write validates everything before it
    /// changes a component or despawns ([`CommandFn`]).
    pub fn command(mut self, name: &str, kind: Kind, f: CommandFn) -> GameBuilder {
        self.extras.commands.push((name.to_owned(), kind, f));
        self
    }

    /// The game at tick 0 with its scene spawned.
    pub fn build(self) -> Result<Game, Problem> {
        let set = self.setup.scripts.clone();
        let mut g = Game::make(self.setup, self.seed, self.extras, &set)?;
        g.project = self.project;
        let scene = g.setup.scene.clone();
        scene.spawn_into(&mut g.sim.boundary())?;
        // Every seat of the scene starts with the episode's `start` decision point (time.md).
        g.player.attach(g.sim.world());
        Ok(g)
    }

    /// The game with no entities, running `set`: what a restore or a replay fills.
    pub fn build_empty(self, set: &CompiledSet) -> Result<Game, Problem> {
        let mut g = Game::make(self.setup, self.seed, self.extras, set)?;
        g.project = self.project;
        Ok(g)
    }
}

/// The persistence registry of every game: the simulation's, physics' and the script host's types.
pub fn registry() -> Arc<Registry> {
    static REG: OnceLock<Result<Arc<Registry>, Problem>> = OnceLock::new();
    REG.get_or_init(|| {
        let mut b = pocket_persist::sim_registry();
        pocket_physics::declare(&mut b);
        pocket_assets::declare(&mut b);
        pocket_script::scripts::declare(&mut b);
        crate::player::declare(&mut b);
        b.build().map(Arc::new)
    })
    .clone()
    .unwrap_or_else(|p| panic!("the engine's persistence registry does not build: {p:?}"))
}

/// The game.
pub struct Game {
    sim: Sim,
    reg: Arc<Registry>,
    setup: Arc<GameSetup>,
    extras: Extras,
    /// Every bundle this game has run or prepared, by hash.
    bundles: BTreeMap<ContentHash, CompiledSet>,
    current: ContentHash,
    recorder: Option<Recorder>,
    /// Writes applied at the current boundary.
    writes: u32,
    host_seq: u64,
    /// Bumped whenever the world's content is replaced in place (a restore): presenters redraw all.
    generation: u64,
    /// The project directory, when the game was loaded from one.
    project: Option<PathBuf>,
    /// The editor's undo and redo stacks (never persisted or hashed).
    history: History,
    /// The last compile's diagnostics (`scripts.list`).
    diagnostics: Vec<Value>,
    /// The first tick a script debugger evaluated in (script-host.md 13): the run is not
    /// replayable from it on.
    tainted: Option<u64>,
    /// The players' session: pacing, every seat's decision points and push cursor (session state,
    /// never persisted or hashed; `crate::player`).
    player: Controller,
    /// The wall clock the players' session reads (the game thread's), if any.
    wall: Option<Arc<dyn Fn() -> f64 + Send + Sync>>,
}

/// A player's `wait` the game thread drives (`Game::begin_player_wait`).
pub struct PlayerRun {
    run: Box<pocket_interface::time::stepping::Stepping>,
    source: Source,
    real_time: bool,
}

impl PlayerRun {
    /// Stepped pacing: the most ticks the run may still take (what the loop's model is asked
    /// for); real time: none, the clock runs the ticks.
    pub fn ticks_left(&self) -> u64 {
        if self.real_time {
            0
        } else {
            self.run.ticks_left()
        }
    }

    /// Whether the clock, not the run, drives its ticks.
    pub fn real_time(&self) -> bool {
        self.real_time
    }

    /// Before the loop's next tick: whether the run wants it (`false`: it ends without it).
    pub fn before_tick(&mut self, game: &Game) -> bool {
        self.run.before_tick(game.world())
    }

    /// When the run answers `wall_limit` at the latest (the loop's milliseconds).
    pub fn wall_deadline(&self) -> f64 {
        self.run.wall_deadline()
    }

    /// Ends the run at a boundary on its wall limit.
    pub fn stop_at_wall(&mut self) {
        self.run.stop_at_wall();
    }
}

/// What `Game::begin_player_wait` gives: the answer at once, or the run for the loop to drive.
pub enum PlayerStart {
    Answered(Value),
    Running(PlayerRun),
}

fn bad_hash(text: &str) -> Problem {
    Problem::new(
        "request.invalid_value",
        format!("'{text}' is not a bundle hash of 64 hex digits."),
        detail([("bundle", json!(text))]),
    )
}

impl Game {
    /// The game of `setup` with seed `seed`, its scene spawned.
    pub fn new(setup: Arc<GameSetup>, seed: u64) -> Result<Game, Problem> {
        GameBuilder::new(setup).seed(seed).build()
    }

    fn make(
        setup: Arc<GameSetup>,
        seed: u64,
        extras: Extras,
        set: &CompiledSet,
    ) -> Result<Game, Problem> {
        let mut sim = Sim::new(SimConfig {
            rate: setup.rate,
            seed,
        })?;
        pocket_physics::plugin(&mut sim)?;
        pocket_assets::plugin(&mut sim)?;
        engine::install_script_components(sim.world_mut())?;
        crate::player::install(&mut sim, setup.player.as_ref())?;
        pocket_script::install(&mut sim, setup.limits)?;
        for f in &extras.systems {
            f(&mut sim)?;
        }
        pocket_script::swap(sim.world_mut(), set, false)
            .map_err(|e| scripts::refused("load", &e))?;
        sim.world_mut().insert_resource(SnapshotContext {
            bundle: set.bundle.hash,
            writes: 0,
        });
        let mut bundles = BTreeMap::new();
        bundles.insert(set.bundle.hash, set.clone());
        let filter = setup
            .player
            .as_ref()
            .map(|s| s.decisions.clone())
            .unwrap_or_default();
        Ok(Game {
            player: Controller::new(PlayPacing::Stepped, filter, Vec::new()),
            wall: None,
            sim,
            reg: registry(),
            current: set.bundle.hash,
            setup,
            extras,
            bundles,
            recorder: None,
            writes: 0,
            host_seq: 0,
            generation: 0,
            project: None,
            history: History::default(),
            diagnostics: Vec::new(),
            tainted: None,
        })
    }

    /// Hands the render feed what changed in the visual state (presentation only; see
    /// `present`).
    pub fn present(
        &mut self,
        extractor: &mut crate::present::Extractor,
        feed: &pocket_assets::Feed,
    ) {
        let generation = self.generation;
        extractor.publish(self.sim.world_mut(), generation, feed);
    }

    pub fn sim(&self) -> &Sim {
        &self.sim
    }

    pub fn world(&self) -> &World {
        self.sim.world()
    }

    /// The tick the world shows (it is at the boundary after it).
    pub fn tick(&self) -> Tick {
        self.sim.clock().tick
    }

    pub fn setup(&self) -> &Arc<GameSetup> {
        &self.setup
    }

    pub fn registry(&self) -> &Arc<Registry> {
        &self.reg
    }

    /// The project directory, when the game was loaded from one.
    pub fn project(&self) -> Option<&PathBuf> {
        self.project.as_ref()
    }

    /// The edit history.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Takes the script console lines written since the last call (script-host.md 5.7).
    pub fn take_log(&self) -> Vec<pocket_script::LogLine> {
        self.sim
            .world()
            .get_non_send::<pocket_script::Scripts>()
            .map(|s| s.host.take_log())
            .unwrap_or_default()
    }

    /// The bundle the world runs.
    pub fn bundle(&self) -> ContentHash {
        self.current
    }

    /// Writes applied at the current boundary.
    pub fn writes(&self) -> u32 {
        self.writes
    }

    /// Attaches a script debugger to this game's script host, or detaches it (`None`), on the
    /// thread that owns the game (docs/spec/debugger.md 8; `pocket_debug::DebugHub::hook`). A fork
    /// or a replay of this game is not debugged.
    pub fn set_script_debugger(
        &mut self,
        hook: Option<std::rc::Rc<dyn pocket_script::debug::DebugHook>>,
    ) {
        pocket_script::debug::attach(self.sim.world_mut(), hook);
    }

    /// The first tick a script debugger evaluated in, if one did (script-host.md 13): the run is
    /// not a replayable record from there.
    pub fn tainted(&self) -> Option<u64> {
        self.tainted
    }

    /// The script debugger attached to this game, if any (Play hands it to its fork and back,
    /// `thread.rs`).
    pub fn script_debugger(&self) -> Option<std::rc::Rc<dyn pocket_script::debug::DebugHook>> {
        self.sim
            .world()
            .get_non_send::<pocket_script::Scripts>()
            .and_then(|s| s.host.debugger())
    }

    pub fn snapshot(&self) -> Result<Snapshot, Problem> {
        pocket_persist::snapshot(self.sim.world(), &self.reg)
    }

    pub fn world_hash(&self) -> Result<WorldHash, Problem> {
        pocket_persist::world_hash(self.sim.world(), &self.reg)
    }

    /// The events of the last tick and those boundary writes appended since (simulation.md 5.3).
    pub fn inbox(&self) -> &[Event] {
        self.sim
            .world()
            .resource::<pocket_sim::EventInbox>()
            .events()
    }

    /// The registry as presenters read it.
    pub fn registry_info(&self) -> Result<RegistryInfo, Problem> {
        Ok(RegistryInfo {
            components: self
                .sim
                .world()
                .resource::<pocket_sim::ComponentRegistry>()
                .info(),
            formats: FormatTable::of(&self.reg, self.sim.world())?,
        })
    }

    /// The script limits that decide results, as canonical JSON (versions.md 3.7).
    pub fn run_config(&self) -> String {
        run_config(&self.setup.limits)
    }

    fn sync_context(&mut self) {
        self.sim.world_mut().insert_resource(SnapshotContext {
            bundle: self.current,
            writes: self.writes,
        });
    }

    fn next_host_seq(&mut self) -> u64 {
        self.host_seq += 1;
        self.host_seq
    }

    /// Applies one command at the current boundary (threads.md 5.4): a Read answers, a Write
    /// changes the world and is recorded, or is refused and changes nothing, a Request runs its
    /// stages inline and applies the Write it produces (threads.md 5.5), and `step` runs ticks.
    pub fn apply(&mut self, cmd: &Command) -> Result<Value, Problem> {
        self.apply_from(cmd, &(), false)
    }

    fn apply_from(
        &mut self,
        cmd: &Command,
        src: &dyn ReplaySource,
        replaying: bool,
    ) -> Result<Value, Problem> {
        let def = catalog::find(&cmd.name);
        let (name, kind) = match def {
            Some(def) => (def.name, def.kind),
            None => match self.extras.commands.iter().find(|(n, _, _)| *n == cmd.name) {
                Some((_, kind, _)) => ("", *kind),
                None => return Err(catalog::unknown(&cmd.name, &self.extras.commands)),
            },
        };
        if !replaying {
            // A player sends the player tools alone (`crate::player::permitted`).
            let named = if name.is_empty() { &cmd.name } else { name };
            crate::player::permitted(self.sim.world(), named, cmd.source)?;
        }
        if let Some(def) = def {
            if def.host_only && cmd.source != Source::Host && !replaying {
                return Err(catalog::host_only(def.name, cmd.source));
            }
            if def.thread {
                return Err(catalog::thread_only(def.name));
            }
        }
        if name.starts_with("player.") && !crate::player::declared(self.sim.world()) {
            return Err(crate::player::no_layer(name));
        }
        match kind {
            Kind::Read => self.read(cmd, name),
            Kind::Write if name == "player.act" => {
                let applied = self.write_act(cmd, replaying)?;
                if replaying {
                    return Ok(applied);
                }
                let now = self.now();
                let world = self.sim.world();
                Ok(crate::player::act_answer(
                    world,
                    &mut self.player,
                    &applied,
                    &cmd.params,
                    now,
                ))
            }
            Kind::Write => self.write(cmd, name, src),
            Kind::Request => self.request(cmd, name),
            Kind::Control => self.control(cmd, name),
        }
    }

    /// `player.act`'s Write: validated whole and applied at this boundary, recorded in its
    /// canonical form (actions.md, What a replay records); refused, it changes nothing.
    fn write_act(&mut self, cmd: &Command, replaying: bool) -> Result<Value, Problem> {
        let tick = self.tick();
        let mark = self.sim.boundary().mark();
        let r = crate::player::act_apply(self.sim.world_mut(), cmd.source, replaying, &cmd.params);
        let applied = |params: Value, index: u32| Applied {
            tick: Tick(tick.0 + 1),
            index,
            source: cmd.source,
            seq: cmd.seq,
            name: "player.act".to_owned(),
            params,
        };
        match r {
            Ok((answer, canonical)) => {
                let a = applied(canonical, self.writes);
                self.writes += 1;
                self.sync_context();
                if let Some(rec) = &mut self.recorder {
                    rec.applied(&a);
                }
                Ok(answer)
            }
            Err(p) => {
                self.sim.boundary().rollback(mark);
                if let Some(rec) = &mut self.recorder {
                    rec.refused(&applied(cmd.params.clone(), self.writes), &p.code);
                }
                Err(p)
            }
        }
    }

    /// The players' session (decision points, push cursors, pacing): session state beside the
    /// world (`crate::player`).
    pub fn player(&self) -> &Controller {
        &self.player
    }

    pub fn player_mut(&mut self) -> &mut Controller {
        &mut self.player
    }

    /// Installs the wall clock (milliseconds) the players' session measures `max_wall_ms` and
    /// thinking clocks with: the game thread's injected clock. It decides only where a run stops
    /// and when real time resumes, never what a tick computes; a game without one reads 0.
    pub fn set_wall_clock(&mut self, clock: Arc<dyn Fn() -> f64 + Send + Sync>) {
        self.wall = Some(clock);
    }

    /// The wall clock's now, or 0 without one.
    fn now(&self) -> f64 {
        self.wall.as_ref().map_or(0.0, |c| c())
    }

    /// Whether the world declares a player layer (`crate::player`).
    pub fn has_players(&self) -> bool {
        crate::player::declared(self.sim.world())
    }

    /// The game thread's answer at a boundary for a game with players: the players' session decides
    /// (a halt, the episode's end, real time held by a pending decision with pause-on-decision or a
    /// thinking clock running), deferring to the loop's model otherwise (time.md, Threads and the
    /// web).
    pub fn player_pace(
        &mut self,
        model: &mut pocket_interface::TimeModel,
        now_ms: f64,
    ) -> pocket_interface::Pace {
        // A seat whose body was spawned since gets its decision state.
        self.player.attach(self.sim.world());
        self.player.pace(self.sim.world(), model, now_ms)
    }

    /// After a tick the game thread's loop ran: the players' decision points, computed once for the
    /// tick, and each player run's stop; the runs that ended, by index.
    pub fn player_after_tick(
        &mut self,
        report: &StepReport,
        runs: &mut [PlayerRun],
        now_ms: f64,
    ) -> Vec<usize> {
        if !crate::player::declared(self.sim.world()) {
            return Vec::new();
        }
        let world = self.sim.world();
        let points = self
            .player
            .after_tick(world, &report.decisions, &report.errors, now_ms);
        let mut ended = Vec::new();
        for (i, r) in runs.iter_mut().enumerate() {
            match r
                .run
                .after_points(world, &mut self.player, report, &points, now_ms)
            {
                Ok(false) => {}
                Ok(true) | Err(_) => ended.push(i),
            }
        }
        ended
    }

    /// Begins a player's `wait` on the game thread (time.md, Requests): in stepped pacing a run of
    /// ticks the loop drives one per boundary, so its queue is served between ticks; in real time
    /// a run the clock drives, answered when the seat's decision comes. Answered at once when no
    /// tick is to run.
    pub fn begin_player_wait(&mut self, cmd: &Command) -> Result<PlayerStart, Problem> {
        use pocket_interface::time::stepping::{Begun, begin, begin_wait};
        if !crate::player::declared(self.sim.world()) {
            return Err(crate::player::no_layer("player.wait"));
        }
        let step = crate::player::wait_as_step(&cmd.params)?;
        let who = crate::player::caller(self.sim.world(), cmd.source)?;
        let now = self.now();
        let real_time = matches!(self.player.pacing, PlayPacing::RealTime { .. });
        let world = self.sim.world();
        let begun = if real_time {
            begin_wait(world, &mut self.player, &who, &step, now)?
        } else {
            begin(world, &mut self.player, &who, &step, now)?
        };
        match begun {
            Begun::Answered(v) => self.with_hash(cmd.source, v).map(PlayerStart::Answered),
            Begun::Running(run) => Ok(PlayerStart::Running(PlayerRun {
                run,
                source: cmd.source,
                real_time,
            })),
        }
    }

    /// The `TimeResult` of a player run that ended.
    pub fn finish_player_run(&mut self, run: PlayerRun) -> Result<Value, Problem> {
        let source = run.source;
        let v = run.run.finish(self.sim.world(), &mut self.player)?;
        self.with_hash(source, v)
    }

    /// A developer's time answer carries the world hash; a player's never does (mcp.md 4.1).
    fn with_hash(&self, source: Source, mut v: Value) -> Result<Value, Problem> {
        if !matches!(source, Source::Player(_)) {
            v["world_hash"] = json!(self.world_hash()?.to_string());
        }
        Ok(v)
    }

    /// `player.pacing` on the game thread: the players' pacing, and the loop pacing that times
    /// its ticks (stepped, or real time at its speed, running).
    pub fn set_player_pacing(&mut self, cmd: &Command) -> Result<(PlayPacing, Value), Problem> {
        let pacing = crate::player::pacing_params(self.sim.world(), cmd.source, &cmd.params)?;
        self.player.set_pacing(pacing.clone());
        Ok((pacing, json!({"pacing": self.player.pacing})))
    }

    fn root(&self, command: &str) -> Result<&PathBuf, Problem> {
        self.project.as_ref().ok_or_else(|| files::no_root(command))
    }

    fn read(&mut self, cmd: &Command, name: &str) -> Result<Value, Problem> {
        let p = &cmd.params;
        match name {
            "catalog.list" => {
                decode::<NoParams>(p, name)?;
                Ok(catalog::catalog_json(&self.extras.commands))
            }
            "world.get" => {
                let p: WorldGetParams = decode(p, name)?;
                edit::world_get(self.sim.world(), &p)
            }
            "world.tree" => inspect::tree(self.sim.world(), &decode::<WorldTreeParams>(p, name)?),
            "world.query" => {
                inspect::query(self.sim.world(), &decode::<WorldQueryParams>(p, name)?)
            }
            "world.schema" => {
                inspect::schema(self.sim.world(), &decode::<WorldSchemaParams>(p, name)?)
            }
            "history.list" => {
                decode::<NoParams>(p, name)?;
                Ok(self.history.to_json())
            }
            "project.info" => {
                decode::<NoParams>(p, name)?;
                let rate = self.setup.rate.0;
                Ok(match &self.project {
                    Some(root) => files::info(root, &self.setup.name, rate),
                    None => json!({"name": self.setup.name, "root": null, "rate": rate}),
                })
            }
            "scripts.list" => {
                decode::<NoParams>(p, name)?;
                Ok(files::list(self.root(name)?, &self.diagnostics))
            }
            "scripts.read" => {
                let p: ScriptReadParams = decode(p, name)?;
                files::read(self.root(name)?, &p)
            }
            "scripts.types" => {
                let p: ScriptsTypesParams = decode(p, name)?;
                self.script_types(&p)
            }
            "status" => {
                decode::<NoParams>(p, name)?;
                let hash = self.world_hash().ok().map(|h| h.to_string());
                let poisoned = self.sim.poisoned().map(|p| p.tick.0);
                let entities = self.sim.world().resource::<pocket_sim::EntityIndex>().len();
                Ok(
                    json!({"tick": self.tick().0, "writes": self.writes, "world_hash": hash,
                          "bundle": self.current.to_hex(), "poisoned": poisoned,
                          "entities": entities, "tainted": self.tainted}),
                )
            }
            "snapshot" => {
                decode::<NoParams>(p, name)?;
                let s = self.snapshot()?;
                Ok(pocket_link::ReplyValue::Snapshot(s).into_json())
            }
            "scripts.status" => {
                decode::<NoParams>(p, name)?;
                let systems: Vec<String> = pocket_script::scripts::last_tick(self.sim.world())
                    .into_iter()
                    .map(|(n, _)| n)
                    .collect();
                Ok(json!({"bundle": self.current.to_hex(), "ran_last_tick": systems}))
            }
            "player.session" => {
                let model = pocket_interface::TimeModel::new(
                    self.sim.clock().rate,
                    self.player.pacing.loop_pacing(),
                );
                let now = self.now();
                let world = self.sim.world();
                crate::player::session(world, &mut self.player, cmd.source, p, &model, now)
            }
            "player.describe" => {
                let world = self.sim.world();
                self.player.attach(world);
                crate::player::describe(world, &self.player, cmd.source, p)
            }
            "player.observe" => {
                let world = self.sim.world();
                crate::player::observe(world, &mut self.player, cmd.source, p)
            }
            "player.nearby" => crate::player::nearby(self.sim.world(), cmd.source, p),
            "player.events" => crate::player::events(self.sim.world(), cmd.source, p),
            "player.affordances" => crate::player::affordances(self.sim.world(), cmd.source, p),
            "player.intents" => crate::player::intents(self.sim.world(), cmd.source, p),
            _ => {
                let (_, _, f) = self
                    .extras
                    .commands
                    .iter()
                    .find(|(n, _, _)| *n == cmd.name)
                    .ok_or_else(|| catalog::unknown(&cmd.name, &self.extras.commands))?;
                let f = f.clone();
                let mark = self.sim.boundary().mark();
                let r = f(&mut self.sim.boundary(), &cmd.params).map(|(v, _)| v);
                self.sim.boundary().rollback(mark);
                r
            }
        }
    }

    /// Applies `edits` as one `world_edit`: the result, the canonical form and the inverse.
    fn edit(&mut self, edits: Vec<Edit>) -> Result<(Value, Value, Vec<Edit>), Problem> {
        let a = edit::world_edit(&mut self.sim.boundary(), &WorldEditParams { edits })?;
        Ok((a.result, a.canonical, a.inverse))
    }

    /// `history.undo` (`undo`) or `history.redo`: applies the top entry's edits; an entry whose
    /// edits no longer apply (a tick changed what they name) is dropped with `history.stale`.
    fn undo_redo(&mut self, undo: bool) -> Result<Written, Problem> {
        let entry = if undo {
            self.history.pop_undo()
        } else {
            self.history.pop_redo()
        };
        let Some(entry) = entry else {
            let which = if undo { "undo" } else { "redo" };
            return Err(Problem::new(
                "history.empty",
                format!("There is nothing to {which}."),
                detail([("stack", json!(which))]),
            ));
        };
        match self.edit(entry.inverse) {
            Ok((mut result, canonical, inverse)) => {
                result["label"] = json!(entry.label);
                let back = Entry {
                    label: entry.label,
                    inverse,
                };
                Ok(Written {
                    result,
                    canonical,
                    recorded: "world_edit",
                    history: Some(if undo {
                        HistoryStep::Undone(back)
                    } else {
                        HistoryStep::Redone(back)
                    }),
                })
            }
            Err(p) => Err(Problem::new(
                "history.stale",
                format!(
                    "'{}' can no longer be {}: {} The entry was dropped.",
                    entry.label,
                    if undo { "undone" } else { "redone" },
                    p.message
                ),
                detail([
                    ("label", json!(entry.label)),
                    ("cause", serde_json::to_value(&p).unwrap_or(Value::Null)),
                ]),
            )),
        }
    }

    fn write(
        &mut self,
        cmd: &Command,
        name: &str,
        src: &dyn ReplaySource,
    ) -> Result<Value, Problem> {
        let tick = self.tick();
        let mark = self.sim.boundary().mark();
        let new_edit = |r: Result<(Value, Value, Vec<Edit>), Problem>, label: String| {
            r.map(|(mut result, canonical, inverse)| {
                result["label"] = json!(label);
                Written {
                    result,
                    canonical,
                    recorded: "world_edit",
                    history: Some(HistoryStep::New(label, inverse)),
                }
            })
        };
        let outcome = match name {
            "world_edit" => match decode::<WorldEditParams>(&cmd.params, name) {
                Ok(p) => {
                    let label = label_of(&p.edits);
                    new_edit(self.edit(p.edits), label)
                }
                Err(e) => Err(e),
            },
            "world.edit" => match decode::<WorldEditOps>(&cmd.params, name) {
                Ok(p) => {
                    let edits: Vec<Edit> = p.ops.into_iter().map(Edit::from).collect();
                    let label = p.label.unwrap_or_else(|| label_of(&edits));
                    let r = self
                        .edit(edits.clone())
                        .map_err(|e| edit::ops_paths(e, &edits));
                    new_edit(r, label)
                }
                Err(e) => Err(e),
            },
            "history.undo" | "history.redo" => decode::<NoParams>(&cmd.params, name)
                .and_then(|_| self.undo_redo(name == "history.undo")),
            "scripts.swap" => self
                .swap(&cmd.params, src)
                .map(|(result, canonical)| Written {
                    result,
                    canonical,
                    recorded: "scripts.swap",
                    history: None,
                }),
            _ => match self.extras.commands.iter().find(|(n, _, _)| *n == cmd.name) {
                Some((_, _, f)) => {
                    let f = f.clone();
                    f(&mut self.sim.boundary(), &cmd.params).map(|(result, canonical)| Written {
                        result,
                        canonical,
                        recorded: "",
                        history: None,
                    })
                }
                None => Err(catalog::unknown(&cmd.name, &self.extras.commands)),
            },
        };
        let applied = |name: &str, params: Value, index: u32| Applied {
            tick: Tick(tick.0 + 1),
            index,
            source: cmd.source,
            seq: cmd.seq,
            name: name.to_owned(),
            params,
        };
        match outcome {
            Ok(w) => {
                let recorded = if w.recorded.is_empty() {
                    cmd.name.as_str()
                } else {
                    w.recorded
                };
                let a = applied(recorded, w.canonical, self.writes);
                self.writes += 1;
                self.sync_context();
                if let Some(r) = &mut self.recorder {
                    r.applied(&a);
                }
                match w.history {
                    Some(HistoryStep::New(label, inverse)) => self.history.record(label, inverse),
                    Some(HistoryStep::Undone(e)) => self.history.push_redo(e),
                    Some(HistoryStep::Redone(e)) => self.history.push_undo(e),
                    None => {}
                }
                Ok(w.result)
            }
            Err(p) => {
                self.sim.boundary().rollback(mark);
                if let Some(r) = &mut self.recorder {
                    r.refused(
                        &applied(&cmd.name, cmd.params.clone(), self.writes),
                        &p.code,
                    );
                }
                Err(p)
            }
        }
    }

    /// `scripts.swap {bundle}` (hot-update.md 5): the prepared bundle, one this game has, or one a
    /// replay embeds, replaces the world's program; the recorder gets the bundle first.
    fn swap(&mut self, params: &Value, src: &dyn ReplaySource) -> Result<(Value, Value), Problem> {
        let p: ScriptsSwapParams = decode(params, "scripts.swap")?;
        let hash = ContentHash::from_hex(&p.bundle).ok_or_else(|| bad_hash(&p.bundle))?;
        let set = match self.bundles.get(&hash) {
            Some(s) => s.clone(),
            None => {
                let rec = src.bundle(&hash).ok_or_else(|| {
                    pocket_persist::error::bundle_unavailable(&p.bundle, "not prepared or embedded")
                })?;
                let s = compiled_from_record(&rec)?;
                self.bundles.insert(hash, s.clone());
                s
            }
        };
        let report = pocket_script::swap(self.sim.world_mut(), &set, true)
            .map_err(|e| scripts::refused("swap", &e))?;
        let previous = self.current;
        self.current = hash;
        self.sync_context();
        if let Some(r) = &mut self.recorder {
            r.bundle_loaded(&bundle_record(&set));
            if report
                .components
                .iter()
                .any(|c| matches!(c, SchemaChange::Added { .. }))
            {
                r.formats_changed(self.sim.world())?;
            }
        }
        let result = json!({
            "outcome": "applied",
            "applied_at": self.tick().0 + 1,
            "bundle": hash.to_hex(),
            "previous": previous.to_hex(),
            "systems": report.systems,
            "components": report.components,
        });
        Ok((result, json!({"bundle": p.bundle})))
    }

    /// A Request (threads.md 5.5): its stages run inline here.
    fn request(&mut self, cmd: &Command, name: &str) -> Result<Value, Problem> {
        match name {
            "scripts.apply" => self.scripts_apply(cmd),
            "scripts.write" => {
                let p: ScriptWriteParams = decode(&cmd.params, name)?;
                let path = files::write(self.root(name)?, &p)?;
                let diagnostics = match self.gather(&ScriptsApplyParams::default()) {
                    Ok(_) => Vec::new(),
                    Err(e) => files::diagnostics(&e),
                };
                self.diagnostics = diagnostics.clone();
                Ok(json!({"path": path, "bytes": p.text.len(), "diagnostics": diagnostics}))
            }
            "project.save" => {
                decode::<NoParams>(&cmd.params, name)?;
                files::save(self.sim.world(), self.root(name)?)
            }
            other => Err(catalog::unknown(other, &self.extras.commands)),
        }
    }

    /// `scripts.apply` (hot-update.md 4), its stages inline (threads.md 5.5): gather and compile
    /// (the project's TypeScript as it is on disk, or `files`), answer `unchanged` for the same
    /// bundle unless `force`, then apply the `scripts.swap` Host write it produces. A compile that
    /// fails answers `scripts.refused` with `diagnostics` in its detail. With `types`, the
    /// declarations of what it compiled are written too (`scripts.types`, for the check that
    /// follows), from the program it loaded: a dry run then also loads the scripts in a throwaway
    /// host and is refused, as the swap would be, when they do not load.
    fn scripts_apply(&mut self, cmd: &Command) -> Result<Value, Problem> {
        let p: ScriptsApplyParams = decode(&cmd.params, "scripts.apply")?;
        let set = match self.gather(&p) {
            Ok(s) => s,
            Err(e) => return Err(self.refusal(e, p.types)),
        };
        self.diagnostics.clear();
        let hash = set.bundle.hash;
        let mut r = if hash == self.current && !p.force {
            json!({"outcome": "unchanged", "bundle": hash.to_hex(), "diagnostics": []})
        } else if p.dry_run {
            if p.types {
                let world = self.sim.world();
                match pocket_script::scripts::declared_components(world, &set) {
                    Ok(project) => {
                        let mut r = json!({"outcome": "dry_run", "bundle": hash.to_hex(),
                                           "diagnostics": []});
                        r["types"] = self.write_types(Ok(project));
                        return Ok(r);
                    }
                    Err(e) => return Err(self.refusal(scripts::refused("load", &e), true)),
                }
            }
            json!({"outcome": "dry_run", "bundle": hash.to_hex(), "diagnostics": []})
        } else {
            self.bundles.insert(hash, set);
            let swap = Command::new(
                Source::Host,
                self.next_host_seq(),
                "scripts.swap",
                json!({"bundle": hash.to_hex()}),
            );
            let mut r = match self.apply(&swap) {
                Ok(r) => r,
                Err(e) => return Err(self.refusal(e, p.types)),
            };
            r["diagnostics"] = json!([]);
            r
        };
        if p.types {
            r["types"] = self.write_types(Ok(pocket_script::scripts::live_components(
                self.sim.world(),
            )));
        }
        Ok(r)
    }

    /// A refused apply, its `diagnostics` kept for `scripts.list` and put in its detail, and with
    /// `types` the declarations written from the running program's components.
    fn refusal(&mut self, mut e: Problem, types: bool) -> Problem {
        self.diagnostics = files::diagnostics(&e);
        e.detail
            .insert("diagnostics".into(), json!(self.diagnostics));
        if types {
            let t = self.write_types(Err(self.diagnostics.clone()));
            e.detail.insert("types".into(), t);
        }
        e
    }

    /// The declarations written for a check (no `tsconfig.json` in the source tree), or the reason
    /// they were not, as `{error, message}`.
    fn write_types(&self, project: Result<Vec<Arc<ComponentSchema>>, Vec<Value>>) -> Value {
        let d = types::declarations(self.sim.world(), project);
        types::write(
            self.project.as_deref(),
            &d,
            &ScriptsTypesParams::for_check(),
        )
        .unwrap_or_else(|e| json!({"error": e.code, "message": e.message}))
    }

    /// `scripts.types`: the SDK's declarations for the scripts as they are on disk, written into
    /// the project (`crate::types`). Scripts that are the running bundle answer the running
    /// program's components; others are compiled and loaded in a throwaway host.
    pub fn script_types(&self, p: &ScriptsTypesParams) -> Result<Value, Problem> {
        let world = self.sim.world();
        let project = match self.gather(&ScriptsApplyParams::default()) {
            Ok(set) if set.bundle.hash == self.current => {
                Ok(pocket_script::scripts::live_components(world))
            }
            Ok(set) => pocket_script::scripts::declared_components(world, &set)
                .map_err(|e| files::diagnostics(&scripts::refused("load", &e))),
            Err(p) => Err(files::diagnostics(&p)),
        };
        let d = types::declarations(world, project);
        types::write(self.project.as_deref(), &d, p)
    }

    fn gather(&self, p: &ScriptsApplyParams) -> Result<CompiledSet, Problem> {
        #[cfg(feature = "transpile")]
        {
            if let Some(files) = &p.files {
                let mut src = pocket_script::ScriptSource::new();
                for (path, text) in files {
                    src = src.with(path, text);
                }
                return scripts::compile(&src, false);
            }
            if let Some(root) = &self.project {
                let src = pocket_script::ScriptSource::read_dir(root).map_err(|e| {
                    Problem::new(
                        "project.unreadable",
                        format!("{}/scripts cannot be read: {e}.", root.display()),
                        detail([("path", json!(root.join("scripts").display().to_string()))]),
                    )
                })?;
                return scripts::compile(&src, self.setup.lint_off);
            }
            if let Some(src) = &self.setup.source {
                return scripts::compile(src, self.setup.lint_off);
            }
        }
        if p.files.is_some() {
            return Err(Problem::new(
                "scripts.transpile_unavailable",
                "This build cannot compile TypeScript; send compiled scripts.",
                detail([]),
            ));
        }
        Ok(self.setup.scripts.clone())
    }

    fn control(&mut self, cmd: &Command, name: &str) -> Result<Value, Problem> {
        match name {
            "time.step" => {
                let p: StepParams = decode(&cmd.params, name)?;
                let limit = p.limit()?;
                let mut stop = StepStop::new(self, &p)?;
                let mut sample = Sampler::new(self, &p, limit)?;
                let mut stopped_by = None;
                for _ in 0..limit {
                    self.step()?;
                    if let Some(s) = &mut sample {
                        s.after_tick(self);
                    }
                    if let Some(s) = &mut stop
                        && let Some(why) = s.after_tick(self)
                    {
                        stopped_by = Some(why);
                        break;
                    }
                }
                if stop.is_some() && stopped_by.is_none() {
                    stopped_by = Some(json!({"reason": "limit", "ticks": limit}));
                }
                let mut out =
                    json!({"tick": self.tick().0, "world_hash": self.world_hash()?.to_string()});
                if let Some(why) = stopped_by {
                    out["stopped_by"] = why;
                }
                if let Some(s) = sample {
                    out["samples"] = s.finish(self);
                }
                Ok(out)
            }
            "player.wait" => self.player_wait(cmd),
            "player.continue" => {
                if !crate::player::declared(self.sim.world()) {
                    return Err(crate::player::no_layer("player.continue"));
                }
                let who = crate::player::caller(self.sim.world(), cmd.source)?;
                let wall = self.wall.clone();
                let mut ctl = self.take_player();
                let r = pocket_interface::time::session::continue_(
                    &GameTicker { game: self },
                    &mut ctl,
                    &who,
                    &cmd.params,
                    &mut || wall.as_ref().map_or(0.0, |c| c()),
                );
                self.player = ctl;
                r
            }
            "player.pacing" => self.set_player_pacing(cmd).map(|(_, v)| v),
            other => Err(catalog::thread_only(other)),
        }
    }

    /// The players' session, taken out while a run holds the game (put back by the caller).
    fn take_player(&mut self) -> Controller {
        let filter = self.player.default_filter.clone();
        std::mem::replace(
            &mut self.player,
            Controller::new(PlayPacing::Stepped, filter, Vec::new()),
        )
    }

    /// `player.wait` on a game driven directly: stepped pacing, the run inline (time.md, `step`
    /// with `until: "decision"` unless the request says otherwise). Without a wall clock its runs
    /// stop at their ticks, `until`, the episode's end or a halt. Real time is the game thread's.
    fn player_wait(&mut self, cmd: &Command) -> Result<Value, Problem> {
        if !crate::player::declared(self.sim.world()) {
            return Err(crate::player::no_layer("player.wait"));
        }
        let step = crate::player::wait_as_step(&cmd.params)?;
        if !matches!(self.player.pacing, PlayPacing::Stepped) {
            return Err(catalog::thread_only("player.wait in real time"));
        }
        let who = crate::player::caller(self.sim.world(), cmd.source)?;
        let wall = self.wall.clone();
        let mut ctl = self.take_player();
        let r = pocket_interface::time::session::step(
            &mut GameTicker { game: self },
            &mut ctl,
            &who,
            &step,
            &mut || wall.as_ref().map_or(0.0, |c| c()),
        );
        self.player = ctl;
        self.with_hash(cmd.source, r?)
    }

    /// Swaps the world's program for a bundle this game has run or prepared, as `scripts.apply`
    /// does: a `scripts.swap` Host write at this boundary, recorded with the bundle (hot-update.md
    /// 5). The game thread's restore uses it to put the applied scripts back on a world restored
    /// from a snapshot kept with older ones.
    pub fn swap_bundle(&mut self, hash: ContentHash) -> Result<Value, Problem> {
        let swap = Command::new(
            Source::Host,
            self.next_host_seq(),
            "scripts.swap",
            json!({"bundle": hash.to_hex()}),
        );
        self.apply(&swap)
    }

    /// Restores a snapshot of this game's run whatever bundle it names, swapping to that bundle
    /// first when the world runs another (a kept snapshot from before a hot update).
    pub fn restore_any(&mut self, snap: &Snapshot) -> Result<(), Problem> {
        let h = snap.header().bundle;
        if h == self.current {
            return Game::restore(self, snap);
        }
        let rec = self
            .bundle_of(&h)
            .ok_or_else(|| pocket_persist::error::bundle_unavailable(&h.to_hex(), "not kept"))?;
        <Game as Stepper>::restore(self, snap, &rec)
    }

    /// Runs one tick; the recorder takes its hash. A fault poisons the world and ends the
    /// recording's segment (replay.md 2.3); a step on a poisoned world is refused with
    /// `sim.world_poisoned` and records nothing, so one fault is one `Fault` record.
    pub fn step(&mut self) -> Result<StepReport, Problem> {
        self.step_with(&mut NoHooks)
    }

    /// [`Game::step`] with the simulation's step hooks (a profiler timing its systems; the
    /// hooks stand outside the tick and change nothing in it).
    pub fn step_with(
        &mut self,
        hooks: &mut dyn pocket_sim::StepHooks,
    ) -> Result<StepReport, Problem> {
        if let Some(p) = self.sim.poisoned() {
            return Err(pocket_sim::sim::world_poisoned(p.tick));
        }
        let stepped = self.sim.step(hooks);
        // A script debugger that evaluated inside the tick taints the run (script-host.md 13).
        if let Some((tick, why)) = pocket_script::debug::take_taint(self.sim.world_mut()) {
            self.tainted.get_or_insert(tick);
            if let Some(rec) = &mut self.recorder {
                rec.tainted(Tick(tick), &why);
            }
        }
        match stepped {
            Ok(r) => {
                self.writes = 0;
                self.sync_context();
                if let Some(rec) = &mut self.recorder {
                    rec.end_tick(self.sim.world())?;
                }
                Ok(r)
            }
            Err(p) => {
                if let (Some(rec), Some(poison)) = (&mut self.recorder, self.sim.poisoned()) {
                    let system = p
                        .detail
                        .get("system")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    rec.fault(poison.tick, &system, &p.code);
                }
                Err(p)
            }
        }
    }

    /// Runs `ticks` ticks.
    pub fn run(&mut self, ticks: u64) -> Result<(), Problem> {
        for _ in 0..ticks {
            self.step()?;
        }
        Ok(())
    }

    /// A new game at this game's state: a fresh world with the same scripts and systems, restored
    /// from this one's snapshot (persistence.md 6.3). The fork records nothing.
    pub fn fork(&self) -> Result<Game, Problem> {
        let snap = self.snapshot()?;
        let set = self.bundles.get(&self.current).cloned().ok_or_else(|| {
            pocket_persist::error::bundle_unavailable(&self.current.to_hex(), "not kept")
        })?;
        let mut g = Game::make(self.setup.clone(), 1, self.extras.clone(), &set)?;
        g.bundles = self.bundles.clone();
        g.host_seq = self.host_seq;
        g.project = self.project.clone();
        g.restore(&snap)?;
        g.player = self.player.clone();
        g.wall = self.wall.clone();
        Ok(g)
    }

    /// Replaces the world with the snapshot's, keeping the scripts (the snapshot must name the
    /// bundle the world runs). While recording, the open segment ends with the world as it was
    /// (unless a fault already closed it) and a `Rebase` opens the next one with the restored world
    /// (replay.md 2.5); a refused restore rebases onto the world it left unchanged, so the
    /// recording goes on either way.
    pub fn restore(&mut self, snap: &Snapshot) -> Result<(), Problem> {
        let poisoned = self.sim.poisoned().is_some();
        if let Some(rec) = &mut self.recorder
            && !poisoned
        {
            rec.end_segment(self.sim.world())?;
        }
        if poisoned {
            // A tick stopped halfway (simulation.md 4.6): its bookkeeping is Derived, so clearing
            // it first lets the restore replace the world.
            pocket_sim::persisted::rebuild_derived(self.sim.world_mut());
        }
        let r = restore(
            self.sim.world_mut(),
            snap,
            &self.reg,
            RestoreOptions::default(),
        );
        if r.is_ok() {
            self.writes = snap.header().writes;
            self.sync_context();
            self.history.clear();
        }
        self.generation += 1;
        if let Some(rec) = &mut self.recorder
            && (r.is_ok() || self.sim.poisoned().is_none())
        {
            let bundle = bundle_record(&self.bundles[&self.current]);
            rec.rebase(self.sim.world(), RebaseCause::Restore, &bundle)?;
        }
        r.map(|_| ())
    }

    /// Starts recording into `sink` (replay.md 2.3): the start snapshot, its formats and the
    /// bundle, then every applied write and every tick's hash.
    pub fn record(
        &mut self,
        sink: Box<dyn RecordSink>,
        opts: RecordOptions,
    ) -> Result<(), Problem> {
        let set = &self.bundles[&self.current];
        let rec = Recorder::start(
            self.sim.world(),
            self.reg.clone(),
            &bundle_record(set),
            &self.run_config(),
            opts,
            sink,
        )?;
        self.recorder = Some(rec);
        Ok(())
    }

    /// Ends the recording, if one runs.
    pub fn finish_recording(&mut self) -> Result<Option<ReplaySummary>, Problem> {
        match self.recorder.take() {
            Some(r) => r.finish(self.sim.world()).map(Some),
            None => Ok(None),
        }
    }

    /// A game to replay `replay` with (replay.md 3.4): no entities, the recording's tick rate, and
    /// the bundle its first segment starts with, built from the modules the replay embeds; `verify`
    /// restores the start snapshot into it.
    pub fn for_replay(replay: &Replay) -> Result<Game, Problem> {
        let start = replay.start(0).ok_or_else(|| {
            pocket_persist::error::bundle_unavailable("", "the replay has no start snapshot")
        })?;
        let h = start.header().bundle;
        let rec = replay.bundle(&h).ok_or_else(|| {
            pocket_persist::error::bundle_unavailable(&h.to_hex(), "the replay does not embed it")
        })?;
        let set = compiled_from_record(rec)?;
        let setup = GameSetup {
            name: "replay".to_owned(),
            rate: TickRate::new(replay.header().tick_rate)?,
            scene: Scene::empty(),
            scripts: set.clone(),
            source: None,
            limits: ScriptLimits::default(),
            lint_off: false,
            // The start snapshot carries the player declarations, if any (`crate::player`).
            player: None,
        };
        GameBuilder::new(Arc::new(setup)).build_empty(&set)
    }

    /// The bundle record of a bundle this game has.
    fn bundle_of(&self, h: &ContentHash) -> Option<BundleRecord> {
        self.bundles.get(h).map(bundle_record)
    }
}

/// The run configuration of a game with `limits` (versions.md 3.7).
pub fn run_config(limits: &ScriptLimits) -> String {
    canonical_json(&json!({
        "max_call_depth": limits.max_call_depth,
        "steps_per_call": limits.steps_per_call,
        "steps_per_system": limits.steps_per_system,
        "steps_per_tick": limits.steps_per_tick,
    }))
}

/// The game as a player's run steps it (pocket-interface's `Ticker`): its ticks through
/// [`Game::step`], recorded when recording.
struct GameTicker<'g> {
    game: &'g mut Game,
}

impl pocket_interface::time::session::Ticker for GameTicker<'_> {
    fn world(&self) -> &World {
        self.game.sim.world()
    }

    fn tick(&mut self) -> Result<StepReport, Problem> {
        self.game.step()
    }

    fn act(
        &mut self,
        _caller: &pocket_interface::action::Caller,
        _raw: &Value,
    ) -> Result<pocket_interface::action::ActDone, Problem> {
        // Only lockstep queues acts for a later boundary, and `player.pacing` refuses lockstep.
        Err(pocket_interface::action::state::internal(
            "player",
            "a queued lockstep act in a game without lockstep",
        ))
    }
}

/// What a successful write gives the recorder and the history.
struct Written {
    result: Value,
    canonical: Value,
    /// The name it is recorded under; empty: the command's own.
    recorded: &'static str,
    history: Option<HistoryStep>,
}

enum HistoryStep {
    New(String, Vec<Edit>),
    Undone(Entry),
    Redone(Entry),
}

impl Stepper for Game {
    fn world(&self) -> &World {
        self.sim.world()
    }

    fn world_mut(&mut self) -> &mut World {
        self.sim.world_mut()
    }

    fn registry(&self) -> &Registry {
        &self.reg
    }

    fn restore(&mut self, snap: &Snapshot, bundle: &BundleRecord) -> Result<(), Problem> {
        if bundle.hash != self.current {
            let set = match self.bundles.get(&bundle.hash) {
                Some(s) => s.clone(),
                None => compiled_from_record(bundle)?,
            };
            pocket_script::swap(self.sim.world_mut(), &set, true)
                .map_err(|e| scripts::refused("restore", &e))?;
            self.current = bundle.hash;
            self.bundles.insert(bundle.hash, set);
        }
        Game::restore(self, snap)
    }

    fn apply(&mut self, write: &RecordedWrite, source: &dyn ReplaySource) -> Result<(), Problem> {
        let cmd = Command::new(write.source, write.seq, &write.name, write.params_json());
        self.apply_from(&cmd, source, true).map(|_| ())
    }

    fn step(&mut self) -> Result<Vec<Event>, Problem> {
        Game::step(self).map(|r| r.events)
    }

    fn run_config(&self) -> String {
        Game::run_config(self)
    }

    fn find_bundle(&self, h: &ContentHash) -> Option<BundleRecord> {
        self.bundle_of(h)
    }
}
