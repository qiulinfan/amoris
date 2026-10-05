//! Schema identity (docs/spec/versions.md 3.5): a persisted type's `serde-reflection` format with
//! named containers inlined (`ResolvedFormat`), and its fingerprint, the first 16 bytes of a
//! BLAKE3 key derivation over the resolved format's PCE.

pub mod json;

use std::collections::BTreeMap;
use std::fmt;

use pocket_contract::Problem;
use pocket_sim::Persisted;
use pocket_sim::persisted::{record_samples, tracer_config};
use pocket_sim::registry::{ComponentSchema, FieldType};
use serde::{Deserialize, Serialize};
use serde_reflection::{ContainerFormat, Format, Named, Samples, Tracer, VariantFormat};

use crate::error;
use crate::pce;

/// A format with every named container inlined, keeping field and variant names and dropping
/// container names. `Back(n)` refers to the container `n` levels up the containers being expanded
/// (0: the innermost): the one form of recursion, which `PlainData` needs (open choice 6).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ResolvedFormat {
    Unit,
    Bool,
    I8,
    I16,
    I32,
    I64,
    I128,
    U8,
    U16,
    U32,
    U64,
    U128,
    F32,
    F64,
    Char,
    Str,
    Bytes,
    Option(Box<ResolvedFormat>),
    Seq(Box<ResolvedFormat>),
    Map {
        key: Box<ResolvedFormat>,
        value: Box<ResolvedFormat>,
    },
    Tuple(Vec<ResolvedFormat>),
    TupleArray {
        content: Box<ResolvedFormat>,
        size: u64,
    },
    UnitStruct,
    NewTypeStruct(Box<ResolvedFormat>),
    TupleStruct(Vec<ResolvedFormat>),
    Struct(Vec<(String, ResolvedFormat)>),
    /// Variants in ascending index.
    Enum(Vec<(u32, String, ResolvedVariant)>),
    Back(u32),
}

/// One variant of a resolved enum.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ResolvedVariant {
    Unit,
    NewType(Box<ResolvedFormat>),
    Tuple(Vec<ResolvedFormat>),
    Struct(Vec<(String, ResolvedFormat)>),
}

/// A schema fingerprint.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Fingerprint(pub [u8; 16]);

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({})", error::hex(&self.0))
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&error::hex(&self.0))
    }
}

const FINGERPRINT_CONTEXT: &str = "Amoris 2026-10-03 schema fingerprint v1";

impl Fingerprint {
    /// The fingerprint of a resolved format.
    pub fn of(format: &ResolvedFormat) -> Fingerprint {
        // A resolved format holds no floats and no unbounded lengths; encoding cannot fail.
        let bytes = pce::to_bytes(format).unwrap_or_default();
        Fingerprint::of_bytes(&bytes)
    }

    /// The first 16 bytes of the key derivation over `bytes`.
    pub fn of_bytes(bytes: &[u8]) -> Fingerprint {
        let full = blake3::derive_key(FINGERPRINT_CONTEXT, bytes);
        let mut out = [0u8; 16];
        out.copy_from_slice(&full[..16]);
        Fingerprint(out)
    }

    /// A cache's fingerprint: the first 16 bytes of the BLAKE3 hash of its identity (3.6).
    pub fn of_identity(identity: &str) -> Fingerprint {
        let full = blake3::hash(identity.as_bytes());
        let mut out = [0u8; 16];
        out.copy_from_slice(&full.as_bytes()[..16]);
        Fingerprint(out)
    }
}

/// Traces `T` in a session of its own (persistence.md 6.2) and resolves its format.
pub fn trace<T: Persisted>() -> Result<ResolvedFormat, Problem> {
    let fail = |e: serde_reflection::Error| error::bad_type(T::NAME, &e.to_string());
    let mut t = Tracer::new(tracer_config());
    let mut s = Samples::new();
    record_samples(&mut t, &mut s).map_err(fail)?;
    T::trace(&mut t, &s).map_err(fail)?;
    let (top, _) = t.trace_type::<T>(&s).map_err(fail)?;
    let registry = t.registry().map_err(fail)?;
    resolve(&top, &registry).map_err(|why| error::bad_type(T::NAME, &why))
}

/// Inlines every named container of `format`.
pub fn resolve(
    format: &Format,
    registry: &BTreeMap<String, ContainerFormat>,
) -> Result<ResolvedFormat, String> {
    Resolver {
        registry,
        stack: Vec::new(),
    }
    .format(format)
}

struct Resolver<'a> {
    registry: &'a BTreeMap<String, ContainerFormat>,
    stack: Vec<&'a str>,
}

impl<'a> Resolver<'a> {
    fn format(&mut self, f: &'a Format) -> Result<ResolvedFormat, String> {
        use ResolvedFormat as R;
        let boxed = |r: Result<R, String>| r.map(Box::new);
        Ok(match f {
            Format::Variable(_) => return Err("a format left unknown by tracing".into()),
            Format::TypeName(name) => return self.container(name),
            Format::Unit => R::Unit,
            Format::Bool => R::Bool,
            Format::I8 => R::I8,
            Format::I16 => R::I16,
            Format::I32 => R::I32,
            Format::I64 => R::I64,
            Format::I128 => R::I128,
            Format::U8 => R::U8,
            Format::U16 => R::U16,
            Format::U32 => R::U32,
            Format::U64 => R::U64,
            Format::U128 => R::U128,
            Format::F32 => R::F32,
            Format::F64 => R::F64,
            Format::Char => R::Char,
            Format::Str => R::Str,
            Format::Bytes => R::Bytes,
            Format::Option(x) => R::Option(boxed(self.format(x))?),
            Format::Seq(x) => R::Seq(boxed(self.format(x))?),
            Format::Map { key, value } => R::Map {
                key: boxed(self.format(key))?,
                value: boxed(self.format(value))?,
            },
            Format::Tuple(xs) => R::Tuple(self.list(xs)?),
            Format::TupleArray { content, size } => R::TupleArray {
                content: boxed(self.format(content))?,
                size: *size as u64,
            },
        })
    }

    fn list(&mut self, xs: &'a [Format]) -> Result<Vec<ResolvedFormat>, String> {
        xs.iter().map(|x| self.format(x)).collect()
    }

    fn named(&mut self, xs: &'a [Named<Format>]) -> Result<Vec<(String, ResolvedFormat)>, String> {
        xs.iter()
            .map(|n| Ok((n.name.clone(), self.format(&n.value)?)))
            .collect()
    }

    fn container(&mut self, name: &'a str) -> Result<ResolvedFormat, String> {
        if let Some(depth) = self.stack.iter().rev().position(|n| *n == name) {
            return Ok(ResolvedFormat::Back(
                u32::try_from(depth).unwrap_or(u32::MAX),
            ));
        }
        let c = self
            .registry
            .get(name)
            .ok_or_else(|| format!("the container {name} was not traced"))?;
        self.stack.push(name);
        let out = match c {
            ContainerFormat::UnitStruct => Ok(ResolvedFormat::UnitStruct),
            ContainerFormat::NewTypeStruct(x) => self
                .format(x)
                .map(|r| ResolvedFormat::NewTypeStruct(Box::new(r))),
            ContainerFormat::TupleStruct(xs) => self.list(xs).map(ResolvedFormat::TupleStruct),
            ContainerFormat::Struct(xs) => self.named(xs).map(ResolvedFormat::Struct),
            ContainerFormat::Enum(variants) => {
                let mut out = Vec::with_capacity(variants.len());
                for (i, v) in variants {
                    let r = match &v.value {
                        VariantFormat::Variable(_) => {
                            return Err(format!("variant {} of {name} left unknown", v.name));
                        }
                        VariantFormat::Unit => ResolvedVariant::Unit,
                        VariantFormat::NewType(x) => {
                            ResolvedVariant::NewType(Box::new(self.format(x)?))
                        }
                        VariantFormat::Tuple(xs) => ResolvedVariant::Tuple(self.list(xs)?),
                        VariantFormat::Struct(xs) => ResolvedVariant::Struct(self.named(xs)?),
                    };
                    out.push((*i, v.name.clone(), r));
                }
                Ok(ResolvedFormat::Enum(out))
            }
        };
        self.stack.pop();
        out
    }
}

fn vector(names: &[&str]) -> ResolvedFormat {
    ResolvedFormat::Struct(
        names
            .iter()
            .map(|n| ((*n).to_owned(), ResolvedFormat::F64))
            .collect(),
    )
}

/// The resolved format of a field type: the PCE of its Rust counterpart (script-host.md 6; open
/// choice 7): `tick` a `Tick` (a newtype of `u64`), `entity` an `Option<EntityId>`, an enum its
/// unit variants, vectors and quaternions structs of `f64` named `x`, `y`, `z`, `w`.
pub fn field_format(ty: &FieldType) -> ResolvedFormat {
    use ResolvedFormat as R;
    match ty {
        FieldType::F64 => R::F64,
        FieldType::I32 => R::I32,
        FieldType::U32 => R::U32,
        FieldType::Tick => R::NewTypeStruct(Box::new(R::U64)),
        FieldType::Bool => R::Bool,
        FieldType::Entity => R::Option(Box::new(R::NewTypeStruct(Box::new(R::U64)))),
        FieldType::Str => R::Str,
        FieldType::Enum(variants) => R::Enum(
            variants
                .iter()
                .zip(0u32..)
                .map(|(n, i)| (i, n.to_string(), ResolvedVariant::Unit))
                .collect(),
        ),
        FieldType::Vec2 => vector(&["x", "y"]),
        FieldType::Vec3 => vector(&["x", "y", "z"]),
        FieldType::Vec4 | FieldType::Quat => vector(&["x", "y", "z", "w"]),
    }
}

/// A project component's format: a struct of its declared fields in declared order (6.2).
pub fn schema_format(schema: &ComponentSchema) -> ResolvedFormat {
    ResolvedFormat::Struct(
        schema
            .fields
            .iter()
            .map(|f| (f.name.to_string(), field_format(&f.ty)))
            .collect(),
    )
}

/// The format of the entities section: the allocator, then the live ids ascending.
pub fn entities_format() -> ResolvedFormat {
    use ResolvedFormat as R;
    R::Tuple(vec![
        R::Struct(vec![("next".into(), R::U64)]),
        R::Seq(Box::new(R::NewTypeStruct(Box::new(R::U64)))),
    ])
}

/// The format of a component section: the rows, each an id and a value.
pub fn rows_format(value: &ResolvedFormat) -> ResolvedFormat {
    use ResolvedFormat as R;
    R::Seq(Box::new(R::Tuple(vec![
        R::NewTypeStruct(Box::new(R::U64)),
        value.clone(),
    ])))
}
