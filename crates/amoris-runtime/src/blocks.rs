//! `player.blocks` (docs/spec/voxels.md 11.1): what a seat sees of the block volumes, as the
//! surface cells its rays reach. The seat's sight decides: rays leave its eye over its field of
//! view, at most its range (and at most [`MAX_RANGE_M`]), and stop at the first cell that is not
//! air, so a cell behind another is never seen. The answer lists each seen cell once, nearest first,
//! within the call's token budget, and the cell the eye looks at.
//!
//! A Read outside the tick: it changes nothing, and the same world, seat and parameters give the
//! same bytes (the deterministic math library, cells in a fixed order).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use amoris_contract::{CheckOptions, Problem, detail};
use amoris_interface::perception::PerceptionView;
use amoris_interface::projection::tokens;
use amoris_link::Source;
use amoris_physics::Transform;
use amoris_sim::EntityIndex;
use amoris_sim::math::{min, sin_cos};
use amoris_sim::voxels::Voxels;
use bevy_ecs::prelude::World;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use amoris_interface::action::Caller;

use crate::player::caller;

/// The farthest a query reaches, whatever the sight's range.
pub const MAX_RANGE_M: f64 = 32.0;
/// The default reach.
pub const RANGE_M: f64 = 16.0;
/// Rays across the field of view, and up and down it.
pub const COLUMNS: u32 = 32;
pub const ROWS: u32 = 12;
/// The vertical field of view the rows span, degrees.
pub const VERTICAL_DEG: f64 = 60.0;
/// The default budget, tokens.
pub const BUDGET_TOKENS: u32 = 600;

/// `player.blocks`'s parameters.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BlocksParams {
    /// A player's own; a developer names one, or follows the only seat.
    #[serde(default)]
    pub seat: Option<String>,
    /// How far to look, metres (default 16; at most the sight's range and 32).
    #[serde(default)]
    pub range_m: Option<f64>,
    /// The answer's budget (default 600 tokens): the nearest cells that fit.
    #[serde(default)]
    #[schemars(range(min = 50, max = 8000))]
    pub budget_tokens: Option<u32>,
    /// "text" or "json" (the default).
    #[serde(default)]
    pub projection: Option<String>,
}

/// A cell seen.
struct Seen {
    /// The volume (its place in id order) and the cell, which name the cell once.
    key: (usize, [i32; 3]),
    block: String,
    /// The centre of the cell, world metres.
    at: [f64; 3],
    /// The face a ray entered by.
    face: &'static str,
    distance_m: f64,
}

fn face(n: [i32; 3]) -> &'static str {
    match n {
        [1, 0, 0] => "+x",
        [-1, 0, 0] => "-x",
        [0, 1, 0] => "+y",
        [0, -1, 0] => "-y",
        [0, 0, 1] => "+z",
        [0, 0, -1] => "-z",
        _ => "inside",
    }
}

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// `v` turned by the unit quaternion `q` (`[x, y, z, w]`).
fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let [x, y, z, w] = q;
    let t = [
        2.0 * (y * v[2] - z * v[1]),
        2.0 * (z * v[0] - x * v[2]),
        2.0 * (x * v[1] - y * v[0]),
    ];
    [
        v[0] + w * t[0] + (y * t[2] - z * t[1]),
        v[1] + w * t[1] + (z * t[0] - x * t[2]),
        v[2] + w * t[2] + (x * t[1] - y * t[0]),
    ]
}

/// The volumes of the world, in id order, with their origins.
fn volumes(world: &World) -> Vec<(&Voxels, [f64; 3])> {
    let Some(index) = world.get_resource::<EntityIndex>() else {
        return Vec::new();
    };
    index
        .iter()
        .filter_map(|(_, e)| {
            let v = world.get::<Voxels>(e)?;
            let origin = world.get::<Transform>(e).map_or([0.0; 3], |t| t.position);
            Some((v, origin))
        })
        .collect()
}

/// The nearest cell that is not air a ray meets in any volume.
fn cast(vols: &[(&Voxels, [f64; 3])], eye: [f64; 3], dir: [f64; 3], max_m: f64) -> Option<Seen> {
    let mut best: Option<Seen> = None;
    for (i, (v, origin)) in vols.iter().enumerate() {
        let Some(hit) = v.raycast(*origin, eye, dir, max_m, false) else {
            continue;
        };
        if best
            .as_ref()
            .is_some_and(|b| b.distance_m <= hit.distance_m)
        {
            continue;
        }
        let name = v
            .palette
            .get(usize::from(hit.block))
            .map_or_else(|| format!("#{}", hit.block), |b| b.name.clone());
        let at = [0, 1, 2].map(|a| origin[a] + (f64::from(hit.cell[a]) + 0.5) * v.cell_m);
        best = Some(Seen {
            key: (i, hit.cell),
            block: name,
            at,
            face: face(hit.normal),
            distance_m: hit.distance_m,
        });
    }
    best
}

fn seen_json(s: &Seen) -> Value {
    json!({"block": s.block, "at": s.at.map(r2), "face": s.face, "distance_m": r2(s.distance_m)})
}

fn seen_line(s: &Seen) -> String {
    let [x, y, z] = s.at.map(r2);
    format!("{} {x},{y},{z} {} {}\n", s.block, s.face, r2(s.distance_m))
}

/// `player.blocks`.
pub fn blocks(world: &World, source: Source, raw: &Value) -> Result<Value, Problem> {
    let who = caller(world, source)?;
    let p: BlocksParams =
        amoris_contract::decode(raw, &CheckOptions::new("the blocks request"))?.value;
    let seat = match &who {
        Caller::Player { seat } => {
            if let Some(named) = &p.seat
                && named != seat
            {
                return Err(amoris_contract::codes::seat_not_yours(
                    &amoris_contract::Pointer::root().key("seat"),
                    named,
                    seat,
                ));
            }
            seat.clone()
        }
        _ => amoris_interface::time::session::seat_of(world, &who, p.seat.as_deref(), "blocks")?
            .unwrap_or_default(),
    };
    let text = match p.projection.as_deref() {
        None | Some("json") => false,
        Some("text") => true,
        Some(other) => {
            return Err(amoris_contract::codes::invalid_value(
                &amoris_contract::Pointer::root().key("projection"),
                other,
                &[
                    amoris_contract::Candidate::new("text"),
                    amoris_contract::Candidate::new("json"),
                ],
            ));
        }
    };
    let range_req = p.range_m.unwrap_or(RANGE_M);
    if !(range_req.is_finite() && range_req > 0.0) {
        return Err(Problem::new(
            "request.out_of_range",
            format!("range_m must be a finite number above 0; got {range_req}."),
            detail([("path", json!("/range_m")), ("got", json!(range_req))]),
        ));
    }
    let view = PerceptionView::for_seat(world, &seat).ok_or_else(|| {
        Problem::new(
            "perception.no_observer",
            format!("The seat '{seat}' has no body that perceives (an Observer with a profile)."),
            detail([("seat", json!(seat))]),
        )
    })?;
    let tick = view.tick().0;
    let body = world
        .get_resource::<EntityIndex>()
        .and_then(|i| i.get(view.body()))
        .and_then(|e| world.get::<Transform>(e))
        .copied()
        .unwrap_or_default();
    let budget = p.budget_tokens.unwrap_or(BUDGET_TOKENS);
    let (range_m, fov_deg, eye) = match &view.profile().sight {
        Some(s) => (
            min(min(range_req, s.range_m), MAX_RANGE_M),
            s.fov_deg.clamp(1.0, 360.0),
            {
                let off = rotate(body.rotation, s.eye_m.array());
                [0, 1, 2].map(|a| body.position[a] + off[a])
            },
        ),
        // Blind: no rays.
        None => (0.0, 0.0, body.position),
    };
    let look = rotate(body.rotation, [0.0, 0.0, -1.0]);
    let vols = volumes(world);
    // One ray per grid point over the field of view, in the body's frame (+y up, -z forward).
    let mut cells: BTreeMap<(usize, [i32; 3]), Seen> = BTreeMap::new();
    let mut rays = 0u32;
    if range_m > 0.0 {
        let deg = std::f64::consts::PI / 180.0;
        for row in 0..ROWS {
            let b = (-VERTICAL_DEG / 2.0 + VERTICAL_DEG * (f64::from(row) + 0.5) / f64::from(ROWS))
                * deg;
            let (sb, cb) = sin_cos(b);
            for col in 0..COLUMNS {
                let a =
                    (-fov_deg / 2.0 + fov_deg * (f64::from(col) + 0.5) / f64::from(COLUMNS)) * deg;
                let (sa, ca) = sin_cos(a);
                let dir = rotate(body.rotation, [sa * cb, sb, -ca * cb]);
                rays += 1;
                if let Some(s) = cast(&vols, eye, dir, range_m) {
                    let key = s.key;
                    let nearer = cells.get(&key).is_none_or(|o| s.distance_m < o.distance_m);
                    if nearer {
                        cells.insert(key, s);
                    }
                }
            }
        }
    }
    let looking_at = if range_m > 0.0 {
        cast(&vols, eye, look, range_m)
    } else {
        None
    };
    let mut seen: Vec<Seen> = cells.into_values().collect();
    seen.sort_by(|a, b| {
        a.distance_m
            .total_cmp(&b.distance_m)
            .then_with(|| a.key.cmp(&b.key))
    });
    let total = seen.len();
    if text {
        let [ex, ey, ez] = eye.map(r2);
        let [lx, ly, lz] = look.map(r2);
        let mut head = format!(
            "blocks tick {tick} seat {seat} eye {ex},{ey},{ez} look {lx},{ly},{lz} range {} m, \
             {rays} rays: {total} cells\n",
            r2(range_m)
        );
        if let Some(s) = &looking_at {
            let _ = write!(head, "looking_at {}", seen_line(s));
        }
        if tokens(head.len()) > budget {
            return Err(too_small(budget, tokens(head.len())));
        }
        let mut body_text = String::new();
        let mut kept = 0;
        for s in &seen {
            let line = seen_line(s);
            // Room for the omitted line, which is at most this long.
            if tokens(head.len() + body_text.len() + line.len() + 24) > budget {
                break;
            }
            body_text.push_str(&line);
            kept += 1;
        }
        let mut out = head + &body_text;
        if kept < total {
            let _ = writeln!(out, "omitted {} farther", total - kept);
        }
        return Ok(Value::String(out));
    }
    let mut out = json!({
        "tick": tick,
        "seat": seat,
        "eye": eye.map(r2),
        "look": look.map(r2),
        "range_m": r2(range_m),
        "rays": rays,
        "blocks": [],
    });
    if let Some(s) = &looking_at {
        out["looking_at"] = seen_json(s);
    }
    let head = out.to_string().len() + 40;
    if tokens(head) > budget {
        return Err(too_small(budget, tokens(head)));
    }
    let mut used = head;
    let mut list = Vec::new();
    for s in &seen {
        let v = seen_json(s);
        let n = v.to_string().len() + 1;
        if tokens(used + n) > budget {
            break;
        }
        used += n;
        list.push(v);
    }
    let kept = list.len();
    out["blocks"] = Value::Array(list);
    if kept < total {
        out["omitted"] = json!(total - kept);
    }
    out["tokens"] = json!(tokens(out.to_string().len()));
    Ok(out)
}

fn too_small(budget: u32, min: u32) -> Problem {
    Problem::new(
        "perception.budget_too_small",
        format!("A budget of {budget} tokens cannot hold this answer's head; give at least {min}."),
        detail([("budget_tokens", json!(budget)), ("min_tokens", json!(min))]),
    )
}
