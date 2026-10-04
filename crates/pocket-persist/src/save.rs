//! Saves (docs/spec/versions.md 6): world data only, with the formats of its sections, loadable by
//! any later engine and scripts that can migrate it.

use std::collections::BTreeMap;
use std::sync::Arc;

use bevy_ecs::prelude::World;
use pocket_contract::Problem;
use pocket_sim::{ContentHash, EntityId, Tick};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error;
use crate::format::ResolvedFormat;
use crate::format::json::{from_json, to_json};
use crate::hash::{SectionKey, SectionKind, digest_prefix, section_digest};
use crate::migrate::{
    Apply, DynData, DynSection, DynWorld, MigrationCtx, Migrations, conversion_problem,
};
use crate::pce::{self, Decoder, write_uleb};
use crate::registry::Registry;
use crate::sections::Codec;
use crate::snapshot::{SectionData, Snapshot, SnapshotContext, SnapshotHeader, snapshot};
use crate::version::{EngineVersion, FormatEntry, FormatTable};

pub const SAVE_MAGIC: &[u8; 8] = b"P3DSAVE\0";
pub const SAVE_FORMAT: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SaveHeader {
    pub label: String,
    pub meta: BTreeMap<String, String>,
}

/// A save of the world: its header, its format table and its snapshot.
pub fn write_save(
    world: &World,
    reg: &Registry,
    label: &str,
    meta: BTreeMap<String, String>,
) -> Result<Vec<u8>, Problem> {
    let snap = snapshot(world, reg)?;
    let table = FormatTable::of(reg, world)?;
    let present: Vec<FormatEntry> = table
        .entries
        .into_iter()
        .filter(|e| snap.section(&e.section).is_some())
        .collect();
    let mut out = Vec::new();
    out.extend_from_slice(SAVE_MAGIC);
    out.extend_from_slice(&SAVE_FORMAT.to_le_bytes());
    let header = SaveHeader {
        label: label.to_owned(),
        meta,
    };
    for part in [
        pce::to_bytes(&header),
        pce::to_bytes(&FormatTable { entries: present }),
    ] {
        let part = part.map_err(|e| error::encode("save", None, &e.reason()))?;
        write_uleb(&mut out, part.len() as u64);
        out.extend_from_slice(&part);
    }
    out.extend_from_slice(&snap.to_bytes());
    Ok(out)
}

/// What loading changed (6.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// Section, from, to.
    pub migrated: Vec<(SectionKey, u32, u32)>,
    pub renamed: Vec<(String, String)>,
    /// Retired types.
    pub dropped: Vec<SectionKey>,
    /// Identity differed: rebuild them after the restore ([`rebuild_caches`]).
    pub caches_rebuilt: Vec<SectionKey>,
    pub engine_differs: bool,
    pub bundle_differs: bool,
}

/// Runs a project component's TypeScript steps (8.3); `pocket-runtime` implements it with the
/// script host.
pub trait ProjectMigrator {
    fn migrate(&mut self, component: &str, from: u32, value: Value) -> Result<Value, Problem>;
}

/// No project steps: every project component must already be at its current version.
pub struct NoProjectMigrator;

impl ProjectMigrator for NoProjectMigrator {
    fn migrate(&mut self, component: &str, from: u32, _: Value) -> Result<Value, Problem> {
        Err(error::missing_step(component, from))
    }
}

fn to_dyn(key: &SectionKey, f: &ResolvedFormat, bytes: &[u8]) -> Result<DynData, Problem> {
    let name = key.to_string();
    if key.kind != SectionKind::Component {
        let v = to_json(f, bytes, true).map_err(|e| conversion_problem(&name, None, &e))?;
        return Ok(DynData::Value(v));
    }
    let mut d = Decoder::new(bytes, false);
    let fail = |e: pce::PceError| {
        error::noncanonical(e_offset(&e), &format!("in section {key}: {}", e.reason()))
    };
    let n = d.uleb().map_err(fail)?;
    let mut rows = Vec::new();
    for _ in 0..n {
        let id: EntityId = d.value().map_err(fail)?;
        let v = crate::format::json::read(f, &mut d, true)
            .map_err(|e| conversion_problem(&name, Some(id), &e))?;
        rows.push((id, v));
    }
    d.finish().map_err(fail)?;
    Ok(DynData::Rows(rows))
}

fn e_offset(e: &pce::PceError) -> usize {
    match e {
        pce::PceError::Truncated { offset } | pce::PceError::Noncanonical { offset, .. } => *offset,
        pce::PceError::Encode(_) => 0,
    }
}

fn from_dyn(key: &SectionKey, f: &ResolvedFormat, data: &DynData) -> Result<Vec<u8>, Problem> {
    let name = key.to_string();
    let mut out = Vec::new();
    match data {
        DynData::Value(v) => {
            from_json(f, v, &mut out).map_err(|e| conversion_problem(&name, None, &e))?
        }
        DynData::Rows(rows) => {
            write_uleb(&mut out, rows.len() as u64);
            for (id, v) in rows {
                out.extend_from_slice(&id.get().to_le_bytes());
                from_json(f, v, &mut out).map_err(|e| conversion_problem(&name, Some(*id), &e))?;
            }
        }
        DynData::Bytes(b) => out.extend_from_slice(b),
    }
    Ok(out)
}

/// Runs a value step over a section's rows or value.
fn run_value_step(
    key: &SectionKey,
    from: u32,
    tick: Tick,
    data: &mut DynData,
    step: &mut dyn FnMut(&MigrationCtx, Value) -> Result<Value, Problem>,
) -> Result<(), Problem> {
    match data {
        DynData::Value(v) => *v = step(&MigrationCtx { entity: None, tick }, std::mem::take(v))?,
        DynData::Rows(rows) => {
            for (id, v) in rows {
                *v = step(
                    &MigrationCtx {
                        entity: Some(*id),
                        tick,
                    },
                    std::mem::take(v),
                )?;
            }
        }
        DynData::Bytes(_) => return Err(error::missing_step(&key.to_string(), from)),
    }
    Ok(())
}

/// Reads a save into a snapshot in the current versions (6.2), before any world is touched:
/// renames, then per section keep, migrate, drop (retired) or list a cache to rebuild. `world` is
/// the world it will be restored into, whose component registry gives the project components'
/// current schemas (open choice 6). Restore the snapshot, then call [`rebuild_caches`].
pub fn load_save(
    bytes: &[u8],
    reg: &Registry,
    migrations: &Migrations,
    project: &mut dyn ProjectMigrator,
    world: &World,
) -> Result<(Snapshot, LoadReport), Problem> {
    if bytes.len() < 12 || &bytes[..8] != SAVE_MAGIC {
        return Err(error::format(
            "the magic P3DSAVE",
            &String::from_utf8_lossy(&bytes[..bytes.len().min(8)]),
        ));
    }
    let format = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    if format != SAVE_FORMAT {
        return Err(error::version_format("save", format, SAVE_FORMAT));
    }
    let mut d = Decoder::new(bytes, false);
    let (header, table, rest) = (|| {
        d.take(12)?;
        let n = usize::try_from(d.uleb()?).unwrap_or(usize::MAX);
        let header: SaveHeader = pce::from_bytes(d.take(n)?, false)?;
        let n = usize::try_from(d.uleb()?).unwrap_or(usize::MAX);
        let table: FormatTable = pce::from_bytes(d.take(n)?, false)?;
        Ok::<_, pce::PceError>((header, table, d.pos()))
    })()
    .map_err(|e| e.problem(0))?;
    let _ = header;
    let saved = Snapshot::from_bytes(&bytes[rest..])?;
    migrate_snapshot(&saved, &table, reg, migrations, project, world)
}

/// The per-section steps of [`load_save`] on a snapshot whose sections `table` describes (a save's
/// format table, or the `Formats` records of a replay that `Rerun` restores, versions.md 7.2): the
/// snapshot in the current versions of `reg` and `world`'s project components, with the bundle
/// `world` runs.
pub fn migrate_snapshot(
    saved: &Snapshot,
    table: &FormatTable,
    reg: &Registry,
    migrations: &Migrations,
    project: &mut dyn ProjectMigrator,
    world: &World,
) -> Result<(Snapshot, LoadReport), Problem> {
    let current = FormatTable::of(reg, world)?;
    let bundle = world
        .get_resource::<SnapshotContext>()
        .map_or(saved.header().bundle, |c| c.bundle);
    let target = Target {
        current: &current,
        engine: &|k| reg.entry(k).is_some(),
        bundle,
    };
    migrate_with(saved, table, &target, migrations, project)
}

/// What a snapshot is migrated to.
pub(crate) struct Target<'a> {
    /// Every registered section's format and the target world's project components'.
    pub current: &'a FormatTable,
    /// Whether a section is the engine's (its steps are Rust), not a project component's.
    pub engine: &'a dyn Fn(&SectionKey) -> bool,
    /// The bundle the snapshot's header names.
    pub bundle: ContentHash,
}

pub(crate) fn migrate_with(
    saved: &Snapshot,
    table: &FormatTable,
    target: &Target<'_>,
    migrations: &Migrations,
    project: &mut dyn ProjectMigrator,
) -> Result<(Snapshot, LoadReport), Problem> {
    let mut report = LoadReport {
        engine_differs: saved.header().engine.source != EngineVersion::current().source,
        ..LoadReport::default()
    };
    let tick = saved.header().tick;
    // Sections in the current names, and those that need steps as JSON.
    let mut keep: Vec<SectionData> = Vec::new();
    let mut dynw = DynWorld::default();
    let mut formats: BTreeMap<SectionKey, (u32, FormatEntry)> = BTreeMap::new();
    for s in saved.sections() {
        let mut key = s.key.clone();
        if let Some(to) = migrations.renamed(&key) {
            report.renamed.push((key.name.clone(), to.name.clone()));
            key = to;
        }
        let Some(cur) = target.current.get(&key).cloned() else {
            if migrations.removed(&key) {
                report.dropped.push(key);
                continue;
            }
            return Err(error::version_unknown_section(&key.to_string()));
        };
        if key.kind == SectionKind::Cache {
            let recorded = table.get(&s.key).and_then(|e| e.identity.clone());
            if recorded == cur.identity && s.fingerprint == cur.fingerprint {
                keep.push(renamed_section(key, s));
            } else {
                report.caches_rebuilt.push(key);
            }
            continue;
        }
        if s.version == cur.version {
            if s.fingerprint != cur.fingerprint {
                return Err(error::fingerprint_mismatch(&key.to_string(), s.version));
            }
            keep.push(renamed_section(key, s));
            continue;
        }
        if s.version > cur.version {
            return Err(error::newer(&key.to_string(), s.version, cur.version));
        }
        let saved_format = table
            .get(&s.key)
            .and_then(|e| e.format.clone())
            .ok_or_else(|| error::version_unknown_section(&s.key.to_string()))?;
        let data = to_dyn(&key, &saved_format, &s.bytes)?;
        dynw.sections.insert(
            key.clone(),
            DynSection {
                version: s.version,
                data,
            },
        );
        formats.insert(key, (s.version, cur));
    }
    // Engine steps in list order: each runs when its section is at its `from` version.
    for step in &migrations.steps {
        let key = SectionKey::new(step.kind, step.section);
        if dynw.sections.get(&key).map(|s| s.version) != Some(step.from) {
            continue;
        }
        match &step.apply {
            Apply::Value(f) => {
                let name = key.to_string();
                let sec = dynw
                    .sections
                    .get_mut(&key)
                    .ok_or_else(|| error::missing_step(&name, step.from))?;
                let mut run = |ctx: &MigrationCtx, v: Value| {
                    f(ctx, v).map_err(|e| {
                        error::migrate_failed(
                            &name,
                            ctx.entity.map(EntityId::get),
                            step.from,
                            &e.path,
                            &e.message,
                        )
                    })
                };
                run_value_step(&key, step.from, tick, &mut sec.data, &mut run)?;
                sec.version += 1;
            }
            Apply::World(f) => {
                f(&mut dynw).map_err(|e| {
                    error::migrate_failed(step.section, None, step.from, &e.path, &e.message)
                })?;
                if let Some(sec) = dynw.sections.get_mut(&key) {
                    sec.version = step.from + 1;
                }
            }
        }
    }
    // Project components through the script host, then every migrated section converted by its
    // current format.
    for (key, sec) in &mut dynw.sections {
        let Some((from0, cur)) = formats.get(key) else {
            continue;
        };
        let is_project = !(target.engine)(key);
        while is_project && sec.version < cur.version {
            let v = sec.version;
            let mut run = |_: &MigrationCtx, value: Value| project.migrate(&key.name, v, value);
            run_value_step(key, v, tick, &mut sec.data, &mut run)?;
            sec.version += 1;
        }
        if sec.version != cur.version {
            return Err(error::missing_step(&key.to_string(), sec.version));
        }
        let f = cur
            .format
            .as_ref()
            .ok_or_else(|| error::version_unknown_section(&key.to_string()))?;
        let bytes = from_dyn(key, f, &sec.data)?;
        let mut buf = Vec::new();
        digest_prefix(&mut buf, key.kind, &key.name, cur.version);
        buf.extend_from_slice(&bytes);
        report.migrated.push((key.clone(), *from0, cur.version));
        keep.push(SectionData {
            key: key.clone(),
            version: cur.version,
            fingerprint: cur.fingerprint,
            digest: section_digest(&buf),
            bytes: Arc::from(bytes),
        });
    }
    report.migrated.sort();
    keep.sort_by(|a, b| a.key.cmp(&b.key));
    let bundle = target.bundle;
    report.bundle_differs = bundle != saved.header().bundle;
    let header = SnapshotHeader {
        tick,
        writes: saved.header().writes,
        engine: EngineVersion::current().clone(),
        bundle,
    };
    Ok((Snapshot::from_parts(header, keep), report))
}

/// A kept section under its current name: the digest covers the name, so a renamed section gets
/// its digest again.
fn renamed_section(key: SectionKey, s: &SectionData) -> SectionData {
    if key == s.key {
        return s.clone();
    }
    let mut buf = Vec::new();
    digest_prefix(&mut buf, key.kind, &key.name, s.version);
    buf.extend_from_slice(&s.bytes);
    SectionData {
        key,
        digest: section_digest(&buf),
        ..s.clone()
    }
}

/// Rebuilds the caches a load listed (6.3), after the snapshot is restored.
pub fn rebuild_caches(
    world: &mut World,
    reg: &Registry,
    caches: &[SectionKey],
) -> Result<(), Problem> {
    for rebuild in cache_rebuilds(reg, caches) {
        rebuild(world)?;
    }
    Ok(())
}

/// A cache's `PersistedCache::rebuild`.
pub(crate) type CacheRebuild = fn(&mut World) -> Result<(), Problem>;

/// The rebuild functions of the caches named, in order.
pub(crate) fn cache_rebuilds(reg: &Registry, caches: &[SectionKey]) -> Vec<CacheRebuild> {
    caches
        .iter()
        .filter_map(|key| match reg.entry(key).map(|e| &e.codec) {
            Some(Codec::Cache { rebuild, .. }) => Some(*rebuild),
            _ => None,
        })
        .collect()
}
