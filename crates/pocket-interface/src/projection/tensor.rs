//! The tensor projection (shared/contract/projection.md, Tensor projection): for RL observers, the
//! profile's `TensorSpec` fixes the arrays once, so every observation of that profile has the same
//! shapes (Gymnasium's `Dict` of `Box` spaces). Numbers are the `f32` of the unrounded value,
//! booleans 0 or 1, enumerations one-hot, bearings and angles their sine and cosine relative to
//! the heading; facts a percept does not show are 0, with the `present` column saying which rows
//! are real.

use pocket_contract::codes::definition_invalid;
use pocket_contract::{Pointer, Problem};
use pocket_physics::geom::{self, f32_of};
use pocket_physics::query::cast_ray_among;
use pocket_sim::math;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::json::{Obj, array};
use crate::perception::defs::{ObserverProfile, PerceptionDefs, Unit};
use crate::perception::facts;
use crate::perception::geometry::{Eye, relative_deg};
use crate::perception::state::{FactValue, Occluder, Perceivable, VisibilityScale};
use crate::perception::view::{Percept, PerceptionView};

/// At most this many rows or rays in a block.
pub const MAX_COUNT: u32 = 1024;

/// The tensor projection's layout, fixed per profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TensorSpec {
    pub blocks: Vec<TensorBlock>,
}

/// One array of the observation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "block", rename_all = "snake_case", deny_unknown_fields)]
pub enum TensorBlock {
    /// f32[n]: the named instruments, encoded.
    Instruments {
        name: String,
        instruments: Vec<String>,
    },
    /// f32[count, 4 + features]: the `count` highest-ranked percepts of these kinds; per row
    /// present, sin and cos of the relative bearing, range / sight range, then each named fact.
    Nearest {
        name: String,
        kinds: Vec<String>,
        count: u32,
        facts: Vec<String>,
    },
    /// f32[count, kinds + 1]: rays spread evenly over `fov_deg` about the heading, each a one-hot
    /// of the first seen perceivable kind it meets (or none) and the hit distance / `range_m`
    /// (1 when nothing is hit).
    Rays {
        name: String,
        count: u32,
        fov_deg: f64,
        range_m: f64,
        kinds: Vec<String>,
    },
}

/// The width a value of `unit` takes.
pub fn width(unit: &Unit) -> usize {
    match unit {
        Unit::Bearing | Unit::Angle => 2,
        Unit::Enum { values } => values.len(),
        Unit::Position => 3,
        Unit::Text => 0,
        _ => 1,
    }
}

/// Appends the encoding of `v` (absent: zeros) to `out`.
fn encode(out: &mut Vec<f32>, unit: &Unit, v: Option<&FactValue>, heading: f64) {
    let w = width(unit);
    let start = out.len();
    out.resize(start + w, 0.0);
    let Some(v) = v else {
        return;
    };
    let slot = &mut out[start..];
    match (unit, v) {
        (Unit::Bearing, FactValue::Number(x)) | (Unit::Angle, FactValue::Number(x)) => {
            let a = if matches!(unit, Unit::Bearing) {
                relative_deg(*x, heading)
            } else {
                *x
            };
            let (s, c) = math::sin_cos(a.to_radians());
            slot[0] = f32_of(s);
            slot[1] = f32_of(c);
        }
        (Unit::Enum { values }, FactValue::Text(t)) => {
            if let Some(i) = values.iter().position(|x| x == t) {
                slot[i] = 1.0;
            }
        }
        (Unit::Position, FactValue::Position(p)) => {
            for (i, x) in p.iter().enumerate() {
                slot[i] = f32_of(*x);
            }
        }
        (_, FactValue::Bool(b)) if w == 1 => slot[0] = if *b { 1.0 } else { 0.0 },
        (_, FactValue::Number(x)) if w == 1 => slot[0] = f32_of(*x),
        _ => {}
    }
}

/// The unit of a fact as the first of `kinds` declaring it declares it.
fn fact_unit<'d>(defs: &'d PerceptionDefs, kinds: &[String], fact: &str) -> Option<&'d Unit> {
    kinds
        .iter()
        .filter_map(|k| defs.kind(k))
        .find_map(|k| k.facts.iter().find(|f| f.name == fact))
        .map(|f| &f.unit)
}

impl TensorSpec {
    /// Checks the layout against the declarations (`definition.invalid`).
    pub fn validate(
        &self,
        defs: &PerceptionDefs,
        profile: &ObserverProfile,
        at: &Pointer,
    ) -> Result<(), Problem> {
        let bad = |p: Pointer, why: String| Err(definition_invalid(&p, &why));
        for (i, b) in self.blocks.iter().enumerate() {
            let at = at.key("blocks").index(i);
            match b {
                TensorBlock::Instruments { instruments, .. } => {
                    for n in instruments {
                        if !profile.instruments.contains(n) {
                            return bad(at, format!("the profile reads no instrument '{n}'"));
                        }
                    }
                }
                TensorBlock::Nearest {
                    kinds,
                    count,
                    facts,
                    ..
                } => {
                    if *count == 0 || *count > MAX_COUNT {
                        return bad(at, format!("a count is 1 to {MAX_COUNT}"));
                    }
                    for k in kinds {
                        if defs.kind(k).is_none() {
                            return bad(at, format!("no kind '{k}'"));
                        }
                    }
                    for f in facts {
                        if fact_unit(defs, kinds, f).is_none() {
                            return bad(at, format!("no kind of the block has a fact '{f}'"));
                        }
                    }
                }
                TensorBlock::Rays {
                    count,
                    fov_deg,
                    range_m,
                    kinds,
                    ..
                } => {
                    let ok = *count > 0
                        && *count <= MAX_COUNT
                        && *fov_deg > 0.0
                        && *fov_deg <= 360.0
                        && range_m.is_finite()
                        && *range_m > 0.0;
                    if !ok {
                        return bad(at, "rays need a count, a field of view and a range".into());
                    }
                    if let Some(k) = kinds.iter().find(|k| defs.kind(k).is_none()) {
                        return bad(at, format!("no kind '{k}'"));
                    }
                }
            }
        }
        Ok(())
    }
}

/// One block's shape and data.
struct Block {
    name: String,
    shape: Vec<usize>,
    data: Vec<f32>,
}

/// The tensor answer of `view`: `{tick, omniscient, tensors: {name: {shape, data}}}`, `data`
/// row-major. `targets` rank first, as in every other projection.
pub fn observation(
    view: &PerceptionView<'_>,
    spec: &TensorSpec,
    targets: &[pocket_sim::EntityId],
) -> String {
    let defs = view.defs();
    let heading = view.heading();
    let percepts = view.percepts(targets);
    let mut blocks = Vec::new();
    for b in &spec.blocks {
        blocks.push(match b {
            TensorBlock::Instruments { name, instruments } => {
                let mut data = Vec::new();
                for n in instruments {
                    let Some(def) = defs.instrument(n) else {
                        continue;
                    };
                    encode(&mut data, &def.unit, view.instrument(n).as_ref(), heading);
                }
                Block {
                    name: name.clone(),
                    shape: vec![data.len()],
                    data,
                }
            }
            TensorBlock::Nearest {
                name,
                kinds,
                count,
                facts: names,
            } => nearest(view, &percepts, name, kinds, *count, names),
            TensorBlock::Rays {
                name,
                count,
                fov_deg,
                range_m,
                kinds,
            } => rays(view, name, *count, *fov_deg, *range_m, kinds),
        });
    }
    let mut tensors = Obj::new();
    for b in &blocks {
        let shape: Vec<String> = b.shape.iter().map(ToString::to_string).collect();
        let data: Vec<String> = b
            .data
            .iter()
            .map(|x| serde_json::to_string(x).unwrap_or_else(|_| "0".to_owned()))
            .collect();
        tensors.raw(
            &b.name,
            &Obj::new()
                .raw("shape", &array(&shape))
                .raw("data", &array(&data))
                .finish(),
        );
    }
    Obj::new()
        .uint("tick", view.tick().0)
        .bool("omniscient", view.omniscient())
        .raw("tensors", &tensors.finish())
        .finish()
}

fn nearest(
    view: &PerceptionView<'_>,
    percepts: &[Percept],
    name: &str,
    kinds: &[String],
    count: u32,
    names: &[String],
) -> Block {
    let defs = view.defs();
    let units: Vec<Option<&Unit>> = names.iter().map(|f| fact_unit(defs, kinds, f)).collect();
    let features: usize = units.iter().map(|u| u.map_or(0, width)).sum();
    let cols = 4 + features;
    let rows = usize::try_from(count).unwrap_or(0);
    let mut data = Vec::with_capacity(rows * cols);
    let sight = view.profile().sight.as_ref().map_or(1.0, |s| s.range_m);
    for p in percepts
        .iter()
        .filter(|p| kinds.contains(&p.kind))
        .take(rows)
    {
        let (s, c) = math::sin_cos(view.relative_bearing(p).to_radians());
        data.extend([1.0, f32_of(s), f32_of(c), f32_of(p.range_m / sight)]);
        for (f, unit) in names.iter().zip(&units) {
            if let Some(unit) = unit {
                let v = p.facts.iter().find(|r| &r.name == f).map(|r| &r.value);
                encode(&mut data, unit, v, view.heading());
            }
        }
    }
    data.resize(rows * cols, 0.0);
    Block {
        name: name.to_owned(),
        shape: vec![rows, cols],
        data,
    }
}

fn rays(
    view: &PerceptionView<'_>,
    name: &str,
    count: u32,
    fov_deg: f64,
    range_m: f64,
    kinds: &[String],
) -> Block {
    let w = view.world();
    let cols = kinds.len() + 1;
    let rows = usize::try_from(count).unwrap_or(0);
    let mut data = vec![0.0f32; rows * cols];
    let vis = w
        .get_resource::<VisibilityScale>()
        .copied()
        .unwrap_or_default()
        .0;
    let sight = view.profile().sight.as_ref();
    let Some(tf) = facts::live(w, view.body()).and_then(|e| w.get::<pocket_physics::Transform>(e))
    else {
        return Block {
            name: name.to_owned(),
            shape: vec![rows, cols],
            data,
        };
    };
    let (eye, reach) = match sight {
        Some(s) => (Eye::of(tf, s).at, math::min(range_m, s.range_m) * vis),
        None => (tf.position, 0.0),
    };
    // Rays meet the perceivable entities the observer sees now and the occluders that are not
    // perceivable (terrain); they pass through what it does not see, so a ray tells nothing a
    // percept does not (docs/spec/perception-slice2.md 5).
    let accept = |id: pocket_sim::EntityId| {
        id != view.body()
            && facts::live(w, id).is_some_and(|e| match w.get::<Perceivable>(e) {
                Some(_) => view
                    .percept(id)
                    .is_some_and(|p| p.visibility == crate::perception::state::Visibility::Seen),
                None => w.get::<Occluder>(e).is_some(),
            })
    };
    for i in 0..rows {
        let row = &mut data[i * cols..(i + 1) * cols];
        row[cols - 1] = 1.0;
        if reach <= 0.0 {
            continue;
        }
        let k = u32::try_from(i).map_or(0.0, f64::from);
        let off = -fov_deg / 2.0 + (k + 0.5) * fov_deg / f64::from(count);
        let dir = geom::from_bearing(view.heading() + off);
        // A ray the solver cannot hold (an eye or reach past ±3.4e38) is refused and, like a ray
        // that meets nothing, tells nothing.
        let Ok(Some(hit)) = cast_ray_among(w, eye, dir, reach, &accept) else {
            continue;
        };
        row[cols - 1] = f32_of(hit.distance / range_m);
        let kind = facts::live(w, hit.entity).and_then(|e| w.get::<Perceivable>(e));
        if let Some(j) = kind.and_then(|p| kinds.iter().position(|k| *k == p.kind)) {
            row[j] = 1.0;
        }
    }
    Block {
        name: name.to_owned(),
        shape: vec![rows, cols],
        data,
    }
}
