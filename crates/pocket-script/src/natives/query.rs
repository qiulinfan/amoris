//! Batch queries (docs/spec/script-host.md 5.2): the entities with every `with` component and none
//! of `without`, ascending by id, handed over as one `Float64Array` per numeric slot; after the
//! call, the cells that changed bit for bit are checked and written back.

use bevy_ecs::prelude::World;
use pocket_sim::EntityId;
use pocket_sim::entity::EntityIndex;
use pocket_sim::registry::FieldType;
use rquickjs::{Ctx, Object, Persistent, TypedArray, Value};
use serde_json::json;

use super::{NResult, Thrown, arg, with_call};
use crate::call::{CallState, CellWrite, Column, Comp, QueryRecord, resolve_comp};
use crate::convert::{check_number, freeze_new, is_plain_object};
use crate::error::{ErrorPhase, ScriptError};
use crate::host::{Shared, charge_host_bytes, shared};
use crate::js::{self, Own};

/// A parsed query.
pub struct Spec {
    pub with: Vec<String>,
    pub without: Vec<String>,
    /// `Component.field` names: only these columns.
    pub fields: Option<Vec<String>>,
}

fn bad_query(why: &str) -> ScriptError {
    ScriptError::new(
        "script.bad_definition",
        format!("A query {why}."),
        ErrorPhase::Run,
    )
}

fn strings<'js>(ctx: &Ctx<'js>, v: &Value<'js>, key: &str) -> Result<Vec<String>, ScriptError> {
    let Ok(len) = js::dense_array_len(ctx, v, 256) else {
        return Err(bad_query(&format!(
            "takes '{key}' as an array of at most 256 names"
        )));
    };
    let mut out = Vec::with_capacity(len);
    for i in 0..len {
        match js::own(ctx, v, &i.to_string()) {
            Own::Data(x) => match js::text(&x) {
                Some(Ok(s)) => out.push(s),
                _ => return Err(bad_query(&format!("takes '{key}' as an array of names"))),
            },
            _ => return Err(bad_query(&format!("takes '{key}' as an array of names"))),
        }
    }
    Ok(out)
}

/// A query spec from its JavaScript object: `{with, without?, fields?}`, any other key refused.
pub fn parse_spec<'js>(ctx: &Ctx<'js>, v: &Value<'js>) -> Result<Spec, ScriptError> {
    if !is_plain_object(ctx, v) {
        return Err(bad_query(
            "is an object such as { with: [\"Boat\", \"Transform\"] }",
        ));
    }
    let mut spec = Spec {
        with: Vec::new(),
        without: Vec::new(),
        fields: None,
    };
    for key in js::own_keys(ctx, v, true) {
        let value = match js::own(ctx, v, &key) {
            Own::Data(x) => x,
            _ => return Err(bad_query(&format!("gives '{key}' by a getter"))),
        };
        match key.as_str() {
            "with" => spec.with = strings(ctx, &value, "with")?,
            "without" => spec.without = strings(ctx, &value, "without")?,
            "fields" => spec.fields = Some(strings(ctx, &value, "fields")?),
            other => {
                return Err(ScriptError::new(
                    "script.unknown_key",
                    format!("A query has no key '{other}'."),
                    ErrorPhase::Run,
                )
                .suggest(pocket_contract::suggest::suggest_names(
                    other,
                    ["with", "without", "fields"],
                )));
            }
        }
    }
    if spec.with.is_empty() {
        return Err(bad_query("needs at least one component in 'with'"));
    }
    Ok(spec)
}

/// One numeric slot's column: its field's index, the slot and the rows' values.
type SlotColumn = (usize, u16, Vec<f64>);

/// A query's rows and, per component, the columns of each numeric slot asked for.
struct Built {
    ids: Vec<EntityId>,
    comps: Vec<(Comp, Vec<SlotColumn>)>,
}

fn build(world: &World, spec: &Spec, sh: &Shared) -> Result<Built, ScriptError> {
    let with: Vec<Comp> = spec
        .with
        .iter()
        .map(|n| resolve_comp(world, n))
        .collect::<Result<_, _>>()?;
    let without: Vec<Comp> = spec
        .without
        .iter()
        .map(|n| resolve_comp(world, n))
        .collect::<Result<_, _>>()?;
    let wanted = |comp: &Comp, field: &str| {
        spec.fields.as_ref().is_none_or(|fs| {
            fs.iter()
                .any(|f| *f == format!("{}.{field}", comp.schema.name))
        })
    };
    let mut comps: Vec<(Comp, Vec<SlotColumn>)> = with
        .iter()
        .map(|c| {
            let mut cols = Vec::new();
            for (fi, f) in c.schema.fields.iter().enumerate() {
                if f.ty == FieldType::Str || !wanted(c, &f.name) {
                    continue;
                }
                for k in 0..f.ty.slots() {
                    cols.push((fi, f.first_slot + k, Vec::new()));
                }
            }
            (c.clone(), cols)
        })
        .collect();
    let matches = |er: &bevy_ecs::world::EntityRef<'_>| {
        with.iter().all(|c| er.contains_id(c.id)) && !without.iter().any(|c| er.contains_id(c.id))
    };
    // The buffers are charged to the call before they are made: per row its id (the host's and
    // the script's) and per column slot its cell (the script's column and the host's copy the
    // write-back compares with), eight bytes each.
    let rows = world
        .resource::<EntityIndex>()
        .iter()
        .filter(|(_, e)| matches(&world.entity(*e)))
        .count();
    let slots: usize = comps.iter().map(|(_, cols)| cols.len()).sum();
    let bytes = (rows as u64).saturating_mul(16 + 16 * slots as u64);
    charge_host_bytes(sh, bytes, "This query")?;
    let mut ids = Vec::with_capacity(rows);
    for (_, cols) in &mut comps {
        for (_, _, col) in cols.iter_mut() {
            col.reserve_exact(rows);
        }
    }
    for (id, e) in world.resource::<EntityIndex>().iter() {
        let er = world.entity(e);
        if !matches(&er) {
            continue;
        }
        ids.push(id);
        for (comp, cols) in &mut comps {
            let v = comp.access.read(&er).unwrap_or_default();
            for (_, slot, col) in cols.iter_mut() {
                col.push(v.nums.get(usize::from(*slot)).copied().unwrap_or(0.0));
            }
        }
    }
    Ok(Built { ids, comps })
}

/// Hands a query's columns over, recording them for the write-back.
pub fn prepare<'js>(ctx: &Ctx<'js>, spec: &Spec) -> Result<Value<'js>, Thrown> {
    let sh = shared(ctx);
    let built = with_call(ctx, "ctx.query", |_call, world| build(world, spec, &sh))?;
    let len = built.ids.len();
    let ids: Vec<f64> = built.ids.iter().map(|i| i.to_f64()).collect();
    let cols = Object::new(ctx.clone())?;
    let mut record = QueryRecord {
        ids: built.ids,
        comps: Vec::new(),
    };
    for (comp, mut columns) in built.comps {
        let obj = Object::new(ctx.clone())?;
        let mut kept = Vec::new();
        let mut i = 0;
        while i < columns.len() {
            let fi = columns[i].0;
            let f = &comp.schema.fields[fi];
            let n = usize::from(f.ty.slots());
            let parts = ["x", "y", "z", "w"];
            let vector = !matches!(
                f.ty,
                FieldType::F64
                    | FieldType::I32
                    | FieldType::U32
                    | FieldType::Tick
                    | FieldType::Bool
                    | FieldType::Entity
                    | FieldType::Enum(_)
            );
            let holder = Object::new(ctx.clone())?;
            for (k, (field, slot, values)) in columns[i..i + n].iter_mut().enumerate() {
                // A copy in QuickJS-ng's own memory, so `memory_bytes` counts it.
                let array = TypedArray::<f64>::new_copy(ctx.clone(), &*values)?;
                if vector {
                    holder.set(parts[k], array.clone())?;
                } else {
                    obj.set(&*f.name, array.clone())?;
                }
                kept.push(Column {
                    slot: *slot,
                    field: *field,
                    original: std::mem::take(values),
                    array: Persistent::save(ctx, array.as_object().clone()),
                });
            }
            if vector {
                obj.set(&*f.name, freeze_new(ctx, holder)?)?;
            }
            i += n;
        }
        cols.set(&*comp.schema.name, freeze_new(ctx, obj)?)?;
        record.comps.push((comp, kept));
    }
    let raw = Object::new(ctx.clone())?;
    raw.set("len", u32::try_from(len).unwrap_or(u32::MAX))?;
    raw.set("ids", TypedArray::<f64>::new_copy(ctx.clone(), &ids)?)?;
    raw.set("cols", freeze_new(ctx, cols)?)?;
    with_call(ctx, "ctx.query", |call, _world| {
        call.counts.rows = call
            .counts
            .rows
            .saturating_add(u32::try_from(len).unwrap_or(u32::MAX));
        call.queries.push(record);
        Ok(())
    })?;
    Ok(freeze_new(ctx, raw)?)
}

/// `ctx.query(spec)`: a query prepared on demand.
pub fn query<'js>(ctx: &Ctx<'js>, args: &[Value<'js>]) -> NResult<'js> {
    let spec = parse_spec(ctx, &arg(ctx, args, 0))?;
    prepare(ctx, &spec)
}

/// The cells the call changed, checked against the overlay with all its commands, in the order
/// they apply: query by query, rows ascending, slots in declaration order.
pub fn changed_cells<'js>(
    ctx: &Ctx<'js>,
    call: &CallState,
    world: &World,
) -> Result<Vec<CellWrite>, ScriptError> {
    let mut out = Vec::new();
    for q in &call.queries {
        // Each column's values now, read once; a detached buffer is refused.
        let mut now: Vec<Vec<Vec<f64>>> = Vec::with_capacity(q.comps.len());
        for (comp, cols) in &q.comps {
            let mut per = Vec::with_capacity(cols.len());
            for col in cols {
                let object = col
                    .array
                    .clone()
                    .restore(ctx)
                    .map_err(|_| detached(comp, col))?;
                let array =
                    TypedArray::<f64>::from_object(object).map_err(|_| detached(comp, col))?;
                // SAFETY: no JavaScript runs while the bytes are read.
                let bytes = unsafe { array.as_bytes() }.ok_or_else(|| detached(comp, col))?;
                let values: Vec<f64> = bytes
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|c| f64::from_ne_bytes(*c))
                    .collect();
                if values.len() != col.original.len() {
                    return Err(detached(comp, col));
                }
                per.push(values);
            }
            now.push(per);
        }
        for (row, id) in q.ids.iter().enumerate() {
            for ((comp, cols), values) in q.comps.iter().zip(&now) {
                for (col, vals) in cols.iter().zip(values) {
                    let (old, new) = (col.original[row], vals[row]);
                    if old.to_bits() == new.to_bits() {
                        continue;
                    }
                    let f = &comp.schema.fields[col.field];
                    let name = if f.ty.slots() > 1 {
                        format!(
                            "{}.{}",
                            f.name,
                            ["x", "y", "z", "w"][usize::from(col.slot - f.first_slot)]
                        )
                    } else {
                        f.name.to_string()
                    };
                    let value = check_number(&f.ty, new, &comp.schema, &name, &|x| {
                        call.overlay.alive(world, x)
                    })
                    .map_err(|e| {
                        let mut e = e.entity(id.to_f64());
                        e.detail.phase = Some(ErrorPhase::WriteBack);
                        e
                    })?;
                    out.push(CellWrite {
                        id: *id,
                        comp: comp.clone(),
                        slot: col.slot,
                        value,
                    });
                }
            }
        }
    }
    Ok(out)
}

fn detached(comp: &Comp, col: &Column) -> ScriptError {
    let f = &comp.schema.fields[col.field];
    ScriptError::new(
        "script.write_type",
        format!(
            "The column {}.{} was detached or replaced during the call.",
            comp.schema.name, f.name
        ),
        ErrorPhase::WriteBack,
    )
    .component(&comp.schema.name)
    .field(&f.name)
    .with("column", json!(format!("{}.{}", comp.schema.name, f.name)))
}
