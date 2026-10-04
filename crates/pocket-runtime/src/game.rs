//! The game (docs/spec/architecture.md 4.8; threads.md 3.1 and 5): one world with physics and the
//! project's scripts, built from a [`GameSetup`], driven synchronously. Every change from outside a
//! tick is a command applied at a boundary through [`Game::apply`]: the catalog says what kind it
//! is, a Write that succeeds is handed to the recorder in its canonical form as an input of the next
//! tick, and a refused one changes nothing. The headless and batch forms and every check drive a
//! `Game` directly; the game thread wraps one (`thread`).

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use bevy_ecs::prelude::World;
use pocket_contract::{Problem, detail};
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
use pocket_sim::{ContentHash, Event, NoHooks, Sim, SimConfig, StepReport, Tick, TickRate};
use serde_json::{Value, json};

use crate::catalog::{self, Command, CommandFn};
use crate::edit::{self, WorldEditParams, WorldGetParams};
use crate::project::GameSetup;
use crate::scene::Scene;
use crate::scripts::{
    self, ScriptsApplyParams, ScriptsSwapParams, bundle_record, compiled_from_record,
};
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
}

impl GameBuilder {
    pub fn new(setup: Arc<GameSetup>) -> GameBuilder {
        GameBuilder {
            setup,
            seed: 1,
            extras: Extras::default(),
        }
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
        let scene = g.setup.scene.clone();
        scene.spawn_into(&mut g.sim.boundary())?;
        Ok(g)
    }

    /// The game with no entities, running `set`: what a restore or a replay fills.
    pub fn build_empty(self, set: &CompiledSet) -> Result<Game, Problem> {
        Game::make(self.setup, self.seed, self.extras, set)
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
        Ok(Game {
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
        })
    }

    /// Hands the render feed what changed in the visual state (presentation only; see
    /// `present`).
    pub fn present(&mut self, extractor: &mut crate::present::Extractor, feed: &pocket_assets::Feed) {
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

    /// The bundle the world runs.
    pub fn bundle(&self) -> ContentHash {
        self.current
    }

    /// Writes applied at the current boundary.
    pub fn writes(&self) -> u32 {
        self.writes
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
        let kind = match catalog::find(&cmd.name) {
            Some(def) => {
                if def.host_only && cmd.source != Source::Host && !replaying {
                    return Err(catalog::host_only(def.name, cmd.source));
                }
                def.kind
            }
            None => match self.extras.commands.iter().find(|(n, _, _)| *n == cmd.name) {
                Some((_, kind, _)) => *kind,
                None => return Err(catalog::unknown(&cmd.name, &self.extras.commands)),
            },
        };
        match kind {
            Kind::Read => self.read(cmd),
            Kind::Write => self.write(cmd, src),
            Kind::Request => self.scripts_apply(cmd),
            Kind::Control => self.control(cmd),
        }
    }

    fn read(&mut self, cmd: &Command) -> Result<Value, Problem> {
        match cmd.name.as_str() {
            "world_get" => {
                let p: WorldGetParams = decode(&cmd.params, "world_get")?;
                edit::world_get(self.sim.world(), &p)
            }
            "status" => {
                decode::<NoParams>(&cmd.params, "status")?;
                let hash = self.world_hash().ok().map(|h| h.to_string());
                let poisoned = self.sim.poisoned().map(|p| p.tick.0);
                Ok(
                    json!({"tick": self.tick().0, "writes": self.writes, "world_hash": hash,
                          "bundle": self.current.to_hex(), "poisoned": poisoned}),
                )
            }
            "snapshot" => {
                decode::<NoParams>(&cmd.params, "snapshot")?;
                let s = self.snapshot()?;
                Ok(pocket_link::ReplyValue::Snapshot(s).into_json())
            }
            "scripts.status" => {
                decode::<NoParams>(&cmd.params, "scripts.status")?;
                let systems: Vec<String> = pocket_script::scripts::last_tick(self.sim.world())
                    .into_iter()
                    .map(|(n, _)| n)
                    .collect();
                Ok(json!({"bundle": self.current.to_hex(), "ran_last_tick": systems}))
            }
            other => {
                let (_, _, f) = self
                    .extras
                    .commands
                    .iter()
                    .find(|(n, _, _)| n == other)
                    .ok_or_else(|| catalog::unknown(other, &self.extras.commands))?;
                let f = f.clone();
                let mark = self.sim.boundary().mark();
                let r = f(&mut self.sim.boundary(), &cmd.params).map(|(v, _)| v);
                self.sim.boundary().rollback(mark);
                r
            }
        }
    }

    fn write(&mut self, cmd: &Command, src: &dyn ReplaySource) -> Result<Value, Problem> {
        let tick = self.tick();
        let mark = self.sim.boundary().mark();
        let outcome = match cmd.name.as_str() {
            "world_edit" => decode::<WorldEditParams>(&cmd.params, "world_edit")
                .and_then(|p| edit::world_edit(&mut self.sim.boundary(), &p)),
            "scripts.swap" => self.swap(&cmd.params, src),
            other => match self.extras.commands.iter().find(|(n, _, _)| n == other) {
                Some((_, _, f)) => {
                    let f = f.clone();
                    f(&mut self.sim.boundary(), &cmd.params)
                }
                None => Err(catalog::unknown(other, &self.extras.commands)),
            },
        };
        let applied = |params: Value, index: u32| Applied {
            tick: Tick(tick.0 + 1),
            index,
            source: cmd.source,
            seq: cmd.seq,
            name: cmd.name.clone(),
            params,
        };
        match outcome {
            Ok((result, canonical)) => {
                let a = applied(canonical, self.writes);
                self.writes += 1;
                self.sync_context();
                if let Some(r) = &mut self.recorder {
                    r.applied(&a);
                }
                Ok(result)
            }
            Err(p) => {
                self.sim.boundary().rollback(mark);
                if let Some(r) = &mut self.recorder {
                    r.refused(&applied(cmd.params.clone(), self.writes), &p.code);
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

    /// `scripts.apply` (hot-update.md 4), its stages inline (threads.md 5.5): gather and compile
    /// (the project's own TypeScript unless `files` names others), answer `unchanged` for the same
    /// bundle unless `force`, then apply the `scripts.swap` Host write it produces.
    fn scripts_apply(&mut self, cmd: &Command) -> Result<Value, Problem> {
        let p: ScriptsApplyParams = decode(&cmd.params, "scripts.apply")?;
        let set = self.gather(&p)?;
        let hash = set.bundle.hash;
        if hash == self.current && !p.force {
            return Ok(json!({"outcome": "unchanged", "bundle": hash.to_hex()}));
        }
        if p.dry_run {
            return Ok(json!({"outcome": "dry_run", "bundle": hash.to_hex()}));
        }
        self.bundles.insert(hash, set);
        let swap = Command::new(
            Source::Host,
            self.next_host_seq(),
            "scripts.swap",
            json!({"bundle": hash.to_hex()}),
        );
        self.apply(&swap)
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

    fn control(&mut self, cmd: &Command) -> Result<Value, Problem> {
        match cmd.name.as_str() {
            "step" => {
                let p: StepParams = decode(&cmd.params, "step")?;
                for _ in 0..p.ticks {
                    self.step()?;
                }
                Ok(json!({"tick": self.tick().0, "world_hash": self.world_hash()?.to_string()}))
            }
            other => Err(Problem::new(
                "time.wrong_mode",
                format!("{other} paces the game thread; a game driven directly has no pacing."),
                detail([("request", json!(other))]),
            )),
        }
    }

    /// Runs one tick; the recorder takes its hash. A fault poisons the world and ends the
    /// recording's segment (replay.md 2.3); a step on a poisoned world is refused with
    /// `sim.world_poisoned` and records nothing, so one fault is one `Fault` record.
    pub fn step(&mut self) -> Result<StepReport, Problem> {
        if let Some(p) = self.sim.poisoned() {
            return Err(pocket_sim::sim::world_poisoned(p.tick));
        }
        match self.sim.step(&mut NoHooks) {
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
        g.restore(&snap)?;
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

/// A command that takes no parameters.
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoParams {}

/// `step`'s parameters.
#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepParams {
    /// Ticks to run, at least 1.
    #[serde(default = "one")]
    pub ticks: u64,
}

fn one() -> u64 {
    1
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
