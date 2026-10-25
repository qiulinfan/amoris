//! Rigid bodies on Rapier, buoyancy on waves, wind on a sail, keel, rudder and hull drag
//! (docs/spec/architecture.md 4.4; numeric.md 3.1 and 5; persistence.md 8; simulation.md 4.4 and
//! 8.5).
//!
//! - **Components** are plain data in `f64` ([`body`], [`boat`], [`sea`]); the solver
//!   (`rapier3d` 0.36.0 with `enhanced-determinism`) is built from them in `EntityId` order and is
//!   world state itself, a Cache ([`solver::Physics`], [`PhysicsCache`]), so a snapshot, a hash
//!   and a fork carry its contacts and warm-start impulses.
//! - **Systems** ([`plugin`]) take the rows of simulation.md 4.4: `physics.wind`,
//!   `physics.buoyancy`, `physics.sail` and `physics.hull` gather each body's force from the state
//!   at the tick's start; `physics.step` syncs the bodies in, applies the forces, steps once and
//!   writes the components back; `physics.contacts` emits the step's contact events.
//! - **Determinism**: every force is `+ - * /`, `sqrt` and `amoris_sim::math`; `f64` components
//!   meet the `f32` solver only in `physics.step` ([`geom::f32_of`]); a dynamic hull, mesh or
//!   compound states its mass properties instead of a density ([`body::validate`],
//!   `sim.collider_density`); there are no joints and no continuous collision detection.
//! - **Queries** ([`query`]): ray casts, overlaps, the water and the wind at a point.

#![deny(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

pub mod board;
pub mod boat;
pub mod body;
mod cache;
pub mod fluids;
mod forces;
pub mod geom;
pub mod probe;
pub mod query;
mod readings;
pub mod sailing;
pub mod sea;
pub mod shallow;
mod shapes;
pub mod solver;
pub mod terrain;
pub mod vessel;
pub mod walk;
pub mod walker;
pub mod water;

pub use board::Board;
pub use boat::{Boat, BuoyPoint, Floater, Hull, Sail, Trim, best_sheet};
pub use body::{
    BodyKind, Collider, ExternalForce, MassProps, Part, PartShape, RigidBody, Shape, Transform,
    Velocity, validate,
};
pub use cache::{IDENTITY, PhysicsCache, declare};
pub use forces::start_time;
pub use sea::{GRAVITY, Rain, Sea, SeaLook, Water, Wave, WaveField, Wind};
pub use solver::{BodyIndex, ContactLog, Physics, RebuiltSolver};
pub use terrain::{Terrain, TerrainFeature, TerrainLayer, TerrainLook};
pub use vessel::{Hole, Vessel};
pub use walker::Walker;
pub use water::{
    BedSource, Edge, Style, WaterDrain, WaterLook, WaterRegion, WaterScratch, WaterSource,
    WaterSteps, WaterTile,
};

use amoris_contract::Problem;
use amoris_sim::registry::ComponentRegistry;
use amoris_sim::{RunCondition, Sim, TickPhase};
use bevy_ecs::prelude::IntoSystem;
use bevy_ecs::system::System;

/// Installs physics into a world: the solver (stepping the world's `dt` under standard gravity),
/// the derived index and contact log, the components in the registry, and the systems at their
/// rows of the system order (simulation.md 4.4). Persistence classes are declared separately, by
/// [`declare`], on the registry builder.
pub fn plugin(sim: &mut Sim) -> Result<(), Problem> {
    let dt = sim.clock().dt();
    let w = sim.world_mut();
    w.insert_resource(Physics::new(dt));
    w.insert_resource(BodyIndex::default());
    w.insert_resource(ContactLog::default());
    w.insert_resource(WaterScratch::default());
    w.insert_resource(WaterSteps::default());
    w.insert_resource(water::buoyancy::Samples::default());
    w.insert_resource(vessel::Hulls::default());
    w.insert_resource(water::swimming::Samples::default());
    w.insert_resource(water::resident::WaterCheckpoints::default());
    ComponentRegistry::register::<Transform>(w, None)?;
    ComponentRegistry::register::<Velocity>(w, None)?;
    ComponentRegistry::register::<RigidBody>(w, None)?;
    ComponentRegistry::register::<Collider>(w, None)?;
    ComponentRegistry::register::<ExternalForce>(w, None)?;
    ComponentRegistry::register::<Floater>(w, None)?;
    ComponentRegistry::register::<Vessel>(w, None)?;
    ComponentRegistry::register::<Boat>(w, None)?;
    ComponentRegistry::register::<Sail>(w, None)?;
    ComponentRegistry::register::<Hull>(w, None)?;
    ComponentRegistry::register::<Sea>(w, None)?;
    ComponentRegistry::register::<Wind>(w, None)?;
    ComponentRegistry::register::<Rain>(w, None)?;
    ComponentRegistry::register::<Walker>(w, None)?;
    ComponentRegistry::register::<Board>(w, None)?;
    ComponentRegistry::register::<amoris_sim::voxels::Voxels>(w, None)?;
    ComponentRegistry::register::<WaterRegion>(w, None)?;
    ComponentRegistry::register::<water::WaterColliderBed>(w, None)?;
    ComponentRegistry::register::<water::resident::WaterResident>(w, None)?;
    ComponentRegistry::register::<water::resident::WaterOutletStage>(w, None)?;
    ComponentRegistry::register::<water::resident::WaterProbe>(w, None)?;
    ComponentRegistry::register::<water::feedback::WaterFeedback>(w, None)?;
    ComponentRegistry::register::<water::displacement::WaterDisplacement>(w, None)?;
    ComponentRegistry::register::<WaterSource>(w, None)?;
    ComponentRegistry::register::<water::WaterFocus>(w, None)?;
    ComponentRegistry::register::<WaterDrain>(w, None)?;
    ComponentRegistry::register::<Terrain>(w, None)?;
    ComponentRegistry::register::<terrain::TerrainGround>(w, None)?;
    let mut water_system = IntoSystem::into_system(water::water);
    water_system.initialize(w);
    let always = RunCondition::Always;
    sim.add_system("physics.wind", TickPhase::Forces, always, forces::wind)?;
    sim.add_exclusive(
        "physics.water",
        TickPhase::Forces,
        always,
        move |world, ctx| {
            if let Err(error) = water::prepare_collider_beds(world) {
                ctx.fault(error);
                return;
            }
            match water_system.run((), world) {
                Ok(Ok(false)) => {}
                Ok(Ok(true)) => ctx.suspend(),
                Ok(Err(problem)) => ctx.fault(problem),
                Err(error) => ctx.fault(Problem::new(
                    "water.compute_failed",
                    error.to_string(),
                    amoris_contract::detail([]),
                )),
            }
        },
    )?;
    let mut buoyancy_system = IntoSystem::into_system(forces::buoyancy);
    buoyancy_system.initialize(sim.world_mut());
    sim.add_exclusive(
        "physics.buoyancy",
        TickPhase::Forces,
        always,
        move |world, ctx| match buoyancy_system.run((), world) {
            Ok(Ok(())) => {}
            Ok(Err(problem)) => ctx.fault(problem),
            Err(error) => ctx.fault(Problem::new(
                "water.buoyancy_failed",
                error.to_string(),
                amoris_contract::detail([]),
            )),
        },
    )?;
    sim.add_system("physics.sail", TickPhase::Forces, always, forces::sail)?;
    let mut hull_system = IntoSystem::into_system(forces::hull);
    hull_system.initialize(sim.world_mut());
    sim.add_exclusive(
        "physics.hull",
        TickPhase::Forces,
        always,
        move |world, ctx| match hull_system.run((), world) {
            Ok(Ok(())) => {}
            Ok(Err(problem)) => ctx.fault(problem),
            Err(error) => ctx.fault(Problem::new(
                "water.hull_failed",
                error.to_string(),
                amoris_contract::detail([]),
            )),
        },
    )?;
    let mut board_system = IntoSystem::into_system(board::board);
    board_system.initialize(sim.world_mut());
    sim.add_exclusive(
        "physics.board",
        TickPhase::Forces,
        always,
        move |world, ctx| match board_system.run((), world) {
            Ok(Ok(())) => {}
            Ok(Err(problem)) => ctx.fault(problem),
            Err(error) => ctx.fault(Problem::new(
                "board.failed",
                error.to_string(),
                amoris_contract::detail([]),
            )),
        },
    )?;
    sim.add_system(
        "physics.terrain",
        TickPhase::Forces,
        always,
        terrain::ground,
    )?;
    sim.add_system("physics.fluids", TickPhase::Physics, always, fluids::fluids)?;
    let mut walk_system = IntoSystem::into_system(walker::walk);
    walk_system.initialize(sim.world_mut());
    sim.add_exclusive(
        "physics.walk",
        TickPhase::Physics,
        always,
        move |world, ctx| match walk_system.run((), world) {
            Ok(Ok(())) => {}
            Ok(Err(problem)) => ctx.fault(problem),
            Err(error) => ctx.fault(Problem::new(
                "water.swimming_failed",
                error.to_string(),
                amoris_contract::detail([]),
            )),
        },
    )?;
    sim.add_system("physics.step", TickPhase::Physics, always, solver::step)?;
    sim.add_system(
        "physics.contacts",
        TickPhase::Physics,
        always,
        solver::contacts,
    )?;
    sim.add_system("physics.mount", TickPhase::Physics, always, board::mount)?;
    sim.add_system(
        "physics.walk.clear",
        TickPhase::Finish,
        always,
        walker::clear,
    )?;
    Ok(())
}

/// A web test: its name and the function that runs it.
pub type WebTest = (&'static str, fn() -> Result<(), String>);

/// The web tests of this crate (checks.md 7.2, `tests`): each runs in the WebAssembly build as
/// natively and must pass on both.
pub const WEB_TESTS: &[WebTest] = &[
    ("physics.chain", probe::check_chain),
    ("physics.fork", probe::check_forks),
    ("physics.bvh_log2", probe::check_bvh_log2),
    ("physics.level_fork", probe::level::check_level_forks),
    ("physics.shape_builds", probe::layout::check_shape_builds),
];

/// The web tests' report: one `ok name` or `FAIL name: why` line per test, the digest of the shape
/// builds (`shapes digest`, `probe::layout`), then the sailing scene's hash at every tick, then the
/// small level scene's (`level tick hash`), then a water region's dam break (`water second hash
/// substeps`, `water::water_report`). The WebAssembly build (`examples/web_physics.rs`) must
/// produce the same bytes as the native builds.
pub fn web_report() -> String {
    let mut out = String::new();
    for (name, test) in WEB_TESTS {
        match test() {
            Ok(()) => out.push_str(&format!("ok {name}\n")),
            Err(e) => out.push_str(&format!("FAIL {name}: {e}\n")),
        }
    }
    out.push_str(&probe::layout::shapes_report());
    out.push_str(&probe::chain_report());
    out.push_str(&probe::level::level_report());
    out.push_str(&water::water_report());
    out
}
