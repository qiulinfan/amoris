//! One script call as a transaction (docs/spec/script-host.md 5.4): what it has staged (column
//! write-backs, commands, events), the overlay its own commands make of the world, and the commit
//! that applies them all or the rollback that applies none.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_ecs::component::ComponentId;
use bevy_ecs::prelude::World;
use pocket_sim::entity;
use pocket_sim::event::{self, NewEvent};
use pocket_sim::registry::{ComponentSchema, ProjectValues};
use pocket_sim::rng::StreamKey;
use pocket_sim::{ComponentRegistry, EntityId, Tick};
use rquickjs::{Object, Persistent};
use serde_json::json;

use crate::access::{ComponentAccess, ScriptAccess};
use crate::error::{ErrorPhase, ScriptError};

/// A component a call names, resolved in its world.
#[derive(Clone)]
pub(crate) struct Comp {
    pub id: ComponentId,
    pub schema: Arc<ComponentSchema>,
    pub access: Arc<dyn ComponentAccess>,
}

/// `script.unknown_component`, with the nearest names.
pub(crate) fn unknown_component(world: &World, name: &str) -> ScriptError {
    let suggestions = world
        .get_resource::<ComponentRegistry>()
        .map(|r| r.suggest(name))
        .unwrap_or_default();
    ScriptError::new(
        "script.unknown_component",
        format!("There is no component named '{name}' that scripts can use."),
        ErrorPhase::Run,
    )
    .component(name)
    .suggest(suggestions)
}

/// Resolves a component name against the world's registry and accessors.
pub(crate) fn resolve_comp(world: &World, name: &str) -> Result<Comp, ScriptError> {
    let entry = world
        .get_resource::<ComponentRegistry>()
        .and_then(|r| r.get(name));
    let found = entry.and_then(|e| {
        let accessors = world.get_resource::<ScriptAccess>()?;
        let schema = accessors
            .schema(e.id)
            .cloned()
            .or_else(|| e.script.clone())?;
        let access = accessors.get(e.id)?.clone();
        Some(Comp {
            id: e.id,
            schema,
            access,
        })
    });
    found.ok_or_else(|| unknown_component(world, name))
}

/// What the call's own commands make of an entity.
#[derive(Clone, Debug, Default)]
pub(crate) struct EntityOverlay {
    pub alive: bool,
    /// Spawned by this call: components not listed are absent.
    pub spawned: bool,
    pub comps: BTreeMap<ComponentId, Option<ProjectValues>>,
}

/// The world as the call started with its own commands applied in call order.
#[derive(Default)]
pub(crate) struct Overlay {
    pub entities: BTreeMap<EntityId, EntityOverlay>,
}

impl Overlay {
    pub fn alive(&self, world: &World, id: EntityId) -> bool {
        match self.entities.get(&id) {
            Some(o) => o.alive,
            None => entity::entity(world, id).is_some(),
        }
    }

    /// The component's values on the entity, or `None` when it is gone or lacks it.
    pub fn value(&self, world: &World, id: EntityId, comp: &Comp) -> Option<ProjectValues> {
        if let Some(o) = self.entities.get(&id) {
            if !o.alive {
                return None;
            }
            if let Some(v) = o.comps.get(&comp.id) {
                return v.clone();
            }
            if o.spawned {
                return None;
            }
        }
        let e = entity::entity(world, id)?;
        comp.access.read(&world.entity(e))
    }

    pub fn put(&mut self, id: EntityId, comp: ComponentId, value: Option<ProjectValues>) {
        let o = self.entities.entry(id).or_insert_with(|| EntityOverlay {
            alive: true,
            ..EntityOverlay::default()
        });
        o.comps.insert(comp, value);
    }

    pub fn despawn(&mut self, id: EntityId) {
        let o = self.entities.entry(id).or_default();
        o.alive = false;
        o.comps.clear();
    }

    pub fn spawn(&mut self, id: EntityId, comps: &[(Comp, ProjectValues)]) {
        self.entities.insert(
            id,
            EntityOverlay {
                alive: true,
                spawned: true,
                comps: comps.iter().map(|(c, v)| (c.id, Some(v.clone()))).collect(),
            },
        );
    }
}

/// A staged command, applied at commit in call order.
pub(crate) enum Command {
    /// A patch: changed numeric slots and strings.
    Set {
        id: EntityId,
        comp: Comp,
        nums: Vec<(u16, f64)>,
        strs: Vec<(usize, Arc<str>)>,
    },
    Insert {
        id: EntityId,
        comp: Comp,
        value: ProjectValues,
    },
    Remove {
        id: EntityId,
        comp: Comp,
    },
    Despawn {
        id: EntityId,
    },
    Spawn {
        id: EntityId,
        comps: Vec<(Comp, ProjectValues)>,
    },
    Emit(NewEvent),
}

/// A column handed to the script, kept to compare after the call.
pub(crate) struct Column {
    pub slot: u16,
    pub field: usize,
    pub original: Vec<f64>,
    /// The typed array, as its object (rquickjs keeps `TypedArray<f64>` out of `Persistent`).
    pub array: Persistent<Object<'static>>,
}

/// A query's columns: the rows' ids and, per component, its columns.
pub(crate) struct QueryRecord {
    pub ids: Vec<EntityId>,
    pub comps: Vec<(Comp, Vec<Column>)>,
}

/// One validated cell to write back.
pub(crate) struct CellWrite {
    pub id: EntityId,
    pub comp: Comp,
    pub slot: u16,
    pub value: f64,
}

/// What a call did, besides its steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CallCounts {
    pub host_calls: u32,
    pub rows: u32,
    pub cells_written: u32,
    pub commands: u32,
    pub events: u32,
}

/// The state natives reach during a system call.
pub(crate) struct CallState {
    /// The world, borrowed for the call: natives dereference it only while no JavaScript runs.
    pub world: *mut World,
    pub tick: Tick,
    /// The system's key, `script:<name>`, and its name.
    pub key: String,
    pub name: String,
    pub serial: u64,
    pub rng: Vec<StreamKey>,
    pub overlay: Overlay,
    pub commands: Vec<Command>,
    pub queries: Vec<QueryRecord>,
    pub counts: CallCounts,
}

impl CallState {
    pub fn new(world: &mut World, tick: Tick, name: &str, serial: u64) -> CallState {
        CallState {
            world,
            tick,
            key: format!("script:{name}"),
            name: name.to_owned(),
            serial,
            rng: Vec::new(),
            overlay: Overlay::default(),
            commands: Vec::new(),
            queries: Vec::new(),
            counts: CallCounts::default(),
        }
    }
}

fn entity_of(world: &World, id: EntityId) -> Result<bevy_ecs::prelude::Entity, ScriptError> {
    entity::entity(world, id).ok_or_else(|| {
        ScriptError::new(
            "sim.internal",
            format!(
                "Entity {} vanished between its check and the commit.",
                id.get()
            ),
            ErrorPhase::WriteBack,
        )
    })
}

/// Applies a validated call: the column write-backs (query by query, rows ascending, slots in
/// declaration order), then the commands in call order; a later write to a field wins.
pub(crate) fn apply(
    world: &mut World,
    cells: &[CellWrite],
    commands: Vec<Command>,
) -> Result<(), ScriptError> {
    for c in cells {
        let e = entity_of(world, c.id)?;
        let mut em = world.entity_mut(e);
        let Some(mut v) = c.comp.access.read(&em.as_readonly()) else {
            continue;
        };
        v.nums[usize::from(c.slot)] = c.value;
        c.comp.access.write(&mut em, &v);
    }
    for cmd in commands {
        match cmd {
            Command::Set {
                id,
                comp,
                nums,
                strs,
            } => {
                let e = entity_of(world, id)?;
                let mut em = world.entity_mut(e);
                if let Some(mut v) = comp.access.read(&em.as_readonly()) {
                    for (slot, x) in nums {
                        v.nums[usize::from(slot)] = x;
                    }
                    for (i, s) in strs {
                        v.strs[i] = s;
                    }
                    comp.access.write(&mut em, &v);
                }
            }
            Command::Insert { id, comp, value } => {
                let e = entity_of(world, id)?;
                comp.access.insert(&mut world.entity_mut(e), value);
            }
            Command::Remove { id, comp } => {
                if let Some(e) = entity::entity(world, id) {
                    comp.access.remove(&mut world.entity_mut(e));
                }
            }
            Command::Despawn { id } => {
                entity::despawn(world, id).map_err(|p| internal(&p.message))?;
            }
            Command::Spawn { id, comps } => {
                let e = entity::spawn_allocated(world, id, ()).map_err(|p| internal(&p.message))?;
                for (comp, value) in comps {
                    comp.access.insert(&mut world.entity_mut(e), value);
                }
            }
            Command::Emit(ev) => {
                event::emit(world, ev);
            }
        }
    }
    Ok(())
}

fn internal(message: &str) -> ScriptError {
    ScriptError::new("sim.internal", message.to_owned(), ErrorPhase::WriteBack)
        .with("message", json!(message))
}
