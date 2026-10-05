//! Which component diverged (docs/spec/replay.md 3.2): two snapshots compared by section digests,
//! and the sections that differ decoded with their resolved formats, component rows merged by
//! `EntityId`, values walked leaf by leaf and compared by their bytes, so `-0.0` against `+0.0` is
//! found where JSON equality would miss it. Each difference is a JSON Pointer path with both values.

use pocket_contract::Problem;
use pocket_sim::EntityId;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::hex;
use crate::format::json::{JsonError, back, is_container, push_segment, read_at};
use crate::format::{ResolvedFormat as F, ResolvedVariant, entities_format};
use crate::hash::{SectionKey, SectionKind};
use crate::pce::Decoder;
use crate::registry::Registry;
use crate::snapshot::Snapshot;
use crate::version::FormatTable;

/// Which run a value or a refusal comes from; the expected side is the reference.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Side {
    Expected,
    Actual,
}

/// One differing leaf (or whole value).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct FieldDiff {
    pub section: SectionKey,
    /// `None` for resources and the entities section.
    pub entity: Option<EntityId>,
    /// RFC 6901 JSON Pointer into the value; "" for all of it.
    pub path: String,
    /// `Null` when the row or field is absent.
    pub expected: Value,
    pub actual: Value,
    /// Hex bit patterns when the JSON renders are equal.
    pub bits: Option<(String, String)>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum SectionChange {
    OnlyIn { side: Side, key: SectionKey },
    Differs { key: SectionKey },
}

#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct SnapshotDiff {
    pub sections: Vec<SectionChange>,
    pub fields: Vec<FieldDiff>,
    pub truncated: bool,
}

impl SnapshotDiff {
    /// The keys of every differing section, in section order.
    pub fn keys(&self) -> Vec<SectionKey> {
        self.sections
            .iter()
            .map(|c| match c {
                SectionChange::OnlyIn { key, .. } | SectionChange::Differs { key } => key.clone(),
            })
            .collect()
    }
}

/// The default cap on `fields` (3.2).
pub const DEFAULT_LIMIT: usize = 50;

/// Compares two snapshots with the formats the registry knows (project components appear in
/// `sections` only; [`diff_with`] takes a table that has their formats).
pub fn diff(
    a: &Snapshot,
    b: &Snapshot,
    reg: &Registry,
    limit: usize,
) -> Result<SnapshotDiff, Problem> {
    let table = FormatTable {
        entries: reg
            .entries()
            .iter()
            .map(|e| crate::version::FormatEntry {
                section: e.key.clone(),
                version: e.version,
                fingerprint: e.fingerprint,
                format: e.format.clone(),
                identity: e.identity.clone(),
            })
            .collect(),
    };
    diff_with(a, b, &table, limit)
}

/// Compares two snapshots with the formats of `table`.
pub fn diff_with(
    a: &Snapshot,
    b: &Snapshot,
    table: &FormatTable,
    limit: usize,
) -> Result<SnapshotDiff, Problem> {
    let mut out = SnapshotDiff::default();
    let (sa, sb) = (a.sections(), b.sections());
    let (mut i, mut j) = (0, 0);
    while i < sa.len() || j < sb.len() {
        let order = match (sa.get(i), sb.get(j)) {
            (Some(x), Some(y)) => x.key.cmp(&y.key),
            (Some(_), None) => std::cmp::Ordering::Less,
            _ => std::cmp::Ordering::Greater,
        };
        match order {
            std::cmp::Ordering::Less => {
                out.sections.push(SectionChange::OnlyIn {
                    side: Side::Expected,
                    key: sa[i].key.clone(),
                });
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                out.sections.push(SectionChange::OnlyIn {
                    side: Side::Actual,
                    key: sb[j].key.clone(),
                });
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                let (x, y) = (&sa[i], &sb[j]);
                if x.digest != y.digest {
                    out.sections
                        .push(SectionChange::Differs { key: x.key.clone() });
                    if x.version == y.version {
                        let format = if x.key.kind == SectionKind::Entities {
                            Some(entities_format())
                        } else {
                            table
                                .get(&x.key)
                                .filter(|e| e.version == x.version)
                                .and_then(|e| e.format.clone())
                        };
                        if let Some(f) = format {
                            let mut sink = Sink {
                                out: &mut out,
                                limit,
                                key: &x.key,
                                entity: None,
                            };
                            sink.section(&f, &x.bytes, &y.bytes)
                                .map_err(|e| json_problem(&x.key, &e))?;
                        }
                    }
                }
                i += 1;
                j += 1;
            }
        }
    }
    Ok(out)
}

fn json_problem(key: &SectionKey, e: &JsonError) -> Problem {
    match e {
        JsonError::Pce(p) => crate::snapshot::section_problem(key, p),
        other => crate::error::noncanonical(0, &format!("in section {key}: {other:?}")),
    }
}

struct Sink<'a> {
    out: &'a mut SnapshotDiff,
    limit: usize,
    key: &'a SectionKey,
    entity: Option<EntityId>,
}

/// One component row: its id and the bytes of its value.
fn rows<'b>(f: &F, bytes: &'b [u8]) -> Result<Vec<(EntityId, &'b [u8])>, JsonError> {
    let mut d = Decoder::new(bytes, false);
    let n = d.uleb()?;
    let mut out = Vec::new();
    for _ in 0..n {
        let id: EntityId = d.value()?;
        let start = d.pos();
        read_at(f, &mut d, false, &mut Vec::new(), &mut String::new())?;
        out.push((id, d.since(start)));
    }
    d.finish()?;
    Ok(out)
}

fn whole(f: &F, bytes: &[u8]) -> Result<Value, JsonError> {
    let mut d = Decoder::new(bytes, false);
    read_at(f, &mut d, false, &mut Vec::new(), &mut String::new())
}

/// Opens one more compound value on both sides: the decoders count it (`pce::MAX_DEPTH`), as
/// `read_at` does, so a walk refuses what decoding refuses before the stack runs out.
fn enter2(a: &mut Decoder<'_>, b: &mut Decoder<'_>) -> Result<(), JsonError> {
    a.enter()?;
    if let Err(e) = b.enter() {
        a.leave();
        return Err(e.into());
    }
    Ok(())
}

fn leave2(a: &mut Decoder<'_>, b: &mut Decoder<'_>) {
    a.leave();
    b.leave();
}

impl Sink<'_> {
    fn push(&mut self, path: &str, expected: Value, actual: Value, bits: Option<(&[u8], &[u8])>) {
        if self.out.fields.len() >= self.limit {
            self.out.truncated = true;
            return;
        }
        let bits = bits
            .filter(|_| expected == actual)
            .map(|(a, b)| (hex(a), hex(b)));
        self.out.fields.push(FieldDiff {
            section: self.key.clone(),
            entity: self.entity,
            path: path.to_owned(),
            expected,
            actual,
            bits,
        });
    }

    fn section(&mut self, f: &F, a: &[u8], b: &[u8]) -> Result<(), JsonError> {
        if self.key.kind != SectionKind::Component {
            return self.value(f, a, b);
        }
        let (ra, rb) = (rows(f, a)?, rows(f, b)?);
        let (mut i, mut j) = (0, 0);
        while i < ra.len() || j < rb.len() {
            let order = match (ra.get(i), rb.get(j)) {
                (Some(x), Some(y)) => x.0.cmp(&y.0),
                (Some(_), None) => std::cmp::Ordering::Less,
                _ => std::cmp::Ordering::Greater,
            };
            match order {
                std::cmp::Ordering::Less => {
                    self.entity = Some(ra[i].0);
                    self.push("", whole(f, ra[i].1)?, Value::Null, None);
                    i += 1;
                }
                std::cmp::Ordering::Greater => {
                    self.entity = Some(rb[j].0);
                    self.push("", Value::Null, whole(f, rb[j].1)?, None);
                    j += 1;
                }
                std::cmp::Ordering::Equal => {
                    if ra[i].1 != rb[j].1 {
                        self.entity = Some(ra[i].0);
                        self.value(f, ra[i].1, rb[j].1)?;
                    }
                    i += 1;
                    j += 1;
                }
            }
        }
        self.entity = None;
        Ok(())
    }

    fn value(&mut self, f: &F, a: &[u8], b: &[u8]) -> Result<(), JsonError> {
        let (mut da, mut db) = (Decoder::new(a, false), Decoder::new(b, false));
        self.walk(f, &mut da, &mut db, &mut Vec::new(), &mut String::new())
    }

    /// Reads one value of `f` on both sides as a whole and reports it if its bytes differ.
    fn whole_at<'f>(
        &mut self,
        f: &'f F,
        (a, sa): (&mut Decoder<'_>, usize),
        (b, sb): (&mut Decoder<'_>, usize),
        stack: &mut Vec<&'f F>,
        path: &mut String,
    ) -> Result<(), JsonError> {
        a.set_pos(sa);
        b.set_pos(sb);
        // `walk` pushed `f` if it is a container, and `read_at` pushes it again.
        let pushed = stack.last().is_some_and(|t| std::ptr::eq(*t, f));
        if pushed {
            stack.pop();
        }
        let va = read_at(f, a, false, stack, path);
        let vb = read_at(f, b, false, stack, path);
        if pushed {
            stack.push(f);
        }
        let (va, vb) = (va?, vb?);
        let (ba, bb) = (a.since(sa), b.since(sb));
        if ba != bb {
            self.push(path, va, vb, Some((ba, bb)));
        }
        Ok(())
    }

    fn walk<'f>(
        &mut self,
        f: &'f F,
        a: &mut Decoder<'_>,
        b: &mut Decoder<'_>,
        stack: &mut Vec<&'f F>,
        path: &mut String,
    ) -> Result<(), JsonError> {
        if let F::Back(n) = f {
            let target = back(stack, *n)?;
            return self.walk(target, a, b, stack, path);
        }
        let container = is_container(f);
        if container {
            stack.push(f);
        }
        let r = self.walk_inner(f, a, b, stack, path);
        if container {
            stack.pop();
        }
        r
    }

    fn walk_list<'f>(
        &mut self,
        fs: impl Iterator<Item = (String, &'f F)>,
        a: &mut Decoder<'_>,
        b: &mut Decoder<'_>,
        stack: &mut Vec<&'f F>,
        path: &mut String,
    ) -> Result<(), JsonError> {
        for (seg, f) in fs {
            let len = path.len();
            push_segment(path, &seg);
            self.walk(f, a, b, stack, path)?;
            path.truncate(len);
        }
        Ok(())
    }

    fn walk_inner<'f>(
        &mut self,
        f: &'f F,
        a: &mut Decoder<'_>,
        b: &mut Decoder<'_>,
        stack: &mut Vec<&'f F>,
        path: &mut String,
    ) -> Result<(), JsonError> {
        let (sa, sb) = (a.pos(), b.pos());
        match f {
            F::Option(x) => {
                let (ta, tb) = (a.value::<u8>()?, b.value::<u8>()?);
                if ta != tb {
                    return self.whole_at(f, (a, sa), (b, sb), stack, path);
                }
                if ta == 1 {
                    enter2(a, b)?;
                    let r = self.walk(x, a, b, stack, path);
                    leave2(a, b);
                    r?;
                }
            }
            F::Seq(x) => {
                let (na, nb) = (a.uleb()?, b.uleb()?);
                if na != nb {
                    return self.whole_at(f, (a, sa), (b, sb), stack, path);
                }
                enter2(a, b)?;
                let r = self.walk_list((0..na).map(|i| (i.to_string(), &**x)), a, b, stack, path);
                leave2(a, b);
                r?;
            }
            F::Map { .. } => {
                // Maps are compared whole: their entries pair by position, not by key.
                let va = read_at(f, a, false, stack, path)?;
                let vb = read_at(f, b, false, stack, path)?;
                let (ba, bb) = (a.since(sa), b.since(sb));
                if ba != bb {
                    self.push(path, va, vb, Some((ba, bb)));
                }
            }
            F::Tuple(xs) | F::TupleStruct(xs) => {
                enter2(a, b)?;
                let r = self.walk_list(
                    xs.iter().enumerate().map(|(i, x)| (i.to_string(), x)),
                    a,
                    b,
                    stack,
                    path,
                );
                leave2(a, b);
                r?;
            }
            F::TupleArray { content, size } => {
                enter2(a, b)?;
                let r = self.walk_list(
                    (0..*size).map(|i| (i.to_string(), &**content)),
                    a,
                    b,
                    stack,
                    path,
                );
                leave2(a, b);
                r?;
            }
            F::NewTypeStruct(x) => {
                enter2(a, b)?;
                let r = self.walk(x, a, b, stack, path);
                leave2(a, b);
                r?;
            }
            F::Struct(fs) => {
                enter2(a, b)?;
                let r = self.walk_list(fs.iter().map(|(n, x)| (n.clone(), x)), a, b, stack, path);
                leave2(a, b);
                r?;
            }
            F::Enum(variants) => {
                let (ia, ib) = (a.uleb()?, b.uleb()?);
                if ia != ib {
                    return self.whole_at(f, (a, sa), (b, sb), stack, path);
                }
                let Some((_, name, v)) = variants.iter().find(|(i, _, _)| u64::from(*i) == ia)
                else {
                    return Err(crate::pce::PceError::noncanonical(sa, "a variant index").into());
                };
                let len = path.len();
                push_segment(path, name);
                if !matches!(v, ResolvedVariant::Unit) {
                    enter2(a, b)?;
                    let r = match v {
                        ResolvedVariant::Unit => Ok(()),
                        ResolvedVariant::NewType(x) => self.walk(x, a, b, stack, path),
                        ResolvedVariant::Tuple(xs) => self.walk_list(
                            xs.iter().enumerate().map(|(i, x)| (i.to_string(), x)),
                            a,
                            b,
                            stack,
                            path,
                        ),
                        ResolvedVariant::Struct(fs) => self.walk_list(
                            fs.iter().map(|(n, x)| (n.clone(), x)),
                            a,
                            b,
                            stack,
                            path,
                        ),
                    };
                    leave2(a, b);
                    r?;
                }
                path.truncate(len);
            }
            // Leaves: compared by their bytes.
            _ => {
                let va = read_at(f, a, false, stack, path)?;
                let vb = read_at(f, b, false, stack, path)?;
                let (ba, bb) = (a.since(sa), b.since(sb));
                if ba != bb {
                    self.push(path, va, vb, Some((ba, bb)));
                }
            }
        }
        Ok(())
    }
}
