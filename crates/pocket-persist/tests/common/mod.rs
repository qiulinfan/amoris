//! A small game for the persistence tests, built on `pocket-sim` alone: moving bodies whose
//! velocities wander by the RNG, spawns and despawns, a tag component that comes and goes, a score
//! resource with events, and a cache of its own (written with infinities, as a library's state may
//! be). Writes arrive as recorded commands, so the game is a `Stepper`.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use bevy_ecs::prelude::*;
use pocket_contract::{Problem, detail};
use pocket_persist::pce;
use pocket_persist::replay::{
    Applied, BundleRecord, MemorySink, RecordOptions, RecordedWrite, Recorder, ReplaySource,
    Source, Stepper,
};
use pocket_persist::{Registry, RestoreOptions, Snapshot, restore, sim_registry};
use pocket_sim::order::by_id_mut;
use pocket_sim::{
    ContentHash, EntityId, Event, EventKind, NewEvent, NoHooks, Persisted, PersistedCache,
    PlainData, RegisterPersisted, RngTable, RunCondition, Sim, SimClock, SimConfig, Staged, Tick,
    TickPhase, TickRate, entity, math,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pos {
    pub x: f64,
    pub y: f64,
}

#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Vel {
    pub x: f64,
    pub y: f64,
}

/// Comes and goes with writes, so its section appears and disappears.
#[derive(Component, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tag(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mood {
    Calm,
    Gust { strength: u8 },
}

/// A component with a map, an option, a nested enum and a reference to another entity.
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bag {
    pub items: BTreeMap<String, u32>,
    pub owner: Option<EntityId>,
    pub mood: Mood,
}

#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Score(pub i64);

/// A cache: a library's state across ticks, as opaque bytes that may hold infinities.
#[derive(Resource, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Counter {
    pub hits: u64,
    pub wave: f64,
    pub limit: f64,
}

macro_rules! persisted {
    ($($t:ty => $name:literal),* $(,)?) => {
        $(impl Persisted for $t {
            const NAME: &'static str = $name;
            const VERSION: u32 = 1;
        })*
    };
}

persisted! { Pos => "Pos", Vel => "Vel", Tag => "Tag", Score => "Score" }

impl Persisted for Bag {
    const NAME: &'static str = "Bag";
    const VERSION: u32 = 1;
    fn trace(
        t: &mut serde_reflection::Tracer,
        s: &serde_reflection::Samples,
    ) -> serde_reflection::Result<()> {
        t.trace_type::<Mood>(s)?;
        t.trace_type::<Self>(s).map(|_| ())
    }
}

struct StagedCounter(Option<Counter>);

impl Staged for StagedCounter {
    fn apply(self: Box<Self>, world: &mut World) {
        match self.0 {
            Some(c) => world.insert_resource(c),
            None => {
                world.remove_resource::<Counter>();
            }
        }
    }
}

impl PersistedCache for Counter {
    const NAME: &'static str = "test.counter";
    fn identity() -> &'static str {
        "test counter layout 1"
    }
    fn encode(world: &World, out: &mut Vec<u8>) -> Result<(), Problem> {
        if let Some(c) = world.get_resource::<Counter>() {
            pce::encode_into(c, out, false)
                .map_err(|e| Problem::new("persist.encode", e.reason(), detail([])))?;
        }
        Ok(())
    }
    fn decode(bytes: &[u8]) -> Result<Box<dyn Staged>, Problem> {
        if bytes.is_empty() {
            return Ok(Box::new(StagedCounter(None)));
        }
        let c: Counter = pce::from_bytes(bytes, false).map_err(|e| e.problem(0))?;
        Ok(Box::new(StagedCounter(Some(c))))
    }
    fn rebuild(world: &mut World) -> Result<(), Problem> {
        let hits = world.resource::<SimClock>().tick.0;
        world.insert_resource(Counter {
            hits,
            wave: 0.0,
            limit: f64::INFINITY,
        });
        Ok(())
    }
}

/// The test game's declaration.
pub fn declare<R: RegisterPersisted>(r: &mut R) {
    r.component::<Pos>()
        .component::<Vel>()
        .component::<Tag>()
        .component::<Bag>()
        .resource::<Score>()
        .cache::<Counter>();
}

/// The registry: pocket-sim's types and the test game's, built once.
pub fn registry() -> Arc<Registry> {
    static REG: OnceLock<Arc<Registry>> = OnceLock::new();
    REG.get_or_init(|| {
        let mut b = sim_registry();
        declare(&mut b);
        Arc::new(b.build().expect("the test registry builds"))
    })
    .clone()
}

fn wander(mut q: Query<(&EntityId, &mut Vel)>, mut rng: ResMut<RngTable>) {
    for (id, mut v) in by_id_mut(&mut q) {
        let s = rng.entity("test.wander", *id).expect("in a tick");
        v.x += s.range(-0.5, 0.5).expect("a range");
        v.y += s.range(-0.5, 0.5).expect("a range");
    }
}

fn movement(mut q: Query<(&EntityId, &Vel, &mut Pos)>, clock: Res<SimClock>) {
    let dt = clock.dt();
    for (_, v, mut p) in by_id_mut(&mut q) {
        p.x += v.x * dt;
        p.y += v.y * dt;
    }
}

/// Every 7 ticks a body spawns where the RNG says; every 11 ticks the oldest of more than four
/// goes; every tick the score adds the bodies and the cache counts.
fn life(world: &mut World, ctx: &mut pocket_sim::SystemCtx<'_>) {
    let tick = ctx.tick().0;
    if tick % 7 == 0 {
        let (x, y) = {
            let mut rng = world.resource_mut::<RngTable>();
            let s = rng.system(ctx.key().as_str()).expect("in a tick");
            (
                s.range(-10.0, 10.0).expect("range"),
                s.range(-10.0, 10.0).expect("range"),
            )
        };
        pocket_sim::entity::spawn(world, (Pos { x, y }, Vel { x: 0.0, y: 0.0 })).expect("spawn");
    }
    let ids: Vec<EntityId> = world.resource::<pocket_sim::EntityIndex>().ids().collect();
    if tick % 11 == 0 && ids.len() > 4 {
        pocket_sim::entity::despawn(world, ids[0]).expect("despawn");
    }
    let n = world.resource::<pocket_sim::EntityIndex>().len() as i64;
    let score = {
        let mut s = world.resource_mut::<Score>();
        s.0 += n;
        s.0
    };
    #[allow(clippy::cast_precision_loss)]
    let data =
        PlainData::object(vec![("score".into(), PlainData::Number(score as f64))]).expect("data");
    pocket_sim::event::emit(
        world,
        NewEvent::new(EventKind::new("test.scored").expect("kind")).data(data),
    );
    if let Some(mut c) = world.get_resource_mut::<Counter>() {
        c.hits += 1;
        #[allow(clippy::cast_precision_loss)]
        let t = tick as f64;
        c.wave = math::sin(t * 0.1);
    }
}

/// A game at tick 0 with three bodies.
pub fn sim(seed: u64) -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed,
    })
    .expect("sim");
    sim.add_system(
        "test.wander",
        TickPhase::Update,
        RunCondition::Always,
        wander,
    )
    .expect("system");
    sim.add_system(
        "test.move",
        TickPhase::Update,
        RunCondition::Always,
        movement,
    )
    .expect("system");
    sim.add_exclusive("test.life", TickPhase::Finish, RunCondition::Always, life)
        .expect("system");
    let w = sim.world_mut();
    w.insert_resource(Score(0));
    w.insert_resource(Counter {
        hits: 0,
        wave: 0.0,
        limit: f64::INFINITY,
    });
    let mut b = sim.boundary();
    for i in 0..3 {
        let f = f64::from(i);
        b.spawn((Pos { x: f, y: -f }, Vel { x: 1.0, y: 0.5 * f }))
            .expect("spawn");
    }
    sim
}

pub struct Game {
    pub sim: Sim,
    pub reg: Arc<Registry>,
}

pub fn game(seed: u64) -> Game {
    Game {
        sim: sim(seed),
        reg: registry(),
    }
}

fn refused(code: &str, why: &str) -> Problem {
    Problem::new(code, why, detail([]))
}

fn id_of(p: &Value) -> Result<EntityId, Problem> {
    p["id"]
        .as_u64()
        .and_then(EntityId::new)
        .ok_or_else(|| refused("request.wrong_type", "id must be an entity id"))
}

fn num(p: &Value, k: &str) -> Result<f64, Problem> {
    p[k].as_f64()
        .ok_or_else(|| refused("request.wrong_type", "a number is missing"))
}

/// The test's content store: data files by hash.
pub fn content() -> (&'static str, ContentHash, &'static [u8]) {
    const BYTES: &[u8] = &[3, 1, 4, 1, 5];
    (
        "levels/reef.bin",
        ContentHash::derive("Pocket3D 2026-10-03 test data v1", BYTES),
        BYTES,
    )
}

/// The game's commands: `push {id, dx, dy}`, `spawn {x, y}`, `despawn {id}`, `tag {id, n}`,
/// `untag {id}`, `bag {id, item, count}`, `load {hash}` (a body per byte of a data file).
pub fn apply(sim: &mut Sim, name: &str, p: &Value) -> Result<(), Problem> {
    let (_, hash, bytes) = content();
    apply_src(sim, name, p, &|h| (*h == hash).then(|| Arc::from(bytes)))
}

/// [`apply`] with the data files found through `data`.
pub fn apply_src(
    sim: &mut Sim,
    name: &str,
    p: &Value,
    data: &dyn Fn(&ContentHash) -> Option<Arc<[u8]>>,
) -> Result<(), Problem> {
    if name == "load" {
        let s = p["hash"].as_str().unwrap_or("");
        let h = ContentHash::from_hex(s).ok_or_else(|| refused("request.wrong_type", "hash"))?;
        let bytes = data(&h)
            .ok_or_else(|| pocket_persist::error::data_unavailable(s, "levels/reef.bin"))?;
        let mut b = sim.boundary();
        for x in bytes.iter() {
            b.spawn((
                Pos {
                    x: f64::from(*x),
                    y: 0.0,
                },
                Vel { x: 0.0, y: 0.0 },
            ))?;
        }
        return Ok(());
    }
    let mut b = sim.boundary();
    let live = |b: &pocket_sim::Boundary<'_>, id| {
        b.entity(id)
            .ok_or_else(|| refused("sim.entity_not_found", "no such entity"))
    };
    match name {
        "push" => {
            let e = live(&b, id_of(p)?)?;
            let (dx, dy) = (num(p, "dx")?, num(p, "dy")?);
            let mut v = b
                .world_mut()
                .get_mut::<Vel>(e)
                .ok_or_else(|| refused("test.no_vel", "no velocity"))?;
            v.x += dx;
            v.y += dy;
        }
        "spawn" => {
            b.spawn((
                Pos {
                    x: num(p, "x")?,
                    y: num(p, "y")?,
                },
                Vel { x: 0.0, y: 0.0 },
            ))?;
        }
        "despawn" => {
            b.despawn(id_of(p)?)?;
        }
        "tag" => {
            let e = live(&b, id_of(p)?)?;
            let n = p["n"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0);
            b.world_mut().entity_mut(e).insert(Tag(n));
        }
        "untag" => {
            let e = live(&b, id_of(p)?)?;
            b.world_mut().entity_mut(e).remove::<Tag>();
        }
        "bag" => {
            let e = live(&b, id_of(p)?)?;
            let item = p["item"].as_str().unwrap_or("").to_owned();
            let count = p["count"]
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(0);
            let mut w = b.world_mut().entity_mut(e);
            let mut bag = w.get::<Bag>().cloned().unwrap_or(Bag {
                items: BTreeMap::new(),
                owner: None,
                mood: Mood::Calm,
            });
            bag.items.insert(item, count);
            bag.mood = Mood::Gust {
                strength: u8::try_from(count % 200).unwrap_or(0),
            };
            w.insert(bag);
        }
        other => return Err(refused("command.unknown", other)),
    }
    Ok(())
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
    fn restore(&mut self, snap: &Snapshot, _bundle: &BundleRecord) -> Result<(), Problem> {
        restore(
            self.sim.world_mut(),
            snap,
            &self.reg,
            RestoreOptions::default(),
        )
        .map(drop)
    }
    fn apply(&mut self, write: &RecordedWrite, src: &dyn ReplaySource) -> Result<(), Problem> {
        apply_src(&mut self.sim, &write.name, &write.params_json(), &|h| {
            src.data(h)
        })
    }
    fn step(&mut self) -> Result<Vec<Event>, Problem> {
        self.sim.step(&mut NoHooks).map(|r| r.events)
    }
    fn run_config(&self) -> String {
        "{}".to_owned()
    }
}

/// An input: a command for the boundary before `tick`.
#[derive(Clone, Debug)]
pub struct Input {
    pub tick: u64,
    pub name: &'static str,
    pub params: Value,
}

pub fn input(tick: u64, name: &'static str, params: Value) -> Input {
    Input { tick, name, params }
}

/// Runs `game` to `until`, applying each input at the boundary before its tick and handing what
/// it applies to the recorder.
pub fn run(game: &mut Game, rec: Option<&mut Recorder>, inputs: &[Input], until: u64) {
    let mut rec = rec;
    let mut seq = 0;
    while game.sim.clock().tick.0 < until {
        let t = game.sim.clock().tick.0 + 1;
        let mut index = 0;
        for i in inputs.iter().filter(|i| i.tick == t) {
            seq += 1;
            let a = Applied {
                tick: Tick(t),
                index,
                source: Source::Player(0),
                seq,
                name: i.name.to_owned(),
                params: i.params.clone(),
            };
            if i.name == "load"
                && let Some(r) = rec.as_deref_mut()
            {
                let (path, hash, bytes) = content();
                r.data_read(path, hash, bytes);
            }
            match apply(&mut game.sim, i.name, &i.params) {
                Ok(()) => {
                    index += 1;
                    if let Some(r) = rec.as_deref_mut() {
                        r.applied(&a);
                    }
                }
                Err(e) => {
                    if let Some(r) = rec.as_deref_mut() {
                        r.refused(&a, &e.code);
                    }
                }
            }
        }
        game.sim.step(&mut NoHooks).expect("step");
        if let Some(r) = rec.as_deref_mut() {
            r.end_tick(game.sim.world()).expect("end tick");
        }
    }
}

/// Records a run of `game` to `until` and returns the replay's bytes.
pub fn record(game: &mut Game, inputs: &[Input], until: u64, opts: RecordOptions) -> Vec<u8> {
    let sink = MemorySink::new();
    let mut rec = Recorder::start(
        game.sim.world(),
        game.reg.clone(),
        &BundleRecord::empty(),
        "{}",
        opts,
        Box::new(sink.clone()),
    )
    .expect("start");
    run(game, Some(&mut rec), inputs, until);
    rec.finish(game.sim.world()).expect("finish");
    sink.bytes()
}

/// A standard set of inputs over a run.
pub fn inputs() -> Vec<Input> {
    vec![
        input(5, "push", json!({"id": 1, "dx": 2.0, "dy": -1.0})),
        input(9, "tag", json!({"id": 2, "n": 7})),
        input(12, "bag", json!({"id": 3, "item": "rope", "count": 4})),
        input(20, "spawn", json!({"x": 3.5, "y": -0.0})),
        input(26, "untag", json!({"id": 2})),
        input(31, "push", json!({"id": 2, "dx": -0.25, "dy": 0.125})),
        input(40, "tag", json!({"id": 3, "n": 1})),
        input(44, "despawn", json!({"id": 3})),
    ]
}

/// The entity a world holds for `id`.
pub fn ent(sim: &Sim, id: u64) -> Option<Entity> {
    entity::entity(sim.world(), EntityId::new(id)?)
}
