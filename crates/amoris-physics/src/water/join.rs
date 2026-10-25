//! Joined regions (water.md 64.3). Before any region steps, each seam between two joined sides
//! is worked out once from the edge columns both regions' last steps left, the same for every
//! region however the tick batches them, and both take it: what one gives the other receives. The
//! regions of a joined group rest only together, since one resting while its neighbour steps would
//! let water leave the one without reaching the other.
use super::{Edge, WaterRegion, resident::WaterResident, solve::G};
use amoris_assets::water_field::FieldExterior;
use amoris_contract::{Problem, detail};
use amoris_sim::EntityId;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// A joined resident region as the tick finds it.
pub(super) struct Member<'a> {
    pub id: EntityId,
    /// World x and z of its grid's corner.
    pub corner: [f64; 2],
    pub region: &'a WaterRegion,
    /// Its state; none before its first step.
    pub state: Option<&'a WaterResident>,
    /// It may rest by itself this tick (water.md 64).
    pub held: bool,
    /// Nothing changes it this tick and it lies beyond every focus: it rests whatever its
    /// neighbours do, a reservoir at its held state to them.
    pub away: bool,
}

#[derive(Default)]
pub(super) struct Joins {
    /// Per region, its joined sides' exteriors this tick.
    pub exteriors: BTreeMap<EntityId, Vec<FieldExterior>>,
    /// The joined regions that rest this tick.
    pub rests: BTreeSet<EntityId>,
}

pub(super) fn joined(region: &WaterRegion) -> bool {
    region
        .edges
        .iter()
        .any(|e| matches!(e, Edge::Joined { .. }))
}

pub(super) fn plan(members: &[Member<'_>], dt: f64) -> Result<Joins, Problem> {
    let index: BTreeMap<EntityId, usize> =
        members.iter().enumerate().map(|(n, m)| (m.id, n)).collect();
    let mut joins = Joins::default();
    let mut group: Vec<usize> = (0..members.len()).collect();
    for (n, m) in members.iter().enumerate() {
        for (side, number) in (0..4).zip(0u8..) {
            let Edge::Joined { region: other } = m.region.edges[side] else {
                continue;
            };
            let &o = index
                .get(&other)
                .ok_or_else(|| invalid(m.id, side, "joins what is not a resident water region"))?;
            let (first, from, count) = overlap(m, side, &members[o])?;
            let (a, b) = (root(&mut group, n), root(&mut group, o));
            group[a] = b;
            if let (Some(ours), Some(theirs)) = (edge(m, side), edge(&members[o], side ^ 1)) {
                let ours = &ours[first as usize..first as usize + count];
                let theirs = &theirs[from..from + count];
                // Worked out from the +x or +z side's region, as its neighbour works it out.
                let (low, high, l, h) = if side % 2 == 1 {
                    (ours, theirs, m.region, members[o].region)
                } else {
                    (theirs, ours, members[o].region, m.region)
                };
                let faces = seam(low, high, l, h, dt).ok_or_else(|| {
                    invalid(
                        m.id,
                        side,
                        "is too deep for its seam: a wave would cross more than a column a tick",
                    )
                })?;
                joins
                    .exteriors
                    .entry(m.id)
                    .or_default()
                    .push(FieldExterior {
                        side: number,
                        first,
                        faces,
                    });
            }
        }
    }
    let mut held = vec![true; members.len()];
    for (n, m) in members.iter().enumerate() {
        let r = root(&mut group, n);
        held[r] &= m.held;
    }
    for (n, m) in members.iter().enumerate() {
        let r = root(&mut group, n);
        if m.away || held[r] {
            joins.rests.insert(m.id);
        }
    }
    Ok(joins)
}

/// A side's edge columns as the region's last step left them, if it has them all.
fn edge<'a>(m: &Member<'a>, side: usize) -> Option<&'a [[f64; 4]]> {
    let along = m.region.size[1 - side / 2] as usize;
    m.state?
        .edges
        .iter()
        .find(|e| usize::from(e.side) == side)
        .map(|e| e.columns.as_slice())
        .filter(|c| c.len() == along)
}

/// The seam's faces this tick, from the edge columns of the region whose +x or +z side it is
/// (`low`) and of its neighbour (`high`): each face's velocity and discharge per unit width along
/// the axis. The two surfaces push the face over the higher of the two beds (the hydrostatic
/// reconstruction of E. Audusse, F. Bouchut, M.-O. Bristeau, R. Klein and B. Perthame, SIAM J.
/// Sci. Comput. 25, 2004), it is carried from upwind and damped as the regions' interior faces
/// are, and it carries its upwind column's depth. Both regions' copies of the face start from
/// their mean, so they stay one. None where a wave would cross more than a column in the tick,
/// past what one explicit step a tick holds.
#[allow(clippy::cast_possible_truncation)] // The GPU's seam is f32.
fn seam(
    low: &[[f64; 4]],
    high: &[[f64; 4]],
    l: &WaterRegion,
    h: &WaterRegion,
    dt: f64,
) -> Option<Vec<[f32; 2]>> {
    use amoris_sim::math::{max, min};
    let cell = l.cell_m;
    let friction = 0.5 * (l.friction + h.friction);
    let calm = 0.5 * (l.calm_per_s + h.calm_per_s);
    // Within the faster limit of either region's fewest substeps (water_boundaries.wgsl).
    let limit = 0.5 * cell * f64::from(l.substeps.min(h.substeps)) / dt;
    low.iter()
        .zip(high)
        .map(|(a, b)| {
            let top = max(a[1], b[1]);
            let (ha, hb) = (max(a[0] - top, 0.0), max(b[0] - top, 0.0));
            if amoris_sim::math::sqrt(G * max(ha, hb)) * dt >= cell {
                return None;
            }
            if ha <= DRY && hb <= DRY {
                return Some([0.0, 0.0]);
            }
            let own = 0.5 * (a[3] + b[3]);
            let upwind = if own > 0.0 { a[2] } else { b[2] };
            let mut u =
                own - min(own.abs() * dt / cell, 1.0) * (own - upwind) - G * dt * (hb - ha) / cell;
            let calm = calm * min(1.0, 0.5 / max(u.abs(), 1e-6));
            u /= 1.0 + dt * (friction / max(max(ha, hb), 0.05) + calm);
            u = u.clamp(-limit, limit);
            let depth = if u > 0.0 { ha } else { hb };
            if depth <= DRY {
                return Some([0.0, 0.0]);
            }
            // At most half its upwind column leaves through one face in a tick.
            let most = 0.5 * depth * cell / dt;
            Some([u as f32, (u * depth).clamp(-most, most) as f32])
        })
        .collect()
}

/// Water this shallow does not flow, m (water_boundaries.wgsl).
const DRY: f64 = 1e-4;

fn root(group: &mut [usize], mut n: usize) -> usize {
    while group[n] != n {
        group[n] = group[group[n]];
        n = group[n];
    }
    n
}

/// Where `n`'s facing side meets `m`'s side `side`: the first of `m`'s cells along it that `n`
/// covers, the matching cell of `n`'s, and how many.
fn overlap(m: &Member<'_>, side: usize, n: &Member<'_>) -> Result<(u32, usize, usize), Problem> {
    let (a, b) = (m.region, n.region);
    if m.id == n.id || b.edges[side ^ 1] != (Edge::Joined { region: m.id }) {
        return Err(invalid(
            m.id,
            side,
            "joins a region whose facing side is not joined back to it",
        ));
    }
    let cell = a.cell_m;
    if (a.cell_m - b.cell_m).abs() > 1e-9 * cell {
        return Err(invalid(m.id, side, "joined regions need one cell size"));
    }
    let (normal, along) = (side / 2, 1 - side / 2);
    let far = |corner: [f64; 2], size: [u32; 2]| corner[normal] + cell * f64::from(size[normal]);
    let (here, there) = if side % 2 == 1 {
        (far(m.corner, a.size), n.corner[normal])
    } else {
        (m.corner[normal], far(n.corner, b.size))
    };
    if (here - there).abs() > 1e-6 * cell {
        return Err(invalid(m.id, side, "joined sides must meet"));
    }
    let shift = (n.corner[along] - m.corner[along]) / cell;
    let offset = shift.round();
    if (shift - offset).abs() > 1e-6 {
        return Err(invalid(m.id, side, "joined sides' cells must line up"));
    }
    // Beyond the sides' lengths the sides cannot overlap; a cast saturates.
    #[allow(clippy::cast_possible_truncation)]
    let offset = offset.clamp(-1024.0, 1024.0) as i64;
    let lo = offset.max(0);
    let hi = (offset + i64::from(b.size[along])).min(i64::from(a.size[along]));
    let cells = |v: i64| usize::try_from(v).ok();
    match (u32::try_from(lo), cells(lo - offset), cells(hi - lo)) {
        (Ok(first), Some(from), Some(count)) if hi > lo => Ok((first, from, count)),
        _ => Err(invalid(m.id, side, "joined sides must overlap")),
    }
}

fn invalid(id: EntityId, side: usize, message: &str) -> Problem {
    Problem::new(
        "water.invalid",
        format!("region {} edge {side} {message}", id.get()),
        detail([("field", json!(format!("edges.{side}")))]),
    )
}
