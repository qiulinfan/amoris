//! The schema lock (docs/spec/versions.md 5): the last accepted schema of every type, committed,
//! and the check that compares a registry with it, so a version bump never rests on memory.

use pocket_contract::{Problem, detail};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error;
use crate::format::json::push_segment;
use crate::format::{Fingerprint, ResolvedFormat};
use crate::hash::SectionKey;
use crate::migrate::Migrations;
use crate::version::FormatTable;

/// The lock file's content: every section in section order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaLock {
    pub format: u32,
    pub sections: Vec<LockEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockEntry {
    pub section: SectionKey,
    pub version: u32,
    pub fingerprint: String,
    pub format: Option<ResolvedFormat>,
    pub identity: Option<String>,
}

impl SchemaLock {
    /// The lock of a format table (the registry's and the project's components).
    pub fn of(table: &FormatTable) -> SchemaLock {
        SchemaLock {
            format: 1,
            sections: table
                .entries
                .iter()
                .map(|e| LockEntry {
                    section: e.section.clone(),
                    version: e.version,
                    fingerprint: e.fingerprint.to_string(),
                    format: e.format.clone(),
                    identity: e.identity.clone(),
                })
                .collect(),
        }
    }

    /// The generated file's text: pretty JSON, one key order (no `preserve_order`), ending in a
    /// newline.
    pub fn to_text(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).unwrap_or_default();
        s.push('\n');
        s
    }

    pub fn from_text(text: &str) -> Result<SchemaLock, Problem> {
        serde_json::from_str(text).map_err(|e| {
            Problem::new(
                "version.lock_invalid",
                format!("The schema lock cannot be read: {e}."),
                detail([("reason", json!(e.to_string()))]),
            )
        })
    }
}

/// The field paths added, removed or retyped from `old` to `new`, as JSON Pointers.
pub fn changes(old: &ResolvedFormat, new: &ResolvedFormat) -> Vec<String> {
    let mut out = Vec::new();
    walk(old, new, &mut String::new(), &mut out);
    out
}

fn walk(old: &ResolvedFormat, new: &ResolvedFormat, path: &mut String, out: &mut Vec<String>) {
    use ResolvedFormat as F;
    match (old, new) {
        (F::Struct(a), F::Struct(b)) => {
            for (name, fa) in a {
                let len = path.len();
                push_segment(path, name);
                match b.iter().find(|(n, _)| n == name) {
                    None => out.push(format!("{path} removed")),
                    Some((_, fb)) => walk(fa, fb, path, out),
                }
                path.truncate(len);
            }
            for (name, _) in b.iter().filter(|(n, _)| !a.iter().any(|(m, _)| m == n)) {
                let len = path.len();
                push_segment(path, name);
                out.push(format!("{path} added"));
                path.truncate(len);
            }
            let names = |v: &[(String, F)]| v.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
            let common_a: Vec<String> = names(a)
                .into_iter()
                .filter(|n| b.iter().any(|(m, _)| m == n))
                .collect();
            let common_b: Vec<String> = names(b)
                .into_iter()
                .filter(|n| a.iter().any(|(m, _)| m == n))
                .collect();
            if common_a != common_b {
                out.push(format!(
                    "{} reordered",
                    if path.is_empty() { "/" } else { path }
                ));
            }
        }
        (F::NewTypeStruct(a), F::NewTypeStruct(b))
        | (F::Option(a), F::Option(b))
        | (F::Seq(a), F::Seq(b)) => {
            walk(a, b, path, out);
        }
        (a, b) if a == b => {}
        _ => out.push(format!(
            "{} retyped",
            if path.is_empty() { "/" } else { path }
        )),
    }
}

/// Checks a registry's sections against the lock (the table of 5): every problem, in section
/// order. `current` is the registry's format table (with a project's components for a project
/// lock).
pub fn check(current: &FormatTable, lock: &SchemaLock, migrations: &Migrations) -> Vec<Problem> {
    let mut out = Vec::new();
    let mut stale = Vec::new();
    for e in &current.entries {
        let name = e.section.to_string();
        let Some(l) = lock.sections.iter().find(|l| l.section == e.section) else {
            stale.push(name);
            continue;
        };
        let fp = e.fingerprint.to_string();
        if e.version == l.version {
            if fp != l.fingerprint {
                let changes = match (&l.format, &e.format) {
                    (Some(a), Some(b)) => changes(a, b),
                    _ => vec![format!("identity {:?} -> {:?}", l.identity, e.identity)],
                };
                out.push(Problem::new(
                    "version.unbumped",
                    format!(
                        "{name} changed shape ({}) but its version is still {}: set it to {} and add a \
                         migration from {}.",
                        changes.join(", "),
                        e.version,
                        e.version + 1,
                        e.version
                    ),
                    detail([
                        ("section", json!(name)),
                        ("version", json!(e.version)),
                        ("changes", json!(changes)),
                    ]),
                ));
            }
            continue;
        }
        if e.version < l.version {
            out.push(Problem::new(
                "version.downgrade",
                format!(
                    "{name} went down from version {} to {}.",
                    l.version, e.version
                ),
                detail([
                    ("section", json!(name)),
                    ("locked", json!(l.version)),
                    ("current", json!(e.version)),
                ]),
            ));
            continue;
        }
        let missing: Vec<u32> = (l.version..e.version)
            .filter(|v| migrations.step(&e.section, *v).is_none())
            .collect();
        if e.section.kind == crate::hash::SectionKind::Cache || missing.is_empty() {
            stale.push(name);
        } else {
            out.extend(
                missing
                    .into_iter()
                    .map(|from| error::missing_step(&name, from)),
            );
        }
    }
    for l in &lock.sections {
        let gone = current.entries.iter().all(|e| e.section != l.section);
        if gone && !migrations.removed(&l.section) && migrations.renamed(&l.section).is_none() {
            out.push(Problem::new(
                "migrate.missing_removal",
                format!(
                    "{} is in the lock but no longer registered, and not retired.",
                    l.section
                ),
                detail([("section", json!(l.section.to_string()))]),
            ));
        }
    }
    if !stale.is_empty() {
        out.push(Problem::new(
            "version.lock_stale",
            format!(
                "The schema lock needs regenerating for {}.",
                stale.join(", ")
            ),
            detail([("sections", json!(stale))]),
        ));
    }
    out
}

/// A fingerprint from its hex form, for callers comparing lock entries.
pub fn fingerprint_hex(f: &Fingerprint) -> String {
    f.to_string()
}
