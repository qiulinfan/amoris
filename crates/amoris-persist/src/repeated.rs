//! Snapshots of one world taken again and again (docs/spec/persistence.md 4.3): the game thread
//! publishes one after every tick while a presenter reads them (threads.md 4.3), and most of a
//! world's component sections do not change from one tick to the next.
//!
//! [`SnapshotCache::snapshot`] gives the same snapshot as [`crate::snapshot`], byte for byte, but
//! takes over a registered component's section from its last snapshot when none of its rows was
//! added, changed or removed since, and keeps each component's query instead of making it again.
//! Whether rows changed is the ECS's change detection (`bevy_ecs` change ticks, which every write
//! through `Mut`, an insert or a spawn sets): a section is taken over when its row count is the
//! one encoded and no row, nor the `EntityId` of its entity, was added or changed after the world's
//! change tick of that snapshot. A removal without an addition changes the count; a removal and an
//! addition bring a row added after it. After each snapshot the world's change tick is advanced
//! (through `UnsafeWorldCell::increment_change_tick`, which touches no world state, the world hash or
//! a replay), so a write made later always carries a newer tick than the snapshot's.
//!
//! Resources, caches, the entities section and project components are encoded every time (they
//! are cheap, or their codec reads more than one place). The list of sections is kept while the
//! world's component registry is not changed. A cache follows one world and one registry: another
//! one, or a failed snapshot, starts it over.

use std::borrow::Cow;

use amoris_contract::Problem;
use amoris_sim::registry::ComponentRegistry;
use bevy_ecs::change_detection::{CHECK_TICK_THRESHOLD, Tick};
use bevy_ecs::prelude::World;
use bevy_ecs::world::WorldId;

use crate::hash::digest_prefix;
use crate::project::Project;
use crate::registry::Registry;
use crate::sections::{Codec, ComponentRows};
use crate::snapshot::{SectionData, Snapshot, Source, at_boundary, header_of, sources};

/// A section in the kept order.
enum Kept {
    /// The registry's entry at this index.
    Registered(usize),
    Project(Project),
}

/// A registered component's kept query and what it gave last.
struct Slot {
    rows: Box<dyn ComponentRows>,
    /// The rows encoded at the last snapshot (`None`: never encoded since the cache started).
    count: Option<usize>,
    /// The section then (`None`: absent, no rows).
    last: Option<SectionData>,
}

/// Snapshots of one world, each the same as [`crate::snapshot`] gives (module documentation).
#[derive(Default)]
pub struct SnapshotCache {
    /// The world and registry described (the registry by address).
    owner: Option<(WorldId, usize)>,
    /// The world's change tick at the last snapshot: rows changed after it are newer.
    taken: Tick,
    /// The sections in order, kept while the component registry does not change.
    order: Option<Vec<Kept>>,
    /// Per registry entry, for components.
    slots: Vec<Option<Slot>>,
}

impl SnapshotCache {
    pub fn new() -> SnapshotCache {
        SnapshotCache::default()
    }

    /// The world's snapshot. Pure as [`crate::snapshot`] is, apart from the world's change tick,
    /// which advances by one.
    pub fn snapshot(&mut self, world: &World, reg: &Registry) -> Result<Snapshot, Problem> {
        let r = self.take(world, reg);
        if r.is_err() {
            self.owner = None;
        }
        r
    }

    fn start(&mut self, owner: (WorldId, usize), reg: &Registry) {
        self.owner = Some(owner);
        self.order = None;
        self.slots = reg
            .entries()
            .iter()
            .map(|e| match &e.codec {
                Codec::Typed { rows: Some(f), .. } => Some(Slot {
                    rows: f(),
                    count: None,
                    last: None,
                }),
                _ => None,
            })
            .collect();
    }

    fn take(&mut self, world: &World, reg: &Registry) -> Result<Snapshot, Problem> {
        at_boundary(world)?;
        reg.check_classes(world)?;
        reg.check_rows(world)?;
        let now = world.read_change_tick();
        let owner = (world.id(), std::ptr::from_ref(reg) as usize);
        // Ticks stay comparable within bevy's change age; a cache older than that starts over.
        let age = now.get().wrapping_sub(self.taken.get());
        if self.owner != Some(owner) || age > CHECK_TICK_THRESHOLD {
            self.start(owner, reg);
        }
        let since = self.taken;
        let registry_changed = world
            .get_resource_change_ticks::<ComponentRegistry>()
            .is_some_and(|t| t.changed.is_newer_than(since, now));
        if self.order.is_none() || registry_changed {
            self.order = Some(
                sources(world, reg)?
                    .into_iter()
                    .map(|s| match s {
                        Source::Registered(i, _) => Kept::Registered(i),
                        Source::Project(p) => Kept::Project(p.into_owned()),
                    })
                    .collect(),
            );
        }
        let order = self.order.as_deref().unwrap_or_default();
        let mut sections = Vec::with_capacity(order.len());
        let mut buf = Vec::with_capacity(4096);
        for k in order {
            let (source, slot) = match k {
                Kept::Registered(i) => (
                    Source::Registered(*i, &reg.entries()[*i]),
                    self.slots.get_mut(*i).and_then(Option::as_mut),
                ),
                Kept::Project(p) => (Source::Project(Cow::Borrowed(p)), None),
            };
            let Some(slot) = slot else {
                sections.extend(source.encode(world, &mut buf)?);
                continue;
            };
            if let Some(count) = slot.count {
                let (rows, changed) = slot.rows.scan(world, since, now);
                if !changed && rows == count {
                    sections.extend(slot.last.clone());
                    continue;
                }
            }
            let key = source.key();
            buf.clear();
            digest_prefix(&mut buf, key.kind, &key.name, source.version());
            let start = buf.len();
            let rows = slot.rows.encode(world, &mut buf, key)?;
            slot.count = Some(rows);
            slot.last = (rows > 0).then(|| source.data(&buf, start));
            sections.extend(slot.last.clone());
        }
        // Writes from now on carry a newer tick than every change this snapshot saw.
        self.taken = world
            .as_unsafe_world_cell_readonly()
            .increment_change_tick();
        Ok(Snapshot::from_parts(header_of(world), sections))
    }
}
