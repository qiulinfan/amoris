//! The hierarchy a scene brings (docs/spec/simulation.md 7.4): a scene's `Parent`s are checked
//! together once its entities are in the world, since one may name an entity that comes later in
//! the file. `world_edit` checks its own as it plans the call (edit.rs) and scripts theirs as
//! they stage them (amoris-script); the relation itself and its walks are amoris-sim's
//! (`entity::hierarchy`).

use amoris_contract::{Pointer, Problem};
use amoris_sim::entity::{self, hierarchy};
use amoris_sim::{EntityId, Hierarchy, Parent, Persisted};
use bevy_ecs::prelude::World;
use serde_json::json;

/// Checks the `Parent` of every entity of a scene just spawned (`ids`, in file order): it names a
/// live entity (`sim.entity_not_found`, or `sim.entity_id_invalid` for a place past the file) and
/// closes no loop (`sim.parent_cycle`). The refusal's `path` is the parent in the scene,
/// `/entities/<i>/components/Parent/parent`.
pub(crate) fn check_scene(world: &World, ids: &[EntityId]) -> Result<(), Problem> {
    let h = Hierarchy::read(world);
    if h.is_empty() {
        return Ok(());
    }
    for (i, id) in ids.iter().enumerate() {
        let Some(parent) = h.parent(*id) else {
            continue;
        };
        let refused = |mut p: Problem| {
            let entity = Pointer::root().key("entities").index(i);
            let at = entity.key("components").key(Parent::NAME).key("parent");
            p.detail.insert("path".into(), json!(at.to_string()));
            p
        };
        entity::require(world, parent).map_err(refused)?;
        if let Some(chain) = h.loop_through(*id, parent) {
            return Err(refused(hierarchy::parent_cycle(*id, parent, &chain)));
        }
    }
    Ok(())
}
