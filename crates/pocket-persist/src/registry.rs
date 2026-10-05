//! The persistence registry (docs/spec/persistence.md 2 and 6.2): every type that can appear in a
//! world in exactly one class, the codec of each persisted section, the rebuild functions restore
//! runs, and the check that a world holds no type in no class.

use std::any::{TypeId, type_name};
use std::sync::Mutex;

use bevy_ecs::component::ComponentId;
use bevy_ecs::entity_disabling::DefaultQueryFilters;
use bevy_ecs::prelude::{Component, Resource, World};
use bevy_ecs::resource::IsResource;
use bevy_ecs::world::WorldId;
use pocket_contract::Problem;
use pocket_sim::registry::{ComponentOrigin, ComponentRegistry};
use pocket_sim::{EntityAllocator, EntityId, Persisted, PersistedCache, RegisterPersisted};

use crate::error;
use crate::format::{self, Fingerprint, ResolvedFormat};
use crate::hash::{SectionKey, SectionKind, valid_section_name};
use crate::sections::{self, Codec};
use crate::snapshot::SnapshotContext;

/// A function restore runs after applying the sections.
pub type RebuildFn = fn(&mut World);

/// A type's class (persistence.md 2).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    Entities,
    Resource,
    Component,
    Cache,
    Derived,
    Ignored,
}

/// One persisted section type.
pub struct Entry {
    pub key: SectionKey,
    pub version: u32,
    pub fingerprint: Fingerprint,
    /// `None` for caches.
    pub format: Option<ResolvedFormat>,
    /// Caches only (versions.md 3.6).
    pub identity: Option<String>,
    pub(crate) codec: Codec,
}

struct Classified {
    type_id: TypeId,
    class: Class,
    name: &'static str,
    /// A persisted component's section name.
    section: Option<&'static str>,
}

/// Builds a [`Registry`]; crates declare their types through [`RegisterPersisted`].
#[derive(Default)]
pub struct RegistryBuilder {
    entries: Vec<Entry>,
    classes: Vec<Classified>,
    rebuilds: Vec<(&'static str, RebuildFn)>,
    errors: Vec<Problem>,
}

/// The registry: immutable once built, shared by every world of a process (`Send + Sync`).
pub struct Registry {
    entries: Vec<Entry>,
    classes: Vec<Classified>,
    rebuilds: Vec<(&'static str, RebuildFn)>,
    /// Archetypes already checked per world (the last few worlds), so a steady world pays nothing.
    checked: Mutex<Vec<(WorldId, usize)>>,
}

impl RegistryBuilder {
    pub fn new() -> RegistryBuilder {
        let mut b = RegistryBuilder::default();
        b.ignore::<IsResource>("bevy_ecs's marker on the entity that holds a resource");
        b.ignore::<DefaultQueryFilters>("bevy_ecs's query filters, set when a world is created");
        b.ignore::<SnapshotContext>("header metadata: the bundle and the boundary's write count");
        b
    }

    fn classify<T: 'static>(&mut self, class: Class) {
        let type_id = TypeId::of::<T>();
        if self.classes.iter().any(|c| c.type_id == type_id) {
            self.errors.push(error::duplicate_name(type_name::<T>()));
            return;
        }
        self.classes.push(Classified {
            type_id,
            class,
            name: type_name::<T>(),
            section: None,
        });
    }

    fn add(&mut self, entry: Entry) {
        if !valid_section_name(&entry.key.name) {
            self.errors.push(error::bad_type(
                &entry.key.name,
                "its name does not match [A-Za-z][A-Za-z0-9_:.]{0,63}",
            ));
        } else if self.entries.iter().any(|e| e.key == entry.key) {
            self.errors.push(error::duplicate_name(&entry.key.name));
        } else {
            self.entries.push(entry);
        }
    }

    fn typed<T: Persisted>(&mut self, kind: SectionKind, codec: Codec) {
        if T::VERSION == 0 {
            self.errors.push(error::bad_type(
                T::NAME,
                "its schema version must be at least 1",
            ));
            return;
        }
        match format::trace::<T>() {
            Ok(f) => self.add(Entry {
                key: SectionKey::new(kind, T::NAME),
                version: T::VERSION,
                fingerprint: Fingerprint::of(&f),
                format: Some(f),
                identity: None,
                codec,
            }),
            Err(p) => self.errors.push(p),
        }
    }

    /// The registry, or the first problem met while declaring (`persist.type`,
    /// `persist.duplicate_name`).
    pub fn build(mut self) -> Result<Registry, Problem> {
        if let Some(p) = self.errors.into_iter().next() {
            return Err(p);
        }
        self.entries.sort_by(|a, b| a.key.cmp(&b.key));
        self.classes.sort_by_key(|c| c.type_id);
        Ok(Registry {
            entries: self.entries,
            classes: self.classes,
            rebuilds: self.rebuilds,
            checked: Mutex::new(Vec::new()),
        })
    }
}

impl RegisterPersisted for RegistryBuilder {
    fn entities(&mut self) -> &mut Self {
        self.classify::<EntityId>(Class::Entities);
        self.classify::<EntityAllocator>(Class::Entities);
        let f = format::entities_format();
        self.add(Entry {
            key: SectionKey::entities(),
            version: 1,
            fingerprint: Fingerprint::of(&f),
            format: Some(f),
            identity: None,
            codec: Codec::Entities,
        });
        self
    }

    fn resource<R: Persisted + Resource>(&mut self) -> &mut Self {
        self.classify::<R>(Class::Resource);
        self.typed::<R>(SectionKind::Resource, sections::resource_codec::<R>());
        self
    }

    fn component<C: Persisted + Component>(&mut self) -> &mut Self {
        self.classify::<C>(Class::Component);
        if let Some(c) = self
            .classes
            .last_mut()
            .filter(|c| c.type_id == TypeId::of::<C>())
        {
            c.section = Some(C::NAME);
        }
        self.typed::<C>(SectionKind::Component, sections::component_codec::<C>());
        self
    }

    fn cache<K: PersistedCache>(&mut self) -> &mut Self {
        self.classify::<K>(Class::Cache);
        let identity = K::identity();
        self.add(Entry {
            key: SectionKey::new(SectionKind::Cache, K::NAME),
            version: 0,
            fingerprint: Fingerprint::of_identity(identity),
            format: None,
            identity: Some(identity.to_owned()),
            codec: sections::cache_codec::<K>(),
        });
        self
    }

    fn derived<T: 'static>(&mut self, _reason: &'static str) -> &mut Self {
        self.classify::<T>(Class::Derived);
        self
    }

    fn ignore<T: 'static>(&mut self, _reason: &'static str) -> &mut Self {
        self.classify::<T>(Class::Ignored);
        self
    }

    fn rebuild(&mut self, name: &'static str, f: fn(&mut World)) -> &mut Self {
        self.rebuilds.push((name, f));
        self
    }
}

impl Registry {
    /// The registered sections, in canonical section order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The entry of a section key.
    pub fn entry(&self, key: &SectionKey) -> Option<&Entry> {
        self.entries
            .binary_search_by(|e| e.key.cmp(key))
            .ok()
            .map(|i| &self.entries[i])
    }

    /// The class of a Rust type, if registered.
    pub fn class_of(&self, type_id: TypeId) -> Option<Class> {
        self.classes
            .binary_search_by_key(&type_id, |c| c.type_id)
            .ok()
            .map(|i| self.classes[i].class)
    }

    pub(crate) fn rebuilds(&self) -> &[(&'static str, RebuildFn)] {
        &self.rebuilds
    }

    /// Derived types, removed by restore before the rebuild functions put them back.
    pub(crate) fn derived(&self) -> impl Iterator<Item = TypeId> + '_ {
        self.classes
            .iter()
            .filter(|c| matches!(c.class, Class::Derived | Class::Cache))
            .map(|c| c.type_id)
    }

    /// `persist.unclassified` for the first component type of the world's archetypes created
    /// since the last check of this world that is in no class. A dynamic component (no Rust type)
    /// is classified when the world's component registry names it a project component.
    pub fn check_classes(&self, world: &World) -> Result<(), Problem> {
        let id = world.id();
        let total = world.archetypes().len();
        let from = self
            .checked
            .lock()
            .map(|c| c.iter().find(|(w, _)| *w == id).map_or(0, |(_, n)| *n))
            .unwrap_or(0);
        if from >= total {
            return Ok(());
        }
        let projects = world.get_resource::<ComponentRegistry>();
        for archetype in world.archetypes().iter().skip(from) {
            for &component in archetype.components() {
                self.check_component(world, projects, component)?;
            }
        }
        if let Ok(mut c) = self.checked.lock() {
            c.retain(|(w, _)| *w != id);
            if c.len() >= 16 {
                c.remove(0);
            }
            c.push((id, total));
        }
        Ok(())
    }

    /// `persist.encode` when a persisted component (a registered type or a project component) sits
    /// on an entity without an `EntityId`: its row would be in no section and no hash, and restore,
    /// which despawns the entities that have one, would leave it (2: state is never skipped
    /// silently). One pass over the world's archetypes; snapshot, the hash and restore run it.
    pub fn check_rows(&self, world: &World) -> Result<(), Problem> {
        let with_id = world.components().get_id(TypeId::of::<EntityId>());
        let projects = world.get_resource::<ComponentRegistry>();
        for archetype in world.archetypes().iter() {
            if archetype.is_empty() || with_id.is_some_and(|c| archetype.contains(c)) {
                continue;
            }
            for &component in archetype.components() {
                let Some(info) = world.components().get_info(component) else {
                    continue;
                };
                let section = match info.type_id() {
                    Some(t) => self
                        .classes
                        .binary_search_by_key(&t, |c| c.type_id)
                        .ok()
                        .and_then(|i| self.classes[i].section)
                        .map(|name| SectionKey::new(SectionKind::Component, name)),
                    None => projects
                        .and_then(|r| r.by_id(component))
                        .filter(|e| e.origin == ComponentOrigin::Project)
                        .map(|e| SectionKey::new(SectionKind::Component, &e.name)),
                };
                if let Some(key) = section {
                    return Err(error::encode(
                        &key.to_string(),
                        None,
                        "a row on an entity without an EntityId (spawned outside pocket_sim::entity)",
                    ));
                }
            }
        }
        Ok(())
    }

    fn check_component(
        &self,
        world: &World,
        projects: Option<&ComponentRegistry>,
        component: ComponentId,
    ) -> Result<(), Problem> {
        let Some(info) = world.components().get_info(component) else {
            return Ok(());
        };
        match info.type_id() {
            Some(t) if self.class_of(t).is_some() => Ok(()),
            Some(t) => Err(error::unclassified(&format!(
                "the component type {} ({t:?})",
                info.name()
            ))),
            None => {
                let project = projects
                    .and_then(|r| r.by_id(component))
                    .is_some_and(|e| e.origin == ComponentOrigin::Project);
                if project {
                    Ok(())
                } else {
                    Err(error::unclassified(&format!(
                        "the dynamic component {} (component id {})",
                        info.name(),
                        component.index()
                    )))
                }
            }
        }
    }

    /// Every classified Rust type's name and class (P6 lists them).
    pub fn classes(&self) -> Vec<(&'static str, Class)> {
        let mut out: Vec<(&'static str, Class)> =
            self.classes.iter().map(|c| (c.name, c.class)).collect();
        out.sort_by_key(|c| c.0);
        out
    }

    /// The `(key, version, fingerprint)` of every registered section, for comparing registries.
    pub fn schema(&self) -> Vec<(SectionKey, u32, Fingerprint)> {
        self.entries
            .iter()
            .map(|e| (e.key.clone(), e.version, e.fingerprint))
            .collect()
    }
}
