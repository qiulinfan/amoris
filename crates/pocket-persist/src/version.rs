//! What identifies a run's code and data (docs/spec/versions.md 3, 4 and 7.1): the engine version,
//! bundle and source hashes, format tables, and the comparison of a replay's versions with the
//! engine replaying it.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use pocket_sim::ContentHash;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::format::{Fingerprint, ResolvedFormat};
use crate::hash::SectionKey;
use crate::pce::write_uleb;
use crate::registry::Registry;

/// The context of an engine's source hash (3.1).
pub const SOURCE_CONTEXT: &str = "Pocket3D 2026-10-03 engine source v1";
/// The context of a bundle hash (3.2).
pub const BUNDLE_CONTEXT: &str = "Pocket3D 2026-10-03 script bundle v1";

/// The engine a snapshot, replay or save was made by (3.1).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct EngineVersion {
    pub semver: String,
    pub commit: String,
    pub source: ContentHash,
    pub target: String,
    pub profile: String,
    pub contract: String,
    pub c_compiler: String,
}

static CURRENT: OnceLock<EngineVersion> = OnceLock::new();

impl EngineVersion {
    /// The running engine's version: the one [`EngineVersion::install`] set, else
    /// [`EngineVersion::unbuilt`].
    pub fn current() -> &'static EngineVersion {
        CURRENT.get_or_init(EngineVersion::unbuilt)
    }

    /// Sets the running engine's version, once, before anything records (the top crate, whose
    /// build script computes `source`; architecture.md 4.13). Refused with the version already in
    /// place after the first call or after `current` has been read.
    pub fn install(v: EngineVersion) -> Result<(), Box<EngineVersion>> {
        CURRENT.set(v).map_err(Box::new)
    }

    /// The version of a build whose top crate computed no source hash (tests, and every crate
    /// until `pocket-app` exists): `source` is the derivation over no files, `commit` "unknown"
    /// (open choice 6).
    pub fn unbuilt() -> EngineVersion {
        EngineVersion {
            semver: env!("CARGO_PKG_VERSION").to_owned(),
            commit: "unknown".to_owned(),
            source: source_hash(&[]),
            target: format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS),
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .to_owned(),
            contract: "unsynced".to_owned(),
            c_compiler: "unknown".to_owned(),
        }
    }
}

fn path_hash(
    context: &str,
    files: &[(String, Vec<u8>)],
    per: impl Fn(&[u8], &mut Vec<u8>),
) -> ContentHash {
    let mut sorted: Vec<&(String, Vec<u8>)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut h = ContentHash::hasher(context);
    let mut buf = Vec::new();
    for (path, content) in sorted {
        buf.clear();
        write_uleb(&mut buf, path.len() as u64);
        buf.extend_from_slice(path.as_bytes());
        per(content, &mut buf);
        h.update(&buf);
    }
    h.finish()
}

/// An engine source hash over `(path, content)` pairs (3.1): per file in path order,
/// `ULEB128(len path) || path || BLAKE3(content)`.
pub fn source_hash(files: &[(String, Vec<u8>)]) -> ContentHash {
    path_hash(SOURCE_CONTEXT, files, |c, buf| {
        buf.extend_from_slice(blake3::hash(c).as_bytes());
    })
}

/// A bundle hash over its TypeScript sources (3.2): per module in path order,
/// `ULEB128(len path) || path || ULEB128(len source) || source`.
pub fn bundle_hash(files: &[(String, Vec<u8>)]) -> ContentHash {
    path_hash(BUNDLE_CONTEXT, files, |c, buf| {
        write_uleb(buf, c.len() as u64);
        buf.extend_from_slice(c);
    })
}

/// The formats of a file's sections, in section order (4).
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct FormatTable {
    pub entries: Vec<FormatEntry>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct FormatEntry {
    pub section: SectionKey,
    pub version: u32,
    pub fingerprint: Fingerprint,
    /// `None` for caches.
    pub format: Option<ResolvedFormat>,
    /// Caches only.
    pub identity: Option<String>,
}

impl FormatTable {
    /// The table of every section a registry knows, and of the world's project components.
    pub fn of(
        reg: &Registry,
        world: &bevy_ecs::prelude::World,
    ) -> Result<FormatTable, pocket_contract::Problem> {
        let mut entries: Vec<FormatEntry> = reg
            .entries()
            .iter()
            .map(|e| FormatEntry {
                section: e.key.clone(),
                version: e.version,
                fingerprint: e.fingerprint,
                format: e.format.clone(),
                identity: e.identity.clone(),
            })
            .collect();
        for p in crate::project::projects(world)? {
            entries.push(FormatEntry {
                section: p.key.clone(),
                version: p.schema.version,
                fingerprint: p.fingerprint,
                format: Some(p.format.clone()),
                identity: None,
            });
        }
        entries.sort_by(|a, b| a.section.cmp(&b.section));
        Ok(FormatTable { entries })
    }

    pub fn get(&self, key: &SectionKey) -> Option<&FormatEntry> {
        self.entries
            .binary_search_by(|e| e.section.cmp(key))
            .ok()
            .map(|i| &self.entries[i])
    }
}

/// Whether a recorded identifier equals the current one.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Match {
    Same,
    Differs { recorded: String, current: String },
}

impl Match {
    pub fn of(recorded: impl ToString, current: impl ToString) -> Match {
        let (r, c) = (recorded.to_string(), current.to_string());
        if r == c {
            Match::Same
        } else {
            Match::Differs {
                recorded: r,
                current: c,
            }
        }
    }

    pub fn same(&self) -> bool {
        matches!(self, Match::Same)
    }
}

/// How a bundle a replay loads is found (7.1).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum BundleUse {
    /// Embedded in the replay, run as recorded.
    Same,
    /// Replaced by the caller's bundle.
    Substituted {
        with: ContentHash,
    },
    /// Not embedded; the stepper's source may find it by hash.
    Available,
    Unavailable,
}

/// A replay's versions against the engine replaying it (7.1).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct VersionComparison {
    pub format: Match,
    pub engine: Match,
    pub contract: Match,
    pub run_config: Match,
    pub bundles: Vec<(ContentHash, BundleUse)>,
    pub schemas: Vec<(SectionKey, Match)>,
    pub caches: Vec<(SectionKey, Match)>,
    pub data: Vec<(String, Match)>,
}

impl VersionComparison {
    /// What `Verify` would refuse, in words.
    pub fn verify_differences(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (what, m) in [
            ("format", &self.format),
            ("engine source", &self.engine),
            ("contract", &self.contract),
            ("run configuration", &self.run_config),
        ] {
            if !m.same() {
                out.push(what.to_owned());
            }
        }
        for (h, u) in &self.bundles {
            if !matches!(u, BundleUse::Same | BundleUse::Available) {
                out.push(format!("bundle {h}"));
            }
        }
        out.extend(self.schema_differences());
        out
    }

    /// Sections whose schema or cache identity differs (`Compare` refuses them).
    pub fn schema_differences(&self) -> Vec<String> {
        self.schemas
            .iter()
            .chain(&self.caches)
            .filter(|(_, m)| !m.same())
            .map(|(k, _)| k.to_string())
            .collect()
    }

    /// The comparison as a problem's detail.
    pub fn detail(&self) -> pocket_contract::Detail {
        match serde_json::to_value(self) {
            Ok(Value::Object(m)) => m,
            _ => pocket_contract::detail([("comparison", json!(null))]),
        }
    }
}

/// Free metadata of a save or replay.
pub type Labels = BTreeMap<String, String>;
