//! Project components (docs/spec/persistence.md 6.2, open choice 8): dynamic `bevy_ecs` components
//! of layout `ProjectValues`, described by the `ComponentSchema` in the world's component registry,
//! which persistence reads without the script host. A row's bytes are the PCE of a struct of the
//! declared fields in declared order, each as its Rust counterpart (`format::field_format`).

use std::alloc::Layout;
use std::sync::Arc;

use bevy_ecs::component::ComponentId;
use bevy_ecs::prelude::World;
use bevy_ecs::ptr::OwningPtr;
use pocket_contract::Problem;
use pocket_sim::registry::{
    ComponentOrigin, ComponentRegistry, ComponentSchema, FieldType, ProjectValues,
};
use pocket_sim::{EntityId, EntityIndex, MAX_ENTITY_ID, Tick};

use crate::error;
use crate::format::{Fingerprint, ResolvedFormat, schema_format};
use crate::hash::{SectionKey, SectionKind};
use crate::pce::{Decoder, PceError, write_uleb};
use crate::sections::{IdMap, StagedSection, read_rows};

/// A project component of one world: its component id there and its schema.
#[derive(Clone)]
pub(crate) struct Project {
    pub id: ComponentId,
    pub schema: Arc<ComponentSchema>,
    pub key: SectionKey,
    pub format: ResolvedFormat,
    pub fingerprint: Fingerprint,
}

/// The world's project components, ascending by name. A component registered with another layout
/// than `ProjectValues` is refused (`persist.type`): its bytes could not be read.
pub(crate) fn projects(world: &World) -> Result<Vec<Project>, Problem> {
    let Some(reg) = world.get_resource::<ComponentRegistry>() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for e in reg.entries() {
        if e.origin != ComponentOrigin::Project {
            continue;
        }
        let Some(schema) = e.script.clone() else {
            return Err(error::bad_type(
                &e.name,
                "a project component without a schema",
            ));
        };
        let layout = world.components().get_info(e.id).map(|i| i.layout());
        if layout != Some(Layout::new::<ProjectValues>()) {
            return Err(error::bad_type(
                &e.name,
                "a project component not laid out as ProjectValues",
            ));
        }
        let format = schema_format(&schema);
        out.push(Project {
            id: e.id,
            key: SectionKey::new(SectionKind::Component, &e.name),
            fingerprint: Fingerprint::of(&format),
            format,
            schema,
        });
    }
    Ok(out)
}

fn reason(field: &str, why: &str) -> String {
    format!("field '{field}' {why}")
}

/// Writes one value by its schema.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // integral and range-checked
fn write_value(
    schema: &ComponentSchema,
    v: &ProjectValues,
    out: &mut Vec<u8>,
) -> Result<(), String> {
    let mut strs = v.strs.iter();
    for f in &schema.fields {
        let at = usize::from(f.first_slot);
        let slots = usize::from(f.ty.slots());
        let nums = v
            .nums
            .get(at..at + slots)
            .ok_or_else(|| reason(&f.name, "has no slot"))?;
        let int = |lo: f64, hi: f64| -> Result<f64, String> {
            let x = nums[0];
            if x.is_finite() && x == x.trunc() && (lo..=hi).contains(&x) {
                Ok(x)
            } else {
                Err(reason(
                    &f.name,
                    &format!("holds {x}, not an integer from {lo} to {hi}"),
                ))
            }
        };
        if nums.iter().any(|x| !x.is_finite()) {
            return Err(reason(&f.name, "holds a NaN or an infinity"));
        }
        match &f.ty {
            FieldType::F64 => out.extend_from_slice(&nums[0].to_bits().to_le_bytes()),
            FieldType::I32 => {
                let x = int(f64::from(i32::MIN), f64::from(i32::MAX))? as i32;
                out.extend_from_slice(&x.to_le_bytes());
            }
            FieldType::U32 => {
                let x = int(0.0, f64::from(u32::MAX))? as u32;
                out.extend_from_slice(&x.to_le_bytes());
            }
            FieldType::Tick => {
                let x = int(0.0, Tick::MAX.to_f64())? as u64;
                out.extend_from_slice(&x.to_le_bytes());
            }
            FieldType::Bool => out.push(int(0.0, 1.0)? as u8),
            FieldType::Entity => {
                #[allow(clippy::cast_precision_loss)] // 2^53 - 1 is exact
                let x = int(0.0, MAX_ENTITY_ID as f64)? as u64;
                if x == 0 {
                    out.push(0);
                } else {
                    out.push(1);
                    out.extend_from_slice(&x.to_le_bytes());
                }
            }
            FieldType::Enum(variants) => {
                #[allow(clippy::cast_precision_loss)] // a variant count
                let x = int(0.0, (variants.len() as f64) - 1.0)? as u64;
                write_uleb(out, x);
            }
            FieldType::Str => {
                let s = strs
                    .next()
                    .ok_or_else(|| reason(&f.name, "has no string"))?;
                write_uleb(out, s.len() as u64);
                out.extend_from_slice(s.as_bytes());
            }
            FieldType::Vec2 | FieldType::Vec3 | FieldType::Vec4 | FieldType::Quat => {
                for x in nums {
                    out.extend_from_slice(&x.to_bits().to_le_bytes());
                }
            }
        }
    }
    Ok(())
}

/// Reads one value by its schema.
fn read_value(schema: &ComponentSchema, d: &mut Decoder<'_>) -> Result<ProjectValues, PceError> {
    let mut nums = vec![0.0; usize::from(schema.slots())];
    let mut strs: Vec<Arc<str>> = Vec::new();
    for f in &schema.fields {
        let at = usize::from(f.first_slot);
        let slot = &mut nums[at..at + usize::from(f.ty.slots())];
        let pos = d.pos();
        match &f.ty {
            FieldType::F64 => slot[0] = d.value::<f64>()?,
            FieldType::I32 => slot[0] = f64::from(d.value::<i32>()?),
            FieldType::U32 => slot[0] = f64::from(d.value::<u32>()?),
            FieldType::Tick => slot[0] = d.value::<Tick>()?.to_f64(),
            FieldType::Bool => slot[0] = f64::from(u8::from(d.value::<bool>()?)),
            FieldType::Entity => {
                slot[0] = d.value::<Option<EntityId>>()?.map_or(0.0, EntityId::to_f64)
            }
            FieldType::Enum(variants) => {
                let i = d.uleb()?;
                if usize::try_from(i).map_or(true, |i| i >= variants.len()) {
                    return Err(PceError::noncanonical(
                        pos,
                        format!("variant {i} of field '{}'", f.name),
                    ));
                }
                #[allow(clippy::cast_precision_loss)] // a variant index
                let x = i as f64;
                slot[0] = x;
            }
            FieldType::Str => strs.push(Arc::from(d.value::<&str>()?)),
            FieldType::Vec2 | FieldType::Vec3 | FieldType::Vec4 | FieldType::Quat => {
                for x in slot.iter_mut() {
                    *x = d.value::<f64>()?;
                }
            }
        }
    }
    Ok(ProjectValues {
        nums: nums.into_boxed_slice(),
        strs: strs.into_boxed_slice(),
    })
}

/// Writes the rows of one project component, live entities in id order; `false` when none.
pub(crate) fn encode(world: &World, p: &Project, out: &mut Vec<u8>) -> Result<bool, Problem> {
    let Some(index) = world.get_resource::<EntityIndex>() else {
        return Ok(false);
    };
    let count_at = out.len();
    let mut rows = 0u64;
    let mut body = Vec::new();
    for (id, entity) in index.iter() {
        let Ok(e) = world.get_entity(entity) else {
            continue;
        };
        let Ok(ptr) = e.get_by_id(p.id) else {
            continue;
        };
        // SAFETY: `projects` checked that the component is laid out as ProjectValues, the one
        // layout every project component shares (script-host.md 7.3).
        let v = unsafe { ptr.deref::<ProjectValues>() };
        body.extend_from_slice(&id.get().to_le_bytes());
        write_value(&p.schema, v, &mut body)
            .map_err(|why| error::encode(&p.key.to_string(), Some(id.get()), &why))?;
        rows += 1;
    }
    if rows == 0 {
        return Ok(false);
    }
    out.truncate(count_at);
    write_uleb(out, rows);
    out.extend_from_slice(&body);
    Ok(true)
}

struct StagedProject {
    id: ComponentId,
    rows: Vec<(EntityId, ProjectValues)>,
}

impl StagedSection for StagedProject {
    fn apply(self: Box<Self>, world: &mut World, ids: &mut IdMap) {
        for (eid, v) in self.rows {
            let entity = ids.get(eid);
            OwningPtr::make(v, |ptr| {
                // SAFETY: the target world registered this component as ProjectValues (checked by
                // `projects`), and `ptr` owns a ProjectValues.
                unsafe {
                    world.entity_mut(entity).insert_by_id(self.id, ptr);
                }
            });
        }
    }
}

/// Decodes rows of a project component for the world whose project component `p` is.
pub(crate) fn decode(
    p: &Project,
    bytes: &[u8],
    live: &[EntityId],
) -> Result<Box<dyn StagedSection>, Problem> {
    let rows = read_rows(bytes, &p.key, live, |d| read_value(&p.schema, d))?;
    // encode(decode(b)) == b: re-encode and compare (3.6).
    let mut again = Vec::with_capacity(bytes.len());
    write_uleb(&mut again, rows.len() as u64);
    for (id, v) in &rows {
        again.extend_from_slice(&id.get().to_le_bytes());
        write_value(&p.schema, v, &mut again).map_err(|why| error::noncanonical(0, &why))?;
    }
    if again != bytes {
        return Err(error::noncanonical(
            0,
            &format!(
                "in section {}: the rows do not encode back to their bytes",
                p.key
            ),
        ));
    }
    Ok(Box::new(StagedProject { id: p.id, rows }))
}
