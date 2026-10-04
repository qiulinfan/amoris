//! A small deterministic world exercises the assembled foundation, including negative controls.
use bevy_ecs::prelude::*;
use pocket_contract::{CheckOptions, Problem, decode, detail};
use pocket_persist::replay::{BundleRecord, MemorySink, Record, RecordOptions};
use pocket_persist::{
    Applied, EngineVersion, Registry, Replay, ReplayMode, ReplayOutcome, ReplaySource,
    RestoreOptions, Snapshot, Stepper, VerifyOptions, fork_into, restore, sim_registry, snapshot,
    verify, world_hash,
};
use pocket_sim::{
    EntityId, Event, NoHooks, Persisted, RegisterPersisted, RngTable, RunCondition, Sim, SimClock,
    SimConfig, Tick, TickPhase, TickRate,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Component, Clone, Copy, Serialize, Deserialize)]
struct Position {
    x: f64,
    y: f64,
}
#[derive(Component, Clone, Copy, Serialize, Deserialize)]
struct Velocity {
    x: f64,
    y: f64,
}
impl Persisted for Position {
    const NAME: &'static str = "Position";
    const VERSION: u32 = 1;
}
impl Persisted for Velocity {
    const NAME: &'static str = "Velocity";
    const VERSION: u32 = 1;
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Impulse {
    entity: u64,
    dx: f64,
    dy: f64,
}

struct WorldRun {
    sim: Sim,
    registry: Arc<Registry>,
}

impl WorldRun {
    fn new(seed: u64) -> Result<Self, Problem> {
        let mut builder = sim_registry();
        builder.component::<Position>().component::<Velocity>();
        let registry = Arc::new(builder.build()?);
        let mut sim = Sim::new(SimConfig {
            rate: TickRate::DEFAULT,
            seed,
        })?;
        sim.add_system(
            "foundation.motion",
            TickPhase::Update,
            RunCondition::Always,
            |mut query: Query<(&EntityId, &Velocity, &mut Position)>, clock: Res<SimClock>| {
                for (_, velocity, mut position) in pocket_sim::order::by_id_mut(&mut query) {
                    position.x += velocity.x * clock.dt();
                    position.y += velocity.y * clock.dt();
                }
            },
        )?;
        sim.add_system(
            "foundation.wander",
            TickPhase::Update,
            RunCondition::Always,
            |mut query: Query<(&EntityId, &mut Velocity)>, mut rng: ResMut<RngTable>| {
                for (id, mut velocity) in pocket_sim::order::by_id_mut(&mut query) {
                    let stream = rng.entity("foundation.wander", *id).expect("inside a tick");
                    velocity.x += stream.range(-0.1, 0.1).expect("finite range");
                    velocity.y += stream.range(-0.1, 0.1).expect("finite range");
                }
            },
        )?;
        for i in 0..3 {
            sim.boundary().spawn((
                Position {
                    x: f64::from(i),
                    y: 0.0,
                },
                Velocity {
                    x: 1.0,
                    y: f64::from(i) * 0.1,
                },
            ))?;
        }
        Ok(Self { sim, registry })
    }

    fn apply_impulse(&mut self, name: &str, params: &Value) -> Result<(), Problem> {
        if name != "impulse" {
            return Err(Problem::new("command.unknown", name, detail([])));
        }
        let input = decode::<Impulse>(params, &CheckOptions::new("impulse"))?.value;
        if !input.dx.is_finite() || !input.dy.is_finite() {
            return Err(Problem::new(
                "request.invalid_value",
                "impulse must be finite",
                detail([]),
            ));
        }
        let id = EntityId::new(input.entity).ok_or_else(|| {
            Problem::new("sim.entity_id_invalid", "invalid entity id", detail([]))
        })?;
        let mut boundary = self.sim.boundary();
        let entity = boundary
            .entity(id)
            .ok_or_else(|| Problem::new("sim.entity_not_found", "entity not found", detail([])))?;
        let mut velocity = boundary
            .world_mut()
            .get_mut::<Velocity>(entity)
            .ok_or_else(|| Problem::new("sim.component_missing", "velocity missing", detail([])))?;
        let x = velocity.x + input.dx;
        let y = velocity.y + input.dy;
        if !x.is_finite() || !y.is_finite() {
            return Err(Problem::new(
                "request.invalid_value",
                "velocity overflow",
                detail([]),
            ));
        }
        *velocity = Velocity { x, y };
        Ok(())
    }

    fn snap(&self) -> Result<Snapshot, Problem> {
        snapshot(self.sim.world(), &self.registry)
    }
}

impl Stepper for WorldRun {
    fn world(&self) -> &World {
        self.sim.world()
    }
    fn world_mut(&mut self) -> &mut World {
        self.sim.world_mut()
    }
    fn registry(&self) -> &Registry {
        &self.registry
    }
    fn restore(&mut self, snap: &Snapshot, _: &BundleRecord) -> Result<(), Problem> {
        restore(
            self.sim.world_mut(),
            snap,
            &self.registry,
            RestoreOptions::default(),
        )
        .map(drop)
    }
    fn apply(
        &mut self,
        write: &pocket_persist::RecordedWrite,
        _: &dyn ReplaySource,
    ) -> Result<(), Problem> {
        self.apply_impulse(&write.name, &write.params_json())
    }
    fn step(&mut self) -> Result<Vec<Event>, Problem> {
        self.sim.step(&mut NoHooks).map(|out| out.events)
    }
    fn run_config(&self) -> String {
        "{}".to_owned()
    }
}

#[derive(Serialize)]
pub struct Report {
    pub slice: &'static str,
    pub seed: u64,
    pub ticks: u64,
    pub entities: usize,
    pub world_hash: String,
    pub source: EngineVersion,
    pub checks: Vec<&'static str>,
    pub negative_divergence_tick: u64,
}

fn ensure(ok: bool, check: &str) -> Result<(), Problem> {
    if ok {
        Ok(())
    } else {
        Err(Problem::new("foundation.check_failed", check, detail([])))
    }
}

fn input_at(tick: u64, ticks: u64) -> Option<Value> {
    (tick == ticks / 3 || tick == ticks * 2 / 3)
        .then(|| json!({"entity": 1, "dx": 0.75, "dy": -0.25}))
}

fn advance(run: &mut WorldRun, until: u64, ticks: u64) -> Result<(), Problem> {
    while run.sim.clock().tick.0 < until {
        let next = run.sim.clock().tick.0 + 1;
        if let Some(params) = input_at(next, ticks) {
            run.apply_impulse("impulse", &params)?;
        }
        run.step()?;
    }
    Ok(())
}

fn published(run: &WorldRun, version: u64) -> Result<pocket_link::WorldSnapshot, Problem> {
    Ok(pocket_link::WorldSnapshot {
        version,
        snapshot: run.snap()?,
        registry: Arc::new(pocket_link::RegistryInfo {
            components: Vec::new(),
            formats: pocket_persist::FormatTable::of(&run.registry, run.sim.world())?,
        }),
        time: pocket_link::TimeStatus {
            tick: run.sim.clock().tick.0,
            t_s: run.sim.clock().time(),
            pacing: pocket_link::PacingStatus::Stepped,
            paused: true,
            halted: false,
            behind_ms: None,
        },
        last_event: 0,
        published_at_ms: 0.0,
    })
}

fn check_link(run: &mut WorldRun) -> Result<(), Problem> {
    use pocket_link::{Envelope, Received, ReplyTo, Source};
    let (sender, receiver) = pocket_link::queue(3);
    for (source, seq) in [
        (Source::Developer(0), 2),
        (Source::Editor, 1),
        (Source::Developer(0), 1),
    ] {
        sender.push(Envelope {
            source,
            seq,
            at: None,
            name: "impulse".to_owned(),
            params: json!({}),
            reply: ReplyTo::none(),
        })?;
    }
    let full = sender.push(Envelope {
        source: Source::Host,
        seq: 1,
        at: None,
        name: "extra".to_owned(),
        params: json!({}),
        reply: ReplyTo::none(),
    });
    ensure(
        matches!(full, Err(ref e) if e.code == "queue.full"),
        "bounded queue",
    )?;
    let mut batch = Vec::new();
    while let Received::One(envelope) = receiver.try_next() {
        batch.push(envelope);
    }
    ensure(receiver.len() == 3, "held commands count against capacity")?;
    pocket_link::canonical_order(&mut batch);
    ensure(
        batch.iter().map(|e| (e.source, e.seq)).collect::<Vec<_>>()
            == [
                (Source::Editor, 1),
                (Source::Developer(0), 1),
                (Source::Developer(0), 2),
            ],
        "canonical command order",
    )?;
    receiver.done(batch.len());
    ensure(receiver.is_empty(), "queue slots released")?;
    drop(receiver);
    ensure(
        matches!(sender.push(Envelope { source: Source::Host, seq: 1, at: None,
        name: "closed".to_owned(), params: json!({}), reply: ReplyTo::none() }),
        Err(ref e) if e.code == "game.stopped"),
        "closed queue",
    )?;

    let (publisher, reader) = pocket_link::publication(published(run, 1)?, Arc::new(|| 0.0));
    let held = reader.latest();
    let before = held.snapshot.to_bytes();
    run.step()?;
    publisher.publish(published(run, 2)?);
    publisher.set_state(pocket_link::LoopState::Breakpoint, run.sim.clock().tick);
    ensure(
        held.snapshot.to_bytes() == before && reader.latest().version == 2,
        "immutable publication remains readable after replacement",
    )?;
    ensure(
        reader.status().state == pocket_link::LoopState::Breakpoint,
        "published reads work while marked paused",
    )
}

pub fn verify_foundation(seed: u64, ticks: u64) -> Result<Report, Problem> {
    let mut first = WorldRun::new(seed)?;
    let mut second = WorldRun::new(seed)?;
    let sink = MemorySink::new();
    let mut recorder = pocket_persist::Recorder::start(
        first.world(),
        first.registry.clone(),
        &BundleRecord::empty(),
        "{}",
        RecordOptions::default(),
        Box::new(sink.clone()),
    )?;
    let mut checkpoint = None;
    for tick in 1..=ticks {
        if let Some(params) = input_at(tick, ticks) {
            first.apply_impulse("impulse", &params)?;
            second.apply_impulse("impulse", &params)?;
            recorder.applied(&Applied {
                tick: Tick(tick),
                index: 0,
                source: pocket_link::Source::Player(0),
                seq: tick,
                name: "impulse".to_owned(),
                params,
            });
        }
        first.step()?;
        second.step()?;
        recorder.end_tick(first.world())?;
        ensure(
            world_hash(first.world(), &first.registry)?
                == world_hash(second.world(), &second.registry)?,
            "same seed/input at every tick",
        )?;
        if tick == ticks / 2 {
            checkpoint = Some(first.snap()?);
        }
    }
    recorder.finish(first.world())?;
    let final_hash = world_hash(first.world(), &first.registry)?;
    let saved = checkpoint.expect("at least three ticks");
    let parsed = Snapshot::from_bytes(&saved.to_bytes())?;
    let mut resumed = WorldRun::new(seed.wrapping_add(1))?;
    resumed.restore(&parsed, &BundleRecord::empty())?;
    ensure(
        resumed.snap()?.to_bytes() == saved.to_bytes(),
        "canonical snapshot round trip",
    )?;
    advance(&mut resumed, ticks, ticks)?;
    ensure(
        world_hash(resumed.world(), &resumed.registry)? == final_hash,
        "restore continuation",
    )?;

    let mut fork = WorldRun::new(0)?;
    fork_into(first.world(), fork.world_mut(), &first.registry)?;
    ensure(
        fork.snap()?.to_bytes() == first.snap()?.to_bytes(),
        "fork starts identical",
    )?;
    let untouched = first.snap()?.to_bytes();
    fork.apply_impulse("impulse", &json!({"entity": 1, "dx": 5.0, "dy": 0.0}))?;
    fork.step()?;
    ensure(
        first.snap()?.to_bytes() == untouched && fork.snap()?.to_bytes() != untouched,
        "fork isolated from parent",
    )?;

    let refusal = first.apply_impulse(
        "impulse",
        &json!({"entity": 1, "dx": 1.0, "dy": 0.0, "dxx": 2}),
    );
    ensure(
        matches!(refusal, Err(ref e) if e.code == "request.unknown_field")
            && first.snap()?.to_bytes() == untouched,
        "refused input has no side effects",
    )?;
    let mut damaged = saved.to_bytes();
    let last = damaged.len() - 1;
    damaged[last] ^= 1;
    ensure(
        Snapshot::from_bytes(&damaged).is_err(),
        "corrupt snapshot rejected",
    )?;

    let replay = Replay::read(&sink.bytes())?;
    let checked = verify(
        &replay,
        &mut WorldRun::new(0)?,
        VerifyOptions::mode(ReplayMode::Verify),
    );
    ensure(
        checked.verified
            && checked.ticks_run == ticks
            && checked.outcome == ReplayOutcome::Identical,
        "recorded inputs replay identically",
    )?;
    let (header, mut records, complete) = replay.into_parts();
    for record in &mut records {
        if let Record::Tick { writes, .. } = record
            && let Some(write) = writes.first_mut()
        {
            let mut params = write.params_json();
            params["dx"] = json!(3.0);
            write.params = pocket_persist::replay::canonical_json(&params);
            break;
        }
    }
    let changed = Replay::from_records(header, records, complete)?;
    let checked = verify(
        &changed,
        &mut WorldRun::new(0)?,
        VerifyOptions::mode(ReplayMode::Verify),
    );
    let ReplayOutcome::Diverged(divergence) = checked.outcome else {
        return Err(Problem::new(
            "foundation.check_failed",
            "changed replay was not detected",
            detail([]),
        ));
    };
    ensure(
        divergence.at.tick.0 == ticks / 3,
        "first diverging input tick identified",
    )?;
    check_link(&mut fork)?;
    Ok(Report {
        slice: "F0",
        seed,
        ticks,
        entities: first
            .sim
            .world()
            .resource::<pocket_sim::EntityIndex>()
            .len(),
        world_hash: final_hash.to_string(),
        source: EngineVersion::current().clone(),
        checks: vec![
            "determinism",
            "snapshot_round_trip",
            "restore_continuation",
            "fork_isolation",
            "refusal_atomicity",
            "snapshot_corruption",
            "replay",
            "first_divergence",
            "queue",
            "immutable_publication",
        ],
        negative_divergence_tick: divergence.at.tick.0,
    })
}
