//! Helpers the script host's tests share: a project from inline files, a world with the scripts
//! installed, and a hash of what scripts can change.
#![allow(dead_code)]

use amoris_script::{CompiledSet, ScriptLimits, ScriptSource};
use amoris_sim::{ComponentRegistry, Sim, SimConfig, StepReport, TickRate};
use bevy_ecs::prelude::World;

pub fn source(files: &[(&str, &str)]) -> ScriptSource {
    let mut s = ScriptSource::new();
    for (path, text) in files {
        s = s.with(path, text);
    }
    s
}

#[cfg(feature = "transpile")]
pub fn compiled(files: &[(&str, &str)]) -> CompiledSet {
    amoris_script::compile(&source(files), &Default::default()).unwrap_or_else(|e| panic!("{e:#?}"))
}

#[cfg(feature = "transpile")]
pub fn compiled_lint_off(files: &[(&str, &str)]) -> CompiledSet {
    let options = amoris_script::CompileOptions { lint_off: true };
    amoris_script::compile(&source(files), &options).unwrap_or_else(|e| panic!("{e:#?}"))
}

/// A world at tick 0 with a script host and `set` installed.
pub fn sim_with(set: &CompiledSet, limits: ScriptLimits, seed: u64) -> Sim {
    let mut sim = Sim::new(SimConfig {
        rate: TickRate::DEFAULT,
        seed,
    })
    .unwrap();
    amoris_script::install(&mut sim, limits).unwrap();
    amoris_script::swap(sim.world_mut(), set, false).unwrap_or_else(|e| panic!("{e:#?}"));
    sim
}

pub fn step(sim: &mut Sim) -> StepReport {
    sim.step(&mut amoris_sim::NoHooks)
        .unwrap_or_else(|p| panic!("{p:#?}"))
}

/// A hash of what scripts can change (`amoris_script::web::world_digest`).
pub fn hash(world: &World) -> u64 {
    amoris_script::web::world_digest(world)
}

/// The values of a component on an entity, through the script accessors.
pub fn component_values(
    world: &World,
    id: amoris_sim::EntityId,
    name: &str,
) -> Option<amoris_sim::registry::ProjectValues> {
    let entry = world.resource::<ComponentRegistry>().get(name)?;
    let access = world
        .get_resource::<amoris_script::ScriptAccess>()?
        .get(entry.id)?
        .clone();
    let e = amoris_sim::entity::entity(world, id)?;
    access.read(&world.entity(e))
}

/// A component's fields as JSON: reals as floats, integers as integers, enums by name, entities as
/// ids or null.
pub fn component_json(
    world: &World,
    id: amoris_sim::EntityId,
    name: &str,
) -> Option<serde_json::Value> {
    use amoris_sim::registry::FieldType;
    let schema = world
        .resource::<ComponentRegistry>()
        .get(name)?
        .script
        .clone()?;
    let v = component_values(world, id, name)?;
    let mut out = serde_json::Map::new();
    let mut s = 0;
    for f in schema.fields.iter() {
        let slot = usize::from(f.first_slot);
        let x = v.nums.get(slot).copied().unwrap_or(0.0);
        #[allow(clippy::cast_possible_truncation)]
        let value = match &f.ty {
            FieldType::F64 => serde_json::json!(x),
            FieldType::I32 | FieldType::U32 | FieldType::Tick => serde_json::json!(x as i64),
            FieldType::Bool => serde_json::json!(x != 0.0),
            FieldType::Enum(names) => serde_json::json!(&*names[x as usize]),
            FieldType::Entity => {
                if x == 0.0 {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(x as i64)
                }
            }
            FieldType::Str => {
                s += 1;
                serde_json::json!(&*v.strs[s - 1])
            }
            _ => serde_json::json!(v.nums[slot..slot + usize::from(f.ty.slots())]),
        };
        out.insert(f.name.to_string(), value);
    }
    Some(serde_json::Value::Object(out))
}
