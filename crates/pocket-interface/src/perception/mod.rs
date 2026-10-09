//! Perception: observers, what they can perceive, the perception update, queries, token budgets,
//! push deltas and the marked omniscient view (shared/contract/perception.md; slice 2, whose
//! decisions are docs/spec/perception-slice2.md).
//!
//! - [`defs`]: what a game declares perceivable (profiles, instruments, kinds, facts, events and
//!   their scope), game data installed with the game.
//! - [`state`]: the world state (`Observer`, `Perceivable`, `Occluder`, `ObserverMemory`,
//!   `ObserverEvents`, `VisibilityScale`) and its persistence classes ([`declare`]).
//! - [`update`]: the `interface.perception` system: range, field of view, occlusion past the
//!   occluders, fog, attention, memory, sightings and event scopes.
//! - [`view`]: one observer's perception, read-only (`PerceptionView`), which every answer and
//!   every executor reads; [`omniscient`]: the world seen whole, for developers and checkers.
//! - [`query`] and [`fill`]: the requests, the caller's role and seat, entity references, and the
//!   answers filled to their token budgets; [`fill::delta`] gives the events an act or time
//!   answer pushes.
//! - [`sailing`]: the sailing showcase's derived facts and instruments.

pub mod bridge;
pub mod defs;
pub mod facts;
pub mod fill;
pub mod filter;
pub mod geometry;
pub mod omniscient;
pub mod probe;
pub mod query;
pub mod sailing;
pub mod state;
pub mod update;
pub mod view;

pub use defs::{
    EventDef, Exposure, FactDef, FactSource, Hearing, InstrumentDef, KindDef, OMNISCIENT_PLAYER,
    ObserverProfile, PerceptionDecl, PerceptionDefs, SIGHTED, Scope, Sight, Unit, Vec3,
};
pub use fill::{
    Delta, Observation, delta, describe, events, nearby, observe, write_describe, write_events,
    write_nearby, write_observation,
};
pub use query::{
    AffordanceStatus, Affordances, Answer, Caller, DescribeRequest, EntityRef, EventsRequest,
    NearbyRequest, NoAffordances, ObserveRequest, Role, SeatParts, Sector,
};
pub use state::{
    Detail, FactValue, MemoryEntry, Named, Observer, ObserverEvents, ObserverMemory, Occluder,
    Perceivable, PerceivedEvent, Sense, Visibility, VisibilityScale, declare,
};
pub use view::{EventView, Percept, PerceptionView, Reading};

use bevy_ecs::prelude::World;
use pocket_contract::Problem;
use pocket_sim::registry::ComponentRegistry;
use pocket_sim::{RunCondition, Sim, TickPhase};

/// The system key of the perception update (simulation.md 4.4, row 10).
pub const SYSTEM: &str = "interface.perception";

/// Installs perception into a world: the game's declarations (validated, `definition.invalid`
/// otherwise), the visibility scale, the components in the registry, and the update at its row
/// of the system order. Persistence classes are declared separately, by [`declare`].
pub fn plugin(sim: &mut Sim, defs: PerceptionDefs) -> Result<(), Problem> {
    defs.validate()?;
    let w = sim.world_mut();
    w.insert_resource(defs);
    if !w.contains_resource::<VisibilityScale>() {
        w.insert_resource(VisibilityScale::default());
    }
    register(w)?;
    sim.add_exclusive(SYSTEM, TickPhase::Finish, RunCondition::Always, update::run)
}

/// Registers perception's components with the world's component registry (names, versions and
/// JSON Schemas for tools); a component already registered is left as it is.
pub fn register(w: &mut World) -> Result<(), Problem> {
    fn one<C>(w: &mut World) -> Result<(), Problem>
    where
        C: bevy_ecs::component::Component + pocket_sim::Persisted + schemars::JsonSchema,
    {
        let known = w
            .get_resource::<ComponentRegistry>()
            .is_some_and(|r| r.get(<C as pocket_sim::Persisted>::NAME).is_some());
        if known {
            return Ok(());
        }
        ComponentRegistry::register::<C>(w, None).map(|_| ())
    }
    one::<Observer>(w)?;
    one::<Perceivable>(w)?;
    one::<Occluder>(w)?;
    one::<ObserverMemory>(w)?;
    one::<ObserverEvents>(w)
}

/// The declarations of a game from their JSON text, with the engine's derived functions (the
/// sailing ones) registered, validated.
pub fn load(text: &str) -> Result<PerceptionDefs, Problem> {
    let defs = sailing::functions(PerceptionDefs::from_json(text)?);
    defs.validate()?;
    Ok(defs)
}

/// Binds the seat's observer to the reserved `omniscient_player` profile, or unbinds it: a write
/// at a boundary, made by the process owner's grant (shared/contract/mcp.md 3.1). From the next
/// tick its memory holds every perceivable entity in full and its ring every declared event, and
/// every answer for the seat is marked omniscient. Either way the memory and the ring's entries
/// are cleared (the ring's count goes on), so nothing known under one binding is carried into the
/// other (docs/spec/perception-slice2.md 2).
pub fn bind_omniscient_player(world: &mut World, seat: &str, on: bool) -> bool {
    let Some(body) = view::seat_body(world, seat) else {
        return false;
    };
    let Some(e) = facts::live(world, body) else {
        return false;
    };
    let Some(mut o) = world.get_mut::<Observer>(e) else {
        return false;
    };
    if o.omniscient != on {
        o.omniscient = on;
        if let Some(mut m) = world.get_mut::<ObserverMemory>(e) {
            m.entries.clear();
        }
        if let Some(mut r) = world.get_mut::<ObserverEvents>(e) {
            r.ring.clear();
        }
    }
    true
}
