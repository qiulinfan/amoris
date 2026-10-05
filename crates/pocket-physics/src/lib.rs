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
//! - **Determinism**: every force is `+ - * /`, `sqrt` and `pocket_sim::math`; `f64` components
//!   meet the `f32` solver only in `physics.step` ([`geom::f32_of`]); a dynamic hull, mesh or
//!   compound states its mass properties instead of a density ([`body::validate`],
//!   `sim.collider_density`); there are no joints and no continuous collision detection.
//! - **Queries** ([`query`]): ray casts, overlaps, the water and the wind at a point.

#![deny(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

pub mod boat;
pub mod body;
mod cache;
mod forces;
pub mod geom;
pub mod probe;
pub mod query;
mod readings;
pub mod sailing;
pub mod sea;
pub mod solver;

pub use boat::{Boat, BuoyPoint, Floater, Hull, Sail, Trim, best_sheet};
pub use body::{
    BodyKind, Collider, ExternalForce, MassProps, Part, PartShape, RigidBody, Shape, Transform,
    Velocity, validate,
};
pub use cache::{IDENTITY, PhysicsCache, declare};
pub use forces::start_time;
pub use sea::{GRAVITY, Sea, Water, Wave, WaveField, Wind};
pub use solver::{BodyIndex, ContactLog, Physics};

use pocket_contract::Problem;
use pocket_sim::registry::ComponentRegistry;
use pocket_sim::{RunCondition, Sim, TickPhase};

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
    ComponentRegistry::register::<Transform>(w, None)?;
    ComponentRegistry::register::<Velocity>(w, None)?;
    ComponentRegistry::register::<RigidBody>(w, None)?;
    ComponentRegistry::register::<Collider>(w, None)?;
    ComponentRegistry::register::<ExternalForce>(w, None)?;
    ComponentRegistry::register::<Floater>(w, None)?;
    ComponentRegistry::register::<Boat>(w, None)?;
    ComponentRegistry::register::<Sail>(w, None)?;
    ComponentRegistry::register::<Hull>(w, None)?;
    ComponentRegistry::register::<Sea>(w, None)?;
    ComponentRegistry::register::<Wind>(w, None)?;
    let always = RunCondition::Always;
    sim.add_system("physics.wind", TickPhase::Forces, always, forces::wind)?;
    sim.add_system(
        "physics.buoyancy",
        TickPhase::Forces,
        always,
        forces::buoyancy,
    )?;
    sim.add_system("physics.sail", TickPhase::Forces, always, forces::sail)?;
    sim.add_system("physics.hull", TickPhase::Forces, always, forces::hull)?;
    sim.add_system("physics.step", TickPhase::Physics, always, solver::step)?;
    sim.add_system(
        "physics.contacts",
        TickPhase::Physics,
        always,
        solver::contacts,
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
];

/// The web tests' report: one `ok name` or `FAIL name: why` line per test, then the sailing
/// scene's hash at every tick. The WebAssembly build (`examples/web_physics.rs`) must produce the
/// same bytes as the native builds.
pub fn web_report() -> String {
    let mut out = String::new();
    for (name, test) in WEB_TESTS {
        match test() {
            Ok(()) => out.push_str(&format!("ok {name}\n")),
            Err(e) => out.push_str(&format!("FAIL {name}: {e}\n")),
        }
    }
    out.push_str(&probe::chain_report());
    out
}
