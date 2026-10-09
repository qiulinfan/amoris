//! Perception's own fixture world and its web report (checks.md 7.2, `tests`), without
//! `pocket-persist`, which `pocket-interface` cannot link (architecture.md 5): forks and restores
//! go through `pocket_physics::probe`'s ledger with perception's declarations added.
//!
//! The world is the sailing showcase's skipper on a calm sea under a steady westerly, among an
//! island that hides a crate behind it, two charted islets, two charted marks (one beyond sight),
//! crates near and far, and a second boat; its declarations are `samples/sailing-course/perception.json`
//! with a few test-only additions (a hidden fact, a hidden-scope, a sound and a global event, and
//! a tensor layout for the skipper).

use pocket_physics::probe::Ledger;
use pocket_physics::{Collider, RigidBody, Shape, Transform, sailing};
use pocket_sim::rng::fnv1a64_extend;
use pocket_sim::{EntityId, Name, NoHooks, RunCondition, Sim, SimConfig, TickPhase, TickRate};

use super::defs::{EventDef, Exposure, FactDef, FactSource, PerceptionDefs, Scope, Unit, Vec3};
use super::query::{
    Caller, DescribeRequest, EntityRef, EventsRequest, NearbyRequest, ObserveRequest, Role,
    SeatParts,
};
use super::state::{Observer, Occluder, Perceivable};
use super::{NoAffordances, declare, describe, events, load, nearby, observe, plugin};
use crate::projection::Projection;
use crate::projection::tensor::{TensorBlock, TensorSpec};

/// The sailing showcase's perception declarations.
pub const SAILING: &str = include_str!("../../../../samples/sailing-course/perception.json");

/// The sailing declarations with the fixture's additions: a hidden fact on islands
/// (`restitution`), and events of hidden (`secret.signal`), sound (`horn.blast`, 200 m) and global
/// (`race.start`) scope.
pub fn defs() -> PerceptionDefs {
    let mut d = load(SAILING).expect("the sailing declarations load");
    if let Some(k) = d.decl.kinds.iter_mut().find(|k| k.kind == "island") {
        k.facts.push(FactDef {
            name: "restitution".into(),
            doc: "A hidden fact: the island's bounciness.".into(),
            unit: Unit::Fraction { min: 0.0, max: 1.0 },
            precision: 2,
            exposure: Exposure::Hidden,
            source: FactSource::Field {
                component: "Collider".into(),
                path: "restitution".into(),
            },
            relative: false,
        });
    }
    let ev = |kind: &str, scope: Scope| EventDef {
        kind: kind.into(),
        doc: "A fixture event.".into(),
        scope,
        data: vec![FactDef {
            name: "n".into(),
            doc: "A number.".into(),
            unit: Unit::Count,
            precision: 0,
            exposure: Exposure::Coarse,
            source: FactSource::Data { path: "n".into() },
            relative: false,
        }],
    };
    d.decl.events.push(ev("secret.signal", Scope::Hidden));
    d.decl
        .events
        .push(ev("horn.blast", Scope::Sound { radius_m: 200.0 }));
    d.decl.events.push(ev("race.start", Scope::Global));
    if let Some(p) = d.decl.observers.iter_mut().find(|p| p.name == "skipper") {
        p.tensor = Some(tensor_spec());
    }
    d
}

/// A tensor layout for the skipper, as an RL observer would declare one: five instruments, the
/// four nearest crates, the four nearest marks and islands, and eight rays all round.
pub fn tensor_spec() -> TensorSpec {
    let names = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    TensorSpec {
        blocks: vec![
            TensorBlock::Instruments {
                name: "self".into(),
                instruments: names(&[
                    "heading_deg",
                    "speed_mps",
                    "wind_from_deg",
                    "point_of_sail",
                    "afloat",
                ]),
            },
            TensorBlock::Nearest {
                name: "crates".into(),
                kinds: names(&["crate"]),
                count: 4,
                facts: names(&["alongside"]),
            },
            TensorBlock::Nearest {
                name: "marks".into(),
                kinds: names(&["mark", "island"]),
                count: 4,
                facts: names(&["radius_m", "next"]),
            },
            TensorBlock::Rays {
                name: "rays".into(),
                count: 8,
                fov_deg: 360.0,
                range_m: 500.0,
                kinds: names(&["island", "boat", "crate"]),
            },
        ],
    }
}

/// The skipper's crew: sail set and eased at tick 1, a little starboard helm from tick 200.
fn crew(
    clock: bevy_ecs::prelude::Res<pocket_sim::SimClock>,
    mut boats: bevy_ecs::prelude::Query<(&EntityId, &mut pocket_physics::Boat, &Observer)>,
) {
    for (_, mut b, _) in pocket_sim::order::by_id_mut(&mut boats) {
        match clock.tick.0 {
            1 => {
                b.hoist = 1.0;
                b.sheet = 0.9;
            }
            200 => b.rudder = 0.15,
            320 => b.rudder = 0.0,
            _ => {}
        }
    }
}

/// A world with physics, perception and the crew, and no entities: what a fork restores into.
pub fn fresh() -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed: 7,
    })
    .expect("a valid configuration");
    pocket_physics::plugin(&mut sim).expect("physics installs");
    plugin(&mut sim, defs()).expect("perception installs");
    sim.add_system("probe.crew", TickPhase::Update, RunCondition::Always, crew)
        .expect("the crew's key is free");
    sim
}

/// The ledger of the simulation's, physics' and perception's declarations.
pub fn ledger() -> Ledger {
    let mut l = pocket_physics::probe::ledger();
    declare(&mut l);
    l
}

/// The fixture's entities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ids {
    pub sloop: EntityId,
    pub isle: EntityId,
    pub islet: EntityId,
    pub far_islet: EntityId,
    pub mark1: EntityId,
    pub far_mark: EntityId,
    /// Floating 20 m off, seen in full from the start.
    pub near_crate: EntityId,
    /// Behind the Isle, in range but hidden.
    pub hidden_crate: EntityId,
    /// Kilometres away, outside every range.
    pub far_crate: EntityId,
    pub other_boat: EntityId,
}

/// A fixed island: an octagonal prism of `radius` from 5 m under the water to 15 m above it.
pub fn island(at: [f64; 3], radius: f64, chart: bool) -> impl bevy_ecs::prelude::Bundle {
    let mut points = Vec::new();
    for i in 0..8u32 {
        let a = f64::from(i) * std::f64::consts::FRAC_PI_4;
        let (s, c) = pocket_sim::math::sin_cos(a);
        points.push([radius * c, -5.0, radius * s]);
        points.push([radius * c, 15.0, radius * s]);
    }
    (
        Transform::at(at),
        RigidBody::fixed(),
        Collider::new(Shape::ConvexHull { points }),
        Occluder {},
        Perceivable {
            chart_m: chart.then_some(Vec3::of(at)),
            ..super::sailing::island(at, 15.0)
        },
    )
}

fn crate_at(sim: &mut Sim, name: &str, at: [f64; 3]) -> EntityId {
    let mut b = sim.boundary();
    let id = b
        .spawn((
            Name::new(name).expect("a name"),
            sailing::crate_box(at, 0.0),
        ))
        .expect("spawns");
    let e = b.entity(id).expect("live");
    b.world_mut().entity_mut(e).insert(super::sailing::adrift());
    id
}

fn mark_at(sim: &mut Sim, name: &str, at: [f64; 3]) -> EntityId {
    let mut b = sim.boundary();
    let perc = super::sailing::mark(at);
    b.spawn((Name::new(name).expect("a name"), Transform::at(at), perc))
        .expect("spawns")
}

fn named(sim: &mut Sim, name: &str, bundle: impl bevy_ecs::prelude::Bundle) -> EntityId {
    let mut b = sim.boundary();
    b.spawn((Name::new(name).expect("a name"), bundle))
        .expect("spawns")
}

/// Spawns the fixture into a fresh world.
pub fn scene(sim: &mut Sim) -> Ids {
    let mut b = sim.boundary();
    b.spawn((Name::new("Sea").expect("a name"), sailing::calm_sea()))
        .expect("spawns");
    b.spawn((
        Name::new("Breeze").expect("a name"),
        sailing::breeze(270.0, 6.0),
    ))
    .expect("spawns");
    let sloop = named(sim, "Sloop", sailing::sloop([0.0, 0.0, 0.0], 90.0));
    let e = sim.boundary().entity(sloop).expect("live");
    sim.world_mut()
        .entity_mut(e)
        .insert((super::sailing::skipper(), super::sailing::boat()));
    let isle = named(sim, "Isle", island([0.0, 0.0, -60.0], 25.0, true));
    let islet = named(sim, "Islet1", island([700.0, 0.0, -480.0], 60.0, true));
    let far_islet = named(sim, "Islet2", island([-2500.0, 0.0, 1500.0], 55.0, true));
    let mark1 = mark_at(sim, "Mark1", [400.0, 0.0, 10.0]);
    let far_mark = mark_at(sim, "Mark2", [2500.0, 0.0, -2500.0]);
    let near_crate = crate_at(sim, "Crate1", [20.0, 0.0, 3.0]);
    let hidden_crate = crate_at(sim, "Crate2", [0.0, 0.0, -100.0]);
    let far_crate = crate_at(sim, "Crate3", [-3000.0, 0.0, -3000.0]);
    let other_boat = named(sim, "Ketch", sailing::sloop([-150.0, 0.0, 120.0], 0.0));
    let e = sim.boundary().entity(other_boat).expect("live");
    sim.world_mut().entity_mut(e).insert(super::sailing::boat());
    Ids {
        sloop,
        isle,
        islet,
        far_islet,
        mark1,
        far_mark,
        near_crate,
        hidden_crate,
        far_crate,
        other_boat,
    }
}

/// The fixture world, stepped `ticks` ticks.
pub fn world(ticks: u64) -> (Sim, Ids) {
    let mut sim = fresh();
    let ids = scene(&mut sim);
    for _ in 0..ticks {
        sim.step(&mut NoHooks).expect("a tick");
    }
    (sim, ids)
}

/// The skipper as a player.
pub const PLAYER: Caller<'static> = Caller {
    role: Role::Player,
    seat: Some("skipper"),
};

/// Every query kind for the skipper, in text and JSON at two budgets, a `describe` and the tensor
/// projection: the bytes the determinism checks compare.
pub fn answers(sim: &Sim) -> Vec<String> {
    let w = sim.world();
    let parts = SeatParts::default();
    let aff = NoAffordances;
    let mut out = Vec::new();
    let body = |r: Result<super::Answer, pocket_contract::Problem>| match r {
        Ok(a) => a.body,
        Err(p) => format!("refused {} {}", p.code, p.message),
    };
    for projection in [Projection::Text, Projection::Json] {
        for budget in [120, 400] {
            let obs = ObserveRequest {
                budget_tokens: Some(budget),
                projection: Some(projection),
                since: Some(0),
                ..ObserveRequest::default()
            };
            let near = NearbyRequest {
                budget_tokens: Some(budget),
                projection: Some(projection),
                ..NearbyRequest::default()
            };
            let ev = EventsRequest {
                since: 0,
                budget_tokens: Some(budget),
                projection: Some(projection),
                ..EventsRequest::default()
            };
            out.push(body(observe(w, &PLAYER, &obs, &parts, &aff)));
            out.push(body(nearby(w, &PLAYER, &near, &[], &aff)));
            out.push(body(events(w, &PLAYER, &ev)));
        }
        let mark = DescribeRequest {
            seat: None,
            entity: EntityRef::Text("Mark1".into()),
            budget_tokens: None,
            projection: Some(projection),
            omniscient: None,
        };
        out.push(body(describe(w, &PLAYER, &mark, &aff)));
    }
    let tensor = ObserveRequest {
        projection: Some(Projection::Tensor),
        ..ObserveRequest::default()
    };
    out.push(body(observe(w, &PLAYER, &tensor, &parts, &aff)));
    out
}

fn fold(answers: &[String]) -> u64 {
    answers.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, a| {
        fnv1a64_extend(h, a.as_bytes())
    })
}

/// Forks the fixture at tick 300 and runs both branches 120 ticks: the answers must be equal at
/// the fork and at every tick after it.
pub fn check_fork() -> Result<(), String> {
    let (mut sim, _) = world(300);
    let mut branch = pocket_physics::probe::fork(&ledger(), &sim, fresh)?;
    for t in 0..=120u64 {
        if t > 0 {
            sim.step(&mut NoHooks).map_err(|p| p.message)?;
            branch.step(&mut NoHooks).map_err(|p| p.message)?;
        }
        if answers(&sim) != answers(&branch) {
            return Err(format!(
                "the branch answers differently {t} ticks after the fork"
            ));
        }
    }
    Ok(())
}

/// The fixture run 600 ticks with every query kind every 60 ticks, folded into one FNV-1a hash
/// per checkpoint; the fork check; then the last observation in full. Native and WebAssembly
/// builds must give the same bytes.
pub fn web_report() -> String {
    let mut sim = fresh();
    scene(&mut sim);
    let mut out = String::new();
    for t in 1..=600u64 {
        if let Err(p) = sim.step(&mut NoHooks) {
            return format!(
                "FAIL perception.web: tick {t}: {}
",
                p.message
            );
        }
        if t % 60 == 0 {
            out.push_str(&format!(
                "tick {t} answers {:016x}
",
                fold(&answers(&sim))
            ));
        }
    }
    let fork = match check_fork() {
        Ok(()) => "ok perception.fork
"
        .to_owned(),
        Err(e) => format!(
            "FAIL perception.fork: {e}
"
        ),
    };
    let last = answers(&sim).into_iter().nth(1).unwrap_or_default();
    format!(
        "ok perception.web
{fork}{out}{last}"
    )
}
