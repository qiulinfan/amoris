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

use crate::catalog::{self, Command, CommandFn, NoParams};
use crate::control::{StepParams, StepStop};
use crate::edit::{self, Edit, WorldEditOps, WorldEditParams, WorldGetParams, label_of};
use crate::files::{self, ScriptPathParams, ScriptWriteParams};
use crate::history::{Entry, History};
use crate::inspect::{self, WorldQueryParams, WorldSchemaParams, WorldTreeParams};
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
            project: None,
            history: History::default(),
            diagnostics: Vec::new(),
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
        let (name, kind) = match catalog::find(&cmd.name) {
            Some(def) => {
                if def.host_only && cmd.source != Source::Host && !replaying {
                    return Err(catalog::host_only(def.name, cmd.source));
                }
                if def.thread {
                    return Err(catalog::thread_only(def.name));
                }
                (def.name, def.kind)
            }
            None => match self.extras.commands.iter().find(|(n, _, _)| *n == cmd.name) {
                Some((_, kind, _)) => ("", *kind),
                None => return Err(catalog::unknown(&cmd.name, &self.extras.commands)),
            },
        };
        match kind {
            Kind::Read => self.read(cmd, name),
            Kind::Write => self.write(cmd, name, src),
            Kind::Request => self.request(cmd, name),
            Kind::Control => self.control(cmd, name),
        }
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
                let p: ScriptPathParams = decode(p, name)?;
                files::read(self.root(name)?, &p)
            }
            "status" => {
                decode::<NoParams>(p, name)?;
                let hash = self.world_hash().ok().map(|h| h.to_string());
                let poisoned = self.sim.poisoned().map(|p| p.tick.0);
                let entities = self.sim.world().resource::<pocket_sim::EntityIndex>().len();
                Ok(
                    json!({"tick": self.tick().0, "writes": self.writes, "world_hash": hash,
                          "bundle": self.current.to_hex(), "poisoned": poisoned,
                          "entities": entities}),
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
    /// fails answers `scripts.refused` with `diagnostics` in its detail.
    fn scripts_apply(&mut self, cmd: &Command) -> Result<Value, Problem> {
        let p: ScriptsApplyParams = decode(&cmd.params, "scripts.apply")?;
        let set = match self.gather(&p) {
            Ok(s) => s,
            Err(mut e) => {
                self.diagnostics = files::diagnostics(&e);
                e.detail
                    .insert("diagnostics".into(), json!(self.diagnostics));
                return Err(e);
            }
        };
        self.diagnostics.clear();
        let hash = set.bundle.hash;
        if hash == self.current && !p.force {
            return Ok(json!({"outcome": "unchanged", "bundle": hash.to_hex(), "diagnostics": []}));
        }
        if p.dry_run {
            return Ok(json!({"outcome": "dry_run", "bundle": hash.to_hex(), "diagnostics": []}));
        }
        self.bundles.insert(hash, set);
        let swap = Command::new(
            Source::Host,
            self.next_host_seq(),
            "scripts.swap",
            json!({"bundle": hash.to_hex()}),
        );
        let mut r = self.apply(&swap)?;
        r["diagnostics"] = json!([]);
        Ok(r)
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
                let mut stopped_by = None;
                for _ in 0..limit {
                    self.step()?;
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
                Ok(out)
            }
            other => Err(catalog::thread_only(other)),
        }
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
        g.project = self.project.clone();
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
