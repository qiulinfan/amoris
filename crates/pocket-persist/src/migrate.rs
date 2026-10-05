//! Migrations (docs/spec/versions.md 8): steps that take one section type from version `n` to
//! `n + 1` in JSON, append-only and with examples; retired types; the world as JSON for steps that
//! move data between sections.

use std::collections::BTreeMap;

use pocket_contract::Problem;
use pocket_sim::{EntityId, Tick};
use serde_json::Value;

use crate::error;
use crate::format::ResolvedFormat;
use crate::format::json::{JsonError, from_json};
use crate::hash::{SectionKey, SectionKind};

/// What a value step knows besides the value.
#[derive(Clone, Copy, Debug)]
pub struct MigrationCtx {
    pub entity: Option<EntityId>,
    pub tick: Tick,
}

/// A step's failure: where in the value, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MigrationError {
    pub path: String,
    pub message: String,
}

impl MigrationError {
    /// A field missing or of the wrong type at `path`.
    pub fn field(path: &str) -> MigrationError {
        MigrationError {
            path: path.to_owned(),
            message: "missing or of the wrong type".to_owned(),
        }
    }

    pub fn new(path: &str, message: &str) -> MigrationError {
        MigrationError {
            path: path.to_owned(),
            message: message.to_owned(),
        }
    }
}

/// One section of a world as JSON.
#[derive(Clone, Debug, PartialEq)]
pub enum DynData {
    /// Component rows by id.
    Rows(Vec<(EntityId, Value)>),
    /// A resource's value.
    Value(Value),
    /// A cache's bytes (opaque) or the entities section.
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct DynSection {
    pub version: u32,
    pub data: DynData,
}

/// Sections as JSON: component rows by `EntityId`, resources as one value, caches as bytes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DynWorld {
    pub sections: BTreeMap<SectionKey, DynSection>,
}

pub type ValueStep = fn(&MigrationCtx, Value) -> Result<Value, MigrationError>;

#[derive(Clone, Copy, Debug)]
pub enum Apply {
    /// Each component row or the resource value.
    Value(ValueStep),
    /// The whole world, for steps that split, merge or move data between sections.
    World(fn(&mut DynWorld) -> Result<(), MigrationError>),
}

#[derive(Clone, Copy, Debug)]
pub struct MigrationStep {
    pub kind: SectionKind,
    /// The name at version `from`.
    pub section: &'static str,
    /// Produces `from + 1`.
    pub from: u32,
    pub apply: Apply,
    /// `(old JSON, new JSON)`.
    pub examples: &'static [(&'static str, &'static str)],
}

/// A type that left the engine (8.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retired {
    Removed {
        kind: SectionKind,
        section: &'static str,
        last_version: u32,
    },
    Renamed {
        kind: SectionKind,
        from: &'static str,
        to: &'static str,
        at_version: u32,
    },
}

/// The engine's ordered steps and retirements.
#[derive(Clone, Debug, Default)]
pub struct Migrations {
    pub steps: Vec<MigrationStep>,
    pub retired: Vec<Retired>,
}

impl Migrations {
    pub fn new(steps: Vec<MigrationStep>, retired: Vec<Retired>) -> Migrations {
        Migrations { steps, retired }
    }

    /// The step from `from` for a section, if any.
    pub fn step(&self, key: &SectionKey, from: u32) -> Option<&MigrationStep> {
        self.steps
            .iter()
            .find(|s| s.kind == key.kind && s.section == key.name && s.from == from)
    }

    /// The name a retired section now has, if it was renamed.
    pub fn renamed(&self, key: &SectionKey) -> Option<SectionKey> {
        self.retired.iter().find_map(|r| match r {
            Retired::Renamed { kind, from, to, .. } if *kind == key.kind && *from == key.name => {
                Some(SectionKey::new(*kind, to))
            }
            _ => None,
        })
    }

    /// Whether a section was removed for good.
    pub fn removed(&self, key: &SectionKey) -> bool {
        self.retired.iter().any(|r| {
            matches!(r, Retired::Removed { kind, section, .. } if *kind == key.kind && *section == key.name)
        })
    }

    /// V2: every value step's examples give their expected values, compared after the conversion
    /// by `format` as PCE bytes when the step produces the current version (`current` gives each
    /// section's current version and format), as canonical JSON otherwise. A step without examples,
    /// or whose example fails, is `migrate.untested`.
    pub fn check_examples(
        &self,
        current: &dyn Fn(&SectionKey) -> Option<(u32, ResolvedFormat)>,
    ) -> Vec<Problem> {
        let mut out = Vec::new();
        for s in &self.steps {
            let key = SectionKey::new(s.kind, s.section);
            let untested = |e: Value, a: Value| error::untested(&key.to_string(), s.from, e, a);
            if s.examples.is_empty() {
                out.push(untested(Value::Null, Value::Null));
                continue;
            }
            let Apply::Value(f) = &s.apply else {
                continue;
            };
            let ctx = MigrationCtx {
                entity: None,
                tick: Tick(0),
            };
            for (old, new) in s.examples {
                let (Ok(old), Ok(new)) = (
                    serde_json::from_str::<Value>(old),
                    serde_json::from_str::<Value>(new),
                ) else {
                    out.push(untested(
                        Value::String((*new).into()),
                        Value::String((*old).into()),
                    ));
                    continue;
                };
                let got = match f(&ctx, old) {
                    Ok(v) => v,
                    Err(e) => {
                        out.push(untested(new, Value::String(e.message)));
                        continue;
                    }
                };
                let same = match current(&key) {
                    Some((v, format)) if v == s.from + 1 => {
                        let (mut a, mut b) = (Vec::new(), Vec::new());
                        from_json(&format, &new, &mut a).is_ok()
                            && from_json(&format, &got, &mut b).is_ok()
                            && a == b
                    }
                    _ => new == got,
                };
                if !same {
                    out.push(untested(new, got));
                }
            }
        }
        out
    }
}

/// A conversion failure as its problem (8.1): `migrate.nonfinite` for NaN, an infinity, or a `null`
/// where a float belongs (what `serde_json` makes of a non-finite double); `migrate.shape` for
/// the rest.
pub fn conversion_problem(section: &str, entity: Option<EntityId>, e: &JsonError) -> Problem {
    let entity = entity.map(EntityId::get);
    match e {
        JsonError::Nonfinite(path) => error::nonfinite(section, entity, path),
        JsonError::Shape {
            path,
            expected,
            found,
        } if found == "null" && expected.contains("number") => {
            error::nonfinite(section, entity, path)
        }
        JsonError::Shape {
            path,
            expected,
            found,
        } => error::migrate_shape(section, entity, path, expected, found),
        JsonError::Pce(p) => error::migrate_shape(
            section,
            entity,
            "",
            "bytes of the saved format",
            &p.reason(),
        ),
    }
}
