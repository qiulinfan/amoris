//! The rule workload's worlds (docs/bench/script.md), shared by the native bench (`rules_bench`)
//! and its WebAssembly module (`rules_web`): a world running a compiled bundle through the script
//! host (`install`, `swap`, `Sim::step`), or the same rule as Rust systems over Rust components
//! (the twin), and one digest both worlds give alike when they agree.
//!
//! Every reading takes the clock it is handed (a wall clock natively, `performance.now()` on the
//! web), in milliseconds.

#![allow(dead_code)] // each of the two examples uses a part

use bevy_ecs::prelude::{Component, World};
use pocket_script::scripts::{Scripts, install, last_steps, swap};
use pocket_script::{CompiledSet, PhaseTimes, ScriptLimits};
use pocket_sim::event::{self, EventInbox, EventKind, NewEvent};
use pocket_sim::{
    EntityAllocator, EntityId, EntityIndex, EventCounter, PlainData, RngTable, RunCondition, Sim,
    SimClock, SimConfig, StepHooks, SystemKey, TickPhase, TickRate, entity, math,
};

/// The project's seed (bench/rules/project.toml).
pub const SEED: u64 = 1;
/// Entities `populate` spawns (combat.ts).
pub const COUNT: usize = 1000;
const CRIT_CHANCE: f64 = 0.1;
const REGEN: f64 = 0.25;
const HITS_PER_EVENT: usize = 250;
/// The components the rule reads, and their fields in the order the digest reads them.
const FIELDS: [(&str, &str); 8] = [
    ("Attack", "power"),
    ("Attack", "target"),
    ("Spot", "x"),
    ("Spot", "y"),
    ("Health", "hp"),
    ("Health", "max"),
    ("Armor", "value"),
    ("Stats", "kills"),
];

/// A milliseconds clock.
pub type Clock = fn() -> f64;

/// Times the systems of a step by key (the step hooks, as the game loop's profiling does).
struct Timer {
    clock: Clock,
    open: Vec<f64>,
    /// Milliseconds per system key in the last step.
    times: Vec<(SystemKey, f64)>,
}

impl StepHooks for Timer {
    fn system(&mut self, key: &SystemKey, begin: bool) {
        let now = (self.clock)();
        if begin {
            self.open.push(now);
        } else if let Some(t) = self.open.pop() {
            match self.times.iter_mut().find(|(k, _)| k == key) {
                Some((_, ms)) => *ms += now - t,
                None => self.times.push((key.clone(), now - t)),
            }
        }
    }
}

/// What one step did besides its wall time.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    /// `script.update` (the script world) or both twin systems, ms.
    pub update: f64,
    /// The measured system alone: `script:<name>` or `twin.damage`, ms.
    pub system: f64,
    /// The script calls' phases, ms (zero for the twin).
    pub phases: PhaseTimes,
    /// Steps of every script call in the tick.
    pub steps: u64,
}

/// A world running the workload.
pub struct Bench {
    sim: Sim,
    timer: Timer,
    script: bool,
    /// The system `Sample::system` times.
    measured: String,
}

fn sim() -> Result<Sim, String> {
    Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: SEED,
    })
    .map_err(|p| p.message)
}

impl Bench {
    /// A world with the script host and `set` swapped in; `measured` names the script system
    /// `Sample::system` times (`damage`, `idle`, `scan`).
    pub fn script(set: &CompiledSet, measured: &str, clock: Clock) -> Result<Bench, String> {
        let mut sim = sim()?;
        install(&mut sim, ScriptLimits::default()).map_err(|p| p.message)?;
        swap(sim.world_mut(), set, false).map_err(|e| format!("swap: {}", e[0].message))?;
        if let Some(s) = sim.world().get_non_send::<Scripts>() {
            s.host.time_phases(Some(clock));
        }
        Ok(Bench {
            sim,
            timer: Timer {
                clock,
                open: Vec::new(),
                times: Vec::new(),
            },
            script: true,
            measured: format!("script:{measured}"),
        })
    }

    /// The twin: the same rule as two Rust systems over Rust components, no script host.
    pub fn twin(clock: Clock) -> Result<Bench, String> {
        let mut sim = sim()?;
        sim.add_exclusive(
            "twin.populate",
            TickPhase::Update,
            RunCondition::Start,
            twin_populate,
        )
        .map_err(|p| p.message)?;
        sim.add_exclusive(
            "twin.damage",
            TickPhase::Update,
            RunCondition::Always,
            twin_damage,
        )
        .map_err(|p| p.message)?;
        Ok(Bench {
            sim,
            timer: Timer {
                clock,
                open: Vec::new(),
                times: Vec::new(),
            },
            script: false,
            measured: "twin.damage".into(),
        })
    }

    /// One tick: `Sim::step` and nothing else, so a caller's clock around it times the tick.
    /// A failed system is an error: the workload has none.
    pub fn tick(&mut self) -> Result<(), String> {
        // Zeroed, not cleared: the keys stay, so the hooks allocate nothing after the first tick.
        for (_, ms) in &mut self.timer.times {
            *ms = 0.0;
        }
        let r = self.sim.step(&mut self.timer).map_err(|p| p.message)?;
        match r.errors.first() {
            Some(p) => Err(format!("tick {}: {}", r.tick.0, p.message)),
            None => Ok(()),
        }
    }

    /// What the last tick did, read after it.
    pub fn sample(&self) -> Sample {
        let time = |key: &str| {
            self.timer
                .times
                .iter()
                .filter(|(k, _)| k.as_str() == key)
                .map(|(_, ms)| ms)
                .sum::<f64>()
        };
        let world = self.sim.world();
        if self.script {
            let phases = world
                .get_non_send::<Scripts>()
                .map(|s| s.host.take_phases())
                .unwrap_or_default();
            Sample {
                update: time("script.update"),
                system: time(&self.measured),
                phases,
                steps: last_steps(world).iter().map(|(_, s)| s).sum(),
            }
        } else {
            Sample {
                update: time("twin.populate") + time("twin.damage"),
                system: time(&self.measured),
                ..Sample::default()
            }
        }
    }

    /// The digest of the world after the last tick.
    pub fn digest(&self) -> u64 {
        digest(self.sim.world(), self.script)
    }

    /// The last tick's `combat.damaged` events, hits and kills: the workload did its work.
    pub fn combat(&self) -> [u64; 3] {
        let inbox = self.sim.world().resource::<EventInbox>();
        let len = |e: &pocket_sim::Event, key: &str| match &e.data {
            PlainData::Object(fields) => {
                fields
                    .iter()
                    .find(|(k, _)| k == key)
                    .map_or(0, |(_, v)| match v {
                        PlainData::Array(a) => a.len() as u64,
                        _ => 0,
                    })
            }
            _ => 0,
        };
        let mut out = [0; 3];
        for e in inbox.tick_events() {
            match e.kind.as_str() {
                "combat.damaged" => {
                    out[0] += 1;
                    out[1] += len(e, "source");
                }
                "combat.died" => out[2] += len(e, "target"),
                _ => {}
            }
        }
        out
    }

    /// The world.
    pub fn world(&self) -> &World {
        self.sim.world()
    }

    /// The ticks run.
    pub fn tick_count(&self) -> u64 {
        self.sim.world().resource::<SimClock>().tick.0
    }
}

// The twin's components: the script's five, as Rust holds them.

#[derive(Component, Clone, Copy)]
struct Attack {
    power: f64,
    target: Option<EntityId>,
}

#[derive(Component, Clone, Copy)]
struct Spot {
    x: f64,
    y: f64,
}

#[derive(Component, Clone, Copy)]
struct Health {
    hp: f64,
    max: f64,
}

#[derive(Component, Clone, Copy)]
struct Armor {
    value: f64,
}

#[derive(Component, Clone, Copy)]
struct Stats {
    kills: u32,
}

/// `populate` (rules.ts): the same draws from the script system's stream, in the same order.
fn twin_populate(world: &mut World, _: &mut pocket_sim::SystemCtx<'_>) {
    let mut rows = Vec::with_capacity(COUNT);
    {
        let mut table = world.resource_mut::<RngTable>();
        let g = table.system("script:populate").expect("in a tick");
        for _ in 0..COUNT {
            let power = g.range(5.0, 15.0).expect("finite bounds");
            let x = g.range(0.0, 500.0).expect("finite bounds");
            let y = g.range(0.0, 500.0).expect("finite bounds");
            let armor = g.range(0.0, 50.0).expect("finite bounds");
            rows.push((power, x, y, armor));
        }
    }
    let mut ids = Vec::with_capacity(COUNT);
    for (power, x, y, armor) in rows {
        let id = entity::spawn(
            world,
            (
                Attack {
                    power,
                    target: None,
                },
                Spot { x, y },
                Health {
                    hp: 100.0,
                    max: 100.0,
                },
                Armor { value: armor },
                Stats { kills: 0 },
            ),
        )
        .expect("ids left");
        ids.push(id);
    }
    let last = i64::try_from(COUNT - 1).unwrap_or(0);
    for &id in &ids {
        let k = world
            .resource_mut::<RngTable>()
            .system("script:populate")
            .expect("in a tick")
            .int(0, last)
            .expect("ordered bounds");
        let target = ids[usize::try_from(k).unwrap_or(0)];
        let e = entity::entity(world, id).expect("spawned");
        if let Some(mut a) = world.get_mut::<Attack>(e) {
            a.target = Some(target);
        }
    }
}

fn falloff(distance: f64) -> f64 {
    1.0 / (1.0 + distance / 50.0)
}

fn mitigation(armor: f64) -> f64 {
    100.0 / (100.0 + armor)
}

fn wobble(tick: f64, id: f64) -> f64 {
    1.0 + 0.1 * math::sin(tick * 0.1 + id)
}

fn drift_x(tick: f64, id: f64) -> f64 {
    0.05 * math::cos(tick * 0.05 + id)
}

fn drift_y(tick: f64, id: f64) -> f64 {
    0.05 * math::sin(tick * 0.05 + id * 0.5)
}

/// An event of `kind` whose data is an object of number arrays, keys ascending as plain data
/// holds them.
fn emit_arrays(world: &mut World, kind: &str, fields: Vec<(&str, Vec<PlainData>)>) {
    let data = PlainData::Object(
        fields
            .into_iter()
            .map(|(k, v)| (k.to_owned(), PlainData::Array(v)))
            .collect(),
    );
    let kind = EventKind::new(kind).expect("a dotted kind");
    event::emit(world, NewEvent::new(kind).data(data));
}

/// The hits of `combat.damaged` so far: amount, crit, source, target.
type Hits = [Vec<PlainData>; 4];

fn emit_hits(world: &mut World, hits: Hits) {
    let [amount, crit, source, target] = hits;
    emit_arrays(
        world,
        "combat.damaged",
        vec![
            ("amount", amount),
            ("crit", crit),
            ("source", source),
            ("target", target),
        ],
    );
}

fn no_hits() -> Hits {
    std::array::from_fn(|_| Vec::with_capacity(HITS_PER_EVENT))
}

/// `damage` (rules.ts) as Rust writes it: the rows in id order into columns, the same two loops
/// (events as they fill, so in the same order), the columns back into the components.
fn twin_damage(world: &mut World, _: &mut pocket_sim::SystemCtx<'_>) {
    let tick = world.resource::<SimClock>().tick.to_f64();
    let rows: Vec<(EntityId, bevy_ecs::entity::Entity)> =
        world.resource::<EntityIndex>().iter().collect();
    let n = rows.len();
    let (mut power, mut target, mut x, mut y) = (
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
    );
    let (mut hp, mut max, mut armor, mut kills) = (
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
    );
    let (mut ids, mut ents) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for &(id, e) in &rows {
        let r = world.entity(e);
        let (Some(a), Some(s), Some(h), Some(ar), Some(st)) = (
            r.get::<Attack>(),
            r.get::<Spot>(),
            r.get::<Health>(),
            r.get::<Armor>(),
            r.get::<Stats>(),
        ) else {
            continue;
        };
        ids.push(id);
        ents.push(e);
        power.push(a.power);
        target.push(a.target);
        x.push(s.x);
        y.push(s.y);
        hp.push(h.hp);
        max.push(h.max);
        armor.push(ar.value);
        kills.push(st.kills);
    }
    let n = ids.len();
    let mut rolls = vec![0.0; n];
    world
        .resource_mut::<RngTable>()
        .system("script:damage")
        .expect("in a tick")
        .fill(&mut rolls);
    let mut hit = no_hits();
    let mut death = [Vec::new(), Vec::new()];
    for i in 0..n {
        if hp[i] <= 0.0 {
            continue;
        }
        let Some(j) = target[i].and_then(|t| ids.binary_search(&t).ok()) else {
            continue;
        };
        let dx = x[j] - x[i];
        let dy = y[j] - y[i];
        let crit = rolls[i] < CRIT_CHANCE;
        let id = ids[i].to_f64();
        let amount = power[i]
            * falloff(math::sqrt(dx * dx + dy * dy))
            * mitigation(armor[j])
            * wobble(tick, id)
            * if crit { 2.0 } else { 1.0 };
        let before = hp[j];
        hp[j] = before - amount;
        // Keys ascending, as plain data holds them: amount, crit, source, target.
        hit[0].push(PlainData::Number(amount));
        hit[1].push(PlainData::Number(if crit { 1.0 } else { 0.0 }));
        hit[2].push(PlainData::Number(id));
        hit[3].push(PlainData::Number(ids[j].to_f64()));
        if hit[2].len() == HITS_PER_EVENT {
            emit_hits(world, std::mem::replace(&mut hit, no_hits()));
        }
        if before > 0.0 && hp[j] <= 0.0 {
            // killer, target
            death[0].push(PlainData::Number(id));
            death[1].push(PlainData::Number(ids[j].to_f64()));
            kills[i] += 1;
        }
    }
    for i in 0..n {
        hp[i] = if hp[i] <= 0.0 {
            max[i]
        } else {
            math::min(max[i], hp[i] + REGEN)
        };
        let id = ids[i].to_f64();
        x[i] += drift_x(tick, id);
        y[i] += drift_y(tick, id);
    }
    // By the gathered rows' entities: a row without all five components was skipped above.
    for (i, &e) in ents.iter().enumerate() {
        let mut m = world.entity_mut(e);
        if let Some(mut s) = m.get_mut::<Spot>() {
            s.x = x[i];
            s.y = y[i];
        }
        if let Some(mut h) = m.get_mut::<Health>() {
            h.hp = hp[i];
        }
        if let Some(mut st) = m.get_mut::<Stats>() {
            st.kills = kills[i];
        }
    }
    if !hit[2].is_empty() {
        emit_hits(world, hit);
    }
    let [killer, died] = death;
    emit_arrays(
        world,
        "combat.died",
        vec![("killer", killer), ("target", died)],
    );
}

fn fnv64(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// The rule's state in either world: every entity's eight fields in id order (as bits), the event
/// inbox, the event counter, the allocator and the clock.
pub fn digest(world: &World, script: bool) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let fields = if script { script_fields(world) } else { None };
    for (id, e) in world.resource::<EntityIndex>().iter() {
        fnv64(&mut h, &id.get().to_le_bytes());
        let r = world.entity(e);
        let values: Option<[f64; 8]> = match &fields {
            Some(fields) => {
                let mut out = [0.0; 8];
                let mut ok = true;
                for (k, (access, slot)) in fields.iter().enumerate() {
                    match access.read(&r) {
                        Some(v) => out[k] = v.nums[*slot],
                        None => ok = false,
                    }
                }
                ok.then_some(out)
            }
            None => match (
                r.get::<Attack>(),
                r.get::<Spot>(),
                r.get::<Health>(),
                r.get::<Armor>(),
                r.get::<Stats>(),
            ) {
                (Some(a), Some(s), Some(hl), Some(ar), Some(st)) => Some([
                    a.power,
                    a.target.map_or(0.0, EntityId::to_f64),
                    s.x,
                    s.y,
                    hl.hp,
                    hl.max,
                    ar.value,
                    f64::from(st.kills),
                ]),
                _ => None,
            },
        };
        for x in values.unwrap_or([f64::NAN; 8]) {
            fnv64(&mut h, &x.to_bits().to_le_bytes());
        }
    }
    fnv64(
        &mut h,
        &serde_json::to_vec(world.resource::<EventInbox>()).unwrap_or_default(),
    );
    fnv64(
        &mut h,
        &world.resource::<EventCounter>().next().to_le_bytes(),
    );
    fnv64(
        &mut h,
        &world.resource::<EntityAllocator>().next().to_le_bytes(),
    );
    fnv64(&mut h, &world.resource::<SimClock>().tick.0.to_le_bytes());
    h
}

type FieldAccess = (std::sync::Arc<dyn pocket_script::ComponentAccess>, usize);

/// The script world's accessor and slot of each of `FIELDS`, once its components are registered.
fn script_fields(world: &World) -> Option<Vec<FieldAccess>> {
    let registry = world.get_resource::<pocket_sim::ComponentRegistry>()?;
    let access = world.get_resource::<pocket_script::ScriptAccess>()?;
    FIELDS
        .iter()
        .map(|(comp, field)| {
            let entry = registry.get(comp)?;
            let schema = access
                .schema(entry.id)
                .cloned()
                .or_else(|| entry.script.clone())?;
            let slot = schema
                .fields
                .iter()
                .find(|f| &*f.name == *field)?
                .first_slot;
            Some((access.get(entry.id)?.clone(), usize::from(slot)))
        })
        .collect()
}
