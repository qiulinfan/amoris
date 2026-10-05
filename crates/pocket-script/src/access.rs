//! Component access by schema (docs/spec/script-host.md 7.1 and 7.3): the same slot operations for
//! engine components (Rust types, through accessors the crate that owns them registers) and project
//! components (dynamic `bevy_ecs` components of layout `ProjectValues`), so a script sees no
//! difference. The accessors live in the `ScriptAccess` resource (class Ignored: code, not state).

use std::alloc::Layout;
use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_ecs::component::{
    Component, ComponentCloneBehavior, ComponentDescriptor, ComponentId, Mutable, StorageType,
};
use bevy_ecs::prelude::{Resource, World};
use bevy_ecs::ptr::OwningPtr;
use bevy_ecs::world::{EntityRef, EntityWorldMut};
use pocket_contract::{Problem, detail};
use pocket_sim::ComponentRegistry;
use pocket_sim::registry::{ComponentSchema, FieldType, FieldValue, ProjectValues};
use serde_json::json;

/// Whole-component reads and writes through a component's schema: numeric slots in declaration
/// order, strings in declaration order.
pub trait ComponentAccess: Send + Sync + 'static {
    /// The component's values on an entity, or `None` when it lacks the component.
    fn read(&self, e: &EntityRef<'_>) -> Option<ProjectValues>;
    /// Overwrites the component's values; `false` when the entity lacks it.
    fn write(&self, e: &mut EntityWorldMut<'_>, v: &ProjectValues) -> bool;
    /// Inserts the component with these values.
    fn insert(&self, e: &mut EntityWorldMut<'_>, v: ProjectValues);
    /// Removes the component.
    fn remove(&self, e: &mut EntityWorldMut<'_>);
}

/// An engine component's slot functions, which the crate owning the type supplies (the derive of
/// script-host.md 7.2 will generate them).
pub struct EngineFns<C> {
    /// The values of a component, in slot and string order.
    pub read: fn(&C) -> ProjectValues,
    /// Writes values back into a component.
    pub write: fn(&mut C, &ProjectValues),
    /// A component from values.
    pub make: fn(&ProjectValues) -> C,
}

struct EngineAccess<C> {
    fns: EngineFns<C>,
}

impl<C: Component<Mutability = Mutable>> ComponentAccess for EngineAccess<C> {
    fn read(&self, e: &EntityRef<'_>) -> Option<ProjectValues> {
        e.get::<C>().map(|c| (self.fns.read)(c))
    }

    fn write(&self, e: &mut EntityWorldMut<'_>, v: &ProjectValues) -> bool {
        match e.get_mut::<C>() {
            Some(mut c) => {
                (self.fns.write)(&mut c, v);
                true
            }
            None => false,
        }
    }

    fn insert(&self, e: &mut EntityWorldMut<'_>, v: ProjectValues) {
        e.insert((self.fns.make)(&v));
    }

    fn remove(&self, e: &mut EntityWorldMut<'_>) {
        e.remove::<C>();
    }
}

/// A project component: a dynamic component holding `ProjectValues`.
struct ProjectAccess {
    id: ComponentId,
}

impl ComponentAccess for ProjectAccess {
    fn read(&self, e: &EntityRef<'_>) -> Option<ProjectValues> {
        let ptr = e.get_by_id(self.id).ok()?;
        // SAFETY: every project component is registered with ProjectValues' layout and drop.
        Some(unsafe { ptr.deref::<ProjectValues>() }.clone())
    }

    fn write(&self, e: &mut EntityWorldMut<'_>, v: &ProjectValues) -> bool {
        match e.get_mut_by_id(self.id) {
            Ok(m) => {
                // SAFETY: as in `read`.
                *unsafe { m.into_inner().deref_mut::<ProjectValues>() } = v.clone();
                true
            }
            Err(_) => false,
        }
    }

    fn insert(&self, e: &mut EntityWorldMut<'_>, v: ProjectValues) {
        OwningPtr::make(v, |ptr| {
            // SAFETY: the component was registered with ProjectValues' layout.
            unsafe {
                e.insert_by_id(self.id, ptr);
            }
        });
    }

    fn remove(&self, e: &mut EntityWorldMut<'_>) {
        e.remove_by_id(self.id);
    }
}

/// The accessors of every component scripts can name, by world component id, and the current
/// schema of each project component a swap kept (its docs and defaults may be newer than the
/// registry's entry, which cannot be replaced; hot-update.md 15, choice 6).
#[derive(Resource, Default, Clone)]
pub struct ScriptAccess {
    by_id: BTreeMap<ComponentId, Arc<dyn ComponentAccess>>,
    schemas: BTreeMap<ComponentId, Arc<ComponentSchema>>,
}

impl ScriptAccess {
    pub fn get(&self, id: ComponentId) -> Option<&Arc<dyn ComponentAccess>> {
        self.by_id.get(&id)
    }

    /// The schema scripts use for a component: the one a swap last kept, else none (the
    /// registry's then holds).
    pub fn schema(&self, id: ComponentId) -> Option<&Arc<ComponentSchema>> {
        self.schemas.get(&id)
    }
}

fn access_mut(world: &mut World) -> bevy_ecs::prelude::Mut<'_, ScriptAccess> {
    world.get_resource_or_insert_with(ScriptAccess::default)
}

/// Makes the engine component `C` readable and writable by scripts. It must already be in the
/// registry with its `ComponentSchema` (`ComponentRegistry::register::<C>(world, Some(schema))`);
/// `fns` reads and writes its slots in that schema's order.
pub fn register_engine<C: Component<Mutability = Mutable>>(
    world: &mut World,
    fns: EngineFns<C>,
) -> Result<(), Problem> {
    let id = world.register_component::<C>();
    let known = world
        .get_resource::<ComponentRegistry>()
        .and_then(|r| r.by_id(id))
        .is_some_and(|e| e.script.is_some());
    if !known {
        return Err(Problem::new(
            "script.component_unregistered",
            format!(
                "{} has no script schema in the registry; register it with its ComponentSchema first.",
                std::any::type_name::<C>()
            ),
            detail([("type", json!(std::any::type_name::<C>()))]),
        ));
    }
    access_mut(world)
        .by_id
        .insert(id, Arc::new(EngineAccess { fns }));
    Ok(())
}

/// Drops a project component's values.
unsafe fn drop_project_values(ptr: OwningPtr<'_>) {
    // SAFETY: the pointer holds a ProjectValues (the layout every project component has).
    unsafe { ptr.drop_as::<ProjectValues>() };
}

/// Registers a project component (script-host.md 7.3): a dynamic `bevy_ecs` component named after
/// it with `ProjectValues`' layout, then its schema in the registry, then its accessor. A name the
/// registry already holds is `sim.component_duplicate`.
pub fn register_project(
    world: &mut World,
    schema: ComponentSchema,
) -> Result<ComponentId, Problem> {
    if let Some(e) = world
        .get_resource::<ComponentRegistry>()
        .and_then(|r| r.get(&schema.name))
    {
        return Err(Problem::new(
            "sim.component_duplicate",
            format!("A component named '{}' is already registered.", e.name),
            detail([("name", json!(&*e.name))]),
        ));
    }
    let name = format!("project:{}", schema.name);
    // SAFETY: the layout and the drop function are ProjectValues'; the values are Send + Sync.
    let descriptor = unsafe {
        ComponentDescriptor::new_with_layout(
            name,
            StorageType::Table,
            Layout::new::<ProjectValues>(),
            Some(drop_project_values),
            true,
            ComponentCloneBehavior::Default,
            None,
        )
    };
    let id = world.register_component_with_descriptor(descriptor);
    ComponentRegistry::register_project(world, schema, id)?;
    access_mut(world)
        .by_id
        .insert(id, Arc::new(ProjectAccess { id }));
    Ok(id)
}

/// Makes `schema` the one scripts use for the registered project component of its name: a swap that
/// kept the component (same version and fields) with other docs or defaults. Its `ComponentId` and
/// accessor stay; an unknown name changes nothing.
pub fn set_project_schema(world: &mut World, schema: Arc<ComponentSchema>) {
    let Some(id) = world
        .get_resource::<ComponentRegistry>()
        .and_then(|r| r.get(&schema.name))
        .map(|e| e.id)
    else {
        return;
    };
    access_mut(world).schemas.insert(id, schema);
}

/// A component's default values from its schema.
pub fn defaults(schema: &ComponentSchema) -> ProjectValues {
    let mut nums = Vec::with_capacity(usize::from(schema.slots()));
    let mut strs = Vec::new();
    for f in &schema.fields {
        match (&f.ty, &f.default) {
            (_, FieldValue::F64(x)) => nums.push(*x),
            (_, FieldValue::I32(x)) => nums.push(f64::from(*x)),
            (_, FieldValue::U32(x)) => nums.push(f64::from(*x)),
            (_, FieldValue::Tick(t)) => nums.push(t.to_f64()),
            (_, FieldValue::Bool(b)) => nums.push(if *b { 1.0 } else { 0.0 }),
            (_, FieldValue::Entity(e)) => nums.push(e.map_or(0.0, |e| e.to_f64())),
            (_, FieldValue::Enum(i)) => nums.push(f64::from(*i)),
            (_, FieldValue::Vec2(v)) => nums.extend_from_slice(v),
            (_, FieldValue::Vec3(v)) => nums.extend_from_slice(v),
            (_, FieldValue::Vec4(v)) | (_, FieldValue::Quat(v)) => nums.extend_from_slice(v),
            (FieldType::Str, FieldValue::Str(s)) => strs.push(Arc::from(&**s)),
            (_, FieldValue::Str(s)) => strs.push(Arc::from(&**s)),
        }
    }
    ProjectValues {
        nums: nums.into_boxed_slice(),
        strs: strs.into_boxed_slice(),
    }
}

/// The index of a field's first string among a schema's strings.
pub fn str_index(schema: &ComponentSchema, field: usize) -> usize {
    schema.fields[..field]
        .iter()
        .filter(|f| f.ty == FieldType::Str)
        .count()
}
