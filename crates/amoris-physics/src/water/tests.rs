use super::solve::{Grid, substeps_for};
use super::*;

const DT: f64 = 1.0 / 60.0;

/// A region of `tx` by `tz` tiles of `cell_m` columns over a flat bed at 0, dry.
pub(crate) fn region(tx: u32, tz: u32, cell_m: f64) -> WaterRegion {
    let mut r = WaterRegion {
        solver: Solver::Cpu,
        cell_m,
        size: [tx * 16, tz * 16],
        bed: BedSource::Voxels {
            volume: EntityId::FIRST,
            per: 2,
            y_lo: 0,
            y_hi: 63,
        },
        edges: [Edge::Wall; 4],
        friction: 0.02,
        substeps: 2,
        dry_below: 0.0,
        look: WaterLook::default(),
        tiles: Vec::new(),
    };
    for kz in 0..tz {
        for kx in 0..tx {
            r.tiles.push(WaterTile::dry([kx, kz], 0.0));
        }
    }
    r
}

/// Sets column `(i, k)`'s bed and depth.
pub(crate) fn set(r: &mut WaterRegion, i: usize, k: usize, bed: f64, depth: f64) {
    let (t, c) = r.locate(i, k);
    r.tiles[t].bed[c] = bed;
    r.tiles[t].depth[c] = depth;
}

pub(crate) fn depth(r: &WaterRegion, i: usize, k: usize) -> f64 {
    let (t, c) = r.locate(i, k);
    r.tiles[t].depth[c]
}

fn surface(r: &WaterRegion, i: usize, k: usize) -> f64 {
    let (t, c) = r.locate(i, k);
    r.tiles[t].bed[c] + r.tiles[t].depth[c]
}

fn run(r: &mut WaterRegion, ticks: u32) -> Vec<Stepped> {
    let mut g = Grid::default();
    (0..ticks).map(|_| step_region(r, DT, &mut g)).collect()
}

pub(crate) use super::hash;

/// The spike's bench scene on a region: bumps up to 0.8 m, a block of water 2 m deep in one
/// quarter that breaks like a dam.
pub(crate) fn dam(tiles: u32) -> WaterRegion {
    let mut r = region(tiles, tiles, 0.5);
    let n = r.columns_along()[0];
    for k in 0..n {
        for i in 0..n {
            let bump = 0.2 * f64::from(u32::try_from((i * 7 + k * 13) % 5).unwrap());
            set(
                &mut r,
                i,
                k,
                bump,
                if i < n / 2 && k < n / 2 { 2.0 } else { 0.0 },
            );
        }
    }
    r
}

#[test]
fn a_dam_break_conserves_water_and_settles_level() {
    let mut r = dam(2);
    let v0 = r.volume();
    run(&mut r, 60 * 120);
    let v1 = r.volume();
    assert!(((v1 - v0) / v0).abs() < 1e-9, "volume {v0} then {v1}");
    let n = r.columns_along()[0];
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for k in 0..n {
        for i in 0..n {
            if depth(&r, i, k) > 0.05 {
                lo = math::min(lo, surface(&r, i, k));
                hi = math::max(hi, surface(&r, i, k));
            }
        }
    }
    assert!(hi - lo < 0.02, "the surface settles level: {lo} to {hi}");
}

use amoris_sim::math;

#[test]
fn walls_hold_water_against_the_regions_edges() {
    // Water piled against the -x and +z walls stays in the region.
    let mut r = region(1, 1, 0.5);
    for k in 0..16 {
        set(&mut r, 0, k, 0.0, 3.0);
        set(&mut r, 15, k, 0.0, 3.0);
    }
    let v0 = r.volume();
    run(&mut r, 600);
    assert!(((r.volume() - v0) / v0).abs() < 1e-12);
    assert!(depth(&r, 8, 8) > 0.2, "it spread inside");
}

#[test]
fn a_ledge_drains_onto_the_ground_below() {
    // A shelf 2 m high over the first 8 columns of a 32 x 16 strip, holding water 1 m deep, and a
    // wall of bed at its edge broken in the middle: the shelf drains to the low side.
    let mut r = region(2, 1, 0.5);
    for k in 0..16 {
        for i in 0..8 {
            set(&mut r, i, k, 2.0, 1.0);
        }
    }
    let shelf = |r: &WaterRegion| {
        (0..16)
            .flat_map(|k| (0..8).map(move |i| (i, k)))
            .map(|(i, k)| depth(r, i, k))
            .sum::<f64>()
    };
    let before = shelf(&r);
    run(&mut r, 60 * 30);
    let after = shelf(&r);
    assert!(
        after < before * 0.05,
        "the shelf drained: {before} then {after}"
    );
    assert!(
        depth(&r, 30, 8) > 0.1,
        "into the far end: {}",
        depth(&r, 30, 8)
    );
    // The water does not climb back up: the shelf's beds stay above the low water.
    assert!(surface(&r, 20, 8) < 2.0);
}

#[test]
fn a_tick_is_the_same_bits_from_the_same_state() {
    let mut a = dam(2);
    let mut b = a.clone();
    run(&mut a, 60);
    run(&mut b, 60);
    assert_eq!(hash(&a), hash(&b));
    assert_ne!(hash(&a), hash(&dam(2)));
}

#[test]
fn the_substep_count_follows_the_fastest_wave() {
    // A wave crossing more than half a column a substep takes more substeps, up to 8.
    assert_eq!(substeps_for(0.0, DT, 0.5), 1);
    assert_eq!(substeps_for(15.0, DT, 0.5), 1);
    assert_eq!(substeps_for(15.01, DT, 0.5), 2);
    assert_eq!(substeps_for(44.0, DT, 0.5), 3);
    assert_eq!(substeps_for(1e6, DT, 0.5), MAX_SUBSTEPS);
    // Shallow water at the region's own count; 10 m deep on 0.25 m columns takes more.
    let mut r = region(1, 1, 0.25);
    set(&mut r, 3, 3, 0.0, 0.1);
    assert_eq!(run(&mut r, 1)[0].substeps, 2);
    let mut r = region(1, 1, 0.25);
    for k in 0..16 {
        for i in 0..8 {
            set(&mut r, i, k, 0.0, 10.0);
        }
    }
    let n = run(&mut r, 1)[0].substeps;
    // sqrt(9.81 * 10) = 9.9 m/s: 2 * dt * 9.9 / 0.25 = 1.32, so 2; the region asks for 2 too.
    assert_eq!(n, 2);
    r.substeps = 1;
    r.cell_m = 0.25;
    for k in 0..16 {
        for i in 0..8 {
            set(&mut r, i, k, 0.0, 40.0);
        }
    }
    r.tiles[0].still = 0;
    // sqrt(9.81 * 40) = 19.8 m/s plus the flow: at least 3.
    assert!(run(&mut r, 1)[0].substeps >= 3);
}

#[test]
fn tiles_sleep_when_still_and_wake_from_a_neighbours_flow() {
    // Three tiles in a row: water in the first, the others dry. Asleep from the start, nothing is
    // stepped.
    let mut r = region(3, 1, 0.5);
    for k in 0..16 {
        for i in 0..4 {
            set(&mut r, i, k, 0.0, 0.5);
        }
    }
    for t in &mut r.tiles {
        t.still = SLEEP_TICKS;
    }
    let before = r.clone();
    let s = run(&mut r, 5);
    assert!(s.iter().all(|s| s.tiles.is_empty()) && r == before);

    // Woken, the first tile steps itself and its neighbour; the far tile is not stepped until the
    // water reaches the middle one and wakes it.
    r.tiles[0].still = 0;
    let s = run(&mut r, 1);
    assert_eq!(s[0].tiles, vec![0, 1]);
    assert_eq!(r.tiles[1].still, SLEEP_TICKS, "still dry after one tick");
    let s = run(&mut r, 60 * 20);
    assert!(s.iter().any(|s| s.tiles.contains(&2)), "the far tile woke");
    assert!(depth(&r, 33, 8) > 0.0, "the water reached it");
    let v = r.volume();
    assert!(((v - 0.5 * 4.0 * 16.0 * 0.25) / v).abs() < 1e-12);
}

#[test]
fn a_still_lake_sleeps_after_60_ticks_and_a_ripple_settles_to_sleep() {
    let mut r = region(2, 2, 0.5);
    for k in 0..32 {
        for i in 0..32 {
            set(&mut r, i, k, 0.0, 0.1);
        }
    }
    let s = run(&mut r, 59);
    assert!(s.iter().all(|s| s.tiles.len() == 4));
    assert!(r.tiles.iter().all(|t| t.still == 59));
    run(&mut r, 1);
    assert!(r.tiles.iter().all(WaterTile::asleep));
    let before = r.clone();
    assert!(run(&mut r, 10).iter().all(|s| s.substeps == 0) && r == before);

    // A 10 cm bump wakes nothing by itself (it is not a source or an edit); woken, it rings out
    // over the lake and the lake sleeps again with its velocities zeroed.
    for k in 4..7 {
        for i in 4..7 {
            set(&mut r, i, k, 0.0, 0.2);
        }
    }
    r.tiles[0].still = 0;
    let s = run(&mut r, 60 * 120);
    assert!(
        s.iter().any(|s| s.tiles.len() == 4),
        "the ripple woke every tile"
    );
    assert!(r.tiles.iter().all(WaterTile::asleep));
    assert!(
        r.tiles
            .iter()
            .all(|t| t.u.iter().chain(&t.w).all(|v| *v == 0.0))
    );
}

#[test]
fn films_thinner_than_dry_below_dry_when_slow_and_are_reported() {
    // Puddles of 1 mm and 1 cm on flat ground: at dry_below 2 mm the first dries the first tick,
    // the second stays.
    let mut r = region(1, 1, 0.5);
    r.dry_below = 0.002;
    set(&mut r, 2, 2, 0.0, 0.001);
    set(&mut r, 10, 10, 0.0, 0.01);
    let s = run(&mut r, 1);
    assert_eq!(depth(&r, 2, 2), 0.0);
    // The puddle's own millimetre, and the thin edge the 1 cm puddle spread.
    assert!(s[0].dried_m3 >= 0.001 * 0.25, "{}", s[0].dried_m3);
    assert!(s[0].dried_m3 < 0.0011 * 0.25, "{}", s[0].dried_m3);
    assert!(depth(&r, 10, 10) > 0.005);

    // With 0 nothing dries: the dam's flood keeps its volume.
    let mut a = dam(2);
    let v0 = a.volume();
    let s = run(&mut a, 600);
    assert!(s.iter().all(|s| s.dried_m3 == 0.0));
    assert!(((a.volume() - v0) / v0).abs() < 1e-9);

    // A flood's moving front does not dry: in the first second of the dam break at 2 mm, the
    // front (thin but fast) runs on, and only a little dries.
    let mut b = dam(2);
    b.dry_below = 0.002;
    let s = run(&mut b, 60);
    let dried: f64 = s.iter().map(|s| s.dried_m3).sum();
    assert!(dried < v0 * 1e-3, "dried {dried} of {v0}");
    let n = b.columns_along()[0];
    assert!(
        depth(&b, n / 2 + 4, n / 4) > 0.0,
        "the front ran past the dam's line"
    );
}

#[test]
fn regions_are_validated() {
    let ok = region(2, 1, 0.5);
    ok.validate().unwrap();
    let mut empty = ok.clone();
    empty.tiles.clear();
    empty.validate().unwrap();
    let mut colliders = empty.clone();
    colliders.bed = BedSource::Colliders {
        top: 10.0,
        bottom: 0.0,
    };
    colliders.validate().unwrap();
    type Change = Box<dyn Fn(&mut WaterRegion)>;
    let bad: Vec<(&str, Change)> = vec![
        ("cell_m", Box::new(|r| r.cell_m = 0.1)),
        ("size.0", Box::new(|r| r.size[0] = 24)),
        ("size.1", Box::new(|r| r.size[1] = 1024)),
        ("substeps", Box::new(|r| r.substeps = 9)),
        ("friction", Box::new(|r| r.friction = f64::NAN)),
        ("dry_below", Box::new(|r| r.dry_below = 0.06)),
        ("edges.1", Box::new(|r| r.edges[1] = Edge::Outflow)),
        (
            "bed.Colliders",
            Box::new(|r| {
                r.bed = BedSource::Colliders {
                    top: 0.0,
                    bottom: 0.0,
                }
            }),
        ),
        (
            "tiles",
            Box::new(|r| {
                r.tiles.pop();
            }),
        ),
        ("tiles.1.key", Box::new(|r| r.tiles[1].key = [0, 1])),
        ("tiles.0.depth", Box::new(|r| r.tiles[0].depth[3] = -1.0)),
        ("tiles.1.u", Box::new(|r| r.tiles[1].u[3] = f64::INFINITY)),
        ("tiles.0.bed", Box::new(|r| r.tiles[0].bed.push(0.0))),
    ];
    for (field, change) in bad {
        let mut r = ok.clone();
        change(&mut r);
        let e = r.validate().unwrap_err();
        assert_eq!(e.code, "water.invalid", "{field}");
        assert_eq!(e.detail["field"], field, "{e:?}");
    }
}

#[test]
fn a_region_reads_from_json_with_its_defaults() {
    let r: WaterRegion = serde_json::from_str(
        r#"{"size": [32, 16], "bed": {"Voxels": {"volume": 3, "per": 2, "y_lo": 0, "y_hi": 63}}}"#,
    )
    .unwrap();
    assert_eq!(r.cell_m, 1.0);
    assert_eq!(r.substeps, 2);
    assert_eq!(r.edges, [Edge::Wall; 4]);
    assert!(r.tiles.is_empty());
    r.validate().unwrap();
}

// --- Beds from a volume (water.md 5) ------------------------------------------------------------

use amoris_sim::voxels::{Block, Voxels};

const STONE: u8 = 1;
const LEAVES: u8 = 2;

/// A volume of 1 m cells: stone from y 0 to 4 under x and z 0..32, a pit 3 blocks deep at
/// x 8..16, z 8..16 (its floor's top at 2 m); the region over it at two columns a block.
fn ground() -> (Voxels, WaterRegion, bed::Map) {
    let mut v = Voxels::default();
    for name in ["stone", "leaves"] {
        v.palette.push(Block {
            name: name.into(),
            solid: true,
            faces: ["s".into(), "s".into(), "s".into()],
            ..Block::default()
        });
    }
    v.fill([0, 0, 0], [31, 4, 31], STONE, 0).unwrap();
    v.fill([8, 2, 8], [15, 4, 15], 0, 0).unwrap();
    let mut r = region(4, 4, 0.5);
    r.tiles.clear();
    r.bed = BedSource::Voxels {
        volume: EntityId::FIRST,
        per: 2,
        y_lo: 0,
        y_hi: 15,
    };
    let m = bed::map(&r, [0.0, 0.0], &v, [0.0; 3]).unwrap().unwrap();
    bed::load(&mut r, &m, &v);
    (v, r, m)
}

fn bed_at(r: &WaterRegion, i: usize, k: usize) -> f64 {
    let (t, c) = r.locate(i, k);
    r.tiles[t].bed[c]
}

/// The change rule over every chunk, as `physics.water` runs it after an edit.
fn rebed(r: &mut WaterRegion, m: &bed::Map, v: &Voxels) -> Vec<bed::Displaced> {
    let beds = bed::rebeds(r, m, v, &bed::edited_since(v, 0));
    bed::apply(r, &beds)
}

#[test]
fn beds_load_from_the_volume_at_two_columns_a_block() {
    let (v, r, m) = ground();
    assert_eq!(bed_at(&r, 0, 0), 5.0);
    assert_eq!(bed_at(&r, 16, 16), 2.0, "the pit's floor");
    assert_eq!(bed_at(&r, 17, 17), 2.0, "all the columns over one block");
    assert_eq!(bed_at(&r, 15, 16), 5.0);
    assert!(r.tiles.iter().all(|t| t.depth.iter().all(|d| *d == 0.0)));
    r.validate().unwrap();
    // The change rule leaves beds it just read alone.
    assert!(bed::rebeds(&r, &m, &v, &bed::edited_since(&v, 0)).is_empty());
}

#[test]
fn a_misaligned_region_is_refused() {
    let (v, mut r, _) = ground();
    assert!(bed::map(&r, [3.0, 0.0], &v, [0.0; 3]).unwrap().is_ok());
    let e = bed::map(&r, [0.5, 0.0], &v, [0.0; 3]).unwrap().unwrap_err();
    assert_eq!(e.code, "water.misaligned");
    r.cell_m = 1.0;
    let e = bed::map(&r, [0.0, 0.0], &v, [0.0; 3]).unwrap().unwrap_err();
    assert_eq!(e.code, "water.misaligned");
}

#[test]
fn blocks_placed_in_the_water_or_on_the_ground_raise_the_bed() {
    let (mut v, mut r, m) = ground();
    // The pit holds water 2 m deep (surface at 4 m).
    for k in 16..32 {
        for i in 16..32 {
            set(&mut r, i, k, 2.0, 2.0);
        }
    }
    // A block in the water at y 2 (2 to 3 m) takes a metre of each of its four columns.
    v.set([9, 2, 9], STONE, 1).unwrap();
    let d = rebed(&mut r, &m, &v);
    assert_eq!(bed_at(&r, 18, 18), 3.0);
    assert_eq!(depth(&r, 18, 18), 1.0);
    assert_eq!(d.len(), 4);
    assert!(d.iter().all(|d| d.m3 == 0.25), "{d:?}");
    // A block on dry ground raises the bed under it.
    v.set([2, 5, 2], STONE, 2).unwrap();
    rebed(&mut r, &m, &v);
    assert_eq!(bed_at(&r, 4, 4), 6.0);
    // A tile whose bed changed wakes.
    for t in &mut r.tiles {
        t.still = SLEEP_TICKS;
    }
    v.set([2, 6, 2], STONE, 3).unwrap();
    rebed(&mut r, &m, &v);
    let (t, _) = r.locate(4, 4);
    assert_eq!(r.tiles[t].still, 0);
    assert_eq!(r.tiles.iter().filter(|t| t.asleep()).count(), 15);
}

#[test]
fn bridges_roofs_and_leaves_over_water_or_ground_leave_the_bed() {
    let (mut v, mut r, m) = ground();
    for k in 16..32 {
        for i in 16..32 {
            set(&mut r, i, k, 2.0, 2.0);
        }
    }
    let before = r.clone();
    // A bridge over the water at y 6, a roof over dry ground at y 9, leaves over the water.
    v.fill([8, 6, 10], [15, 6, 10], STONE, 1).unwrap();
    v.fill([0, 9, 0], [3, 9, 3], STONE, 1).unwrap();
    v.set([12, 5, 12], LEAVES, 1).unwrap();
    assert!(rebed(&mut r, &m, &v).is_empty());
    assert_eq!(r, before);
    // A tunnel dug under the lake leaves it too.
    v.fill([10, 0, 10], [10, 0, 14], 0, 2).unwrap();
    rebed(&mut r, &m, &v);
    assert_eq!(r, before);
}

#[test]
fn a_broken_top_block_lowers_the_bed_and_a_dug_hole_fills() {
    let (mut v, mut r, m) = ground();
    // Dry ground: breaking the top block lowers the bed by a block.
    v.set([2, 4, 2], 0, 1).unwrap();
    rebed(&mut r, &m, &v);
    assert_eq!(bed_at(&r, 4, 4), 4.0);
    assert_eq!(bed_at(&r, 6, 4), 5.0);

    // A lake in the pit and a hole dug beside it, through its bank: the water runs into it.
    let (mut v, mut r, m) = ground();
    for k in 16..32 {
        for i in 16..32 {
            set(&mut r, i, k, 2.0, 2.5);
        }
    }
    v.fill([16, 2, 10], [19, 4, 12], 0, 1).unwrap();
    rebed(&mut r, &m, &v);
    assert_eq!(bed_at(&r, 34, 22), 2.0);
    let v0 = r.volume();
    run(&mut r, 60 * 20);
    assert!(
        depth(&r, 36, 22) > 1.0,
        "the hole filled: {}",
        depth(&r, 36, 22)
    );
    assert!(((r.volume() - v0) / v0).abs() < 1e-9);
}

// --- Field liquids (water.md 6) -------------------------------------------------------------------

use amoris_sim::voxels::{BlockShape, FALLING, Flow, FlowModel};

const WATER: u8 = 3;

/// `ground()` with a field water in its palette.
fn wet_ground() -> (Voxels, WaterRegion, bed::Map) {
    let (mut v, r, m) = ground();
    v.palette.push(Block {
        name: "water".into(),
        shape: BlockShape::Liquid,
        faces: ["w".into(), String::new(), String::new()],
        flow: Some(Flow {
            spread: 0,
            model: FlowModel::Field,
            ..Flow::default()
        }),
        ..Block::default()
    });
    v.validate().unwrap();
    (v, r, m)
}

#[test]
fn a_field_liquid_needs_no_spread() {
    let (mut v, _, _) = wet_ground();
    v.palette[3].flow.as_mut().unwrap().spread = 7;
    assert_eq!(v.validate().unwrap_err().code, "voxels.invalid");
}

#[test]
fn field_cells_give_their_part_and_beds_load_under_them() {
    let (mut v, mut r, _) = wet_ground();
    // A pond in the pit: two cells deep (y 2 and 3), a source on top; a leaf over it at y 7.
    v.fill([8, 2, 8], [15, 3, 15], WATER, 1).unwrap();
    v.set([12, 7, 12], LEAVES, 1).unwrap();
    v.set_with_state([9, 3, 9], WATER, 3, 1).unwrap();
    v.set_with_state([10, 3, 10], WATER, FALLING, 1).unwrap();
    let cells = field::cells(&v, 0);
    assert_eq!(cells.len(), 8 * 8 * 2);
    let part = |c: [i32; 3]| cells.iter().find(|(x, _)| *x == c).unwrap().1;
    assert_eq!(
        part([12, 2, 12]),
        1.0,
        "under the same liquid: the whole cell"
    );
    assert_eq!(part([12, 3, 12]), 8.0 / 9.0, "a source");
    assert_eq!(part([9, 3, 9]), 5.0 / 9.0, "level 3");
    assert_eq!(part([10, 3, 10]), 1.0, "falling");
    // Beds at load: the first solid below the highest liquid cell, not the leaf above it.
    r.tiles.clear();
    let m = bed::map(&r, [0.0, 0.0], &v, [0.0; 3]).unwrap().unwrap();
    bed::load(&mut r, &m, &v);
    assert_eq!(bed_at(&r, 24, 24), 2.0);
    // Poured, a block's water is shared over its four columns: each rises by the cell's water.
    let at = field::covers(&r, &m, 12, 12).unwrap();
    assert_eq!(at, (24, 24));
    field::pour(&mut r, &m, at, part([12, 3, 12]));
    field::pour(&mut r, &m, at, part([12, 2, 12]));
    for (i, k) in [(24, 24), (25, 24), (24, 25), (25, 25)] {
        assert!((depth(&r, i, k) - (1.0 + 8.0 / 9.0)).abs() < 1e-15);
    }
    assert!(
        (r.volume() - (1.0 + 8.0 / 9.0)).abs() < 1e-12,
        "a block's water: one cell's"
    );
    assert_eq!(field::covers(&r, &m, 40, 0), None);
}

#[test]
fn a_disturbed_deep_basin_sleeps_again_within_two_minutes() {
    // A basin 64 x 16 m, 10 m deep, its first quarter half a metre higher: released, it sloshes
    // from end to end. Friction alone (0.002 a second at this depth) would keep it moving for many
    // minutes; calm damping lets every tile sleep again within 2 minutes (water.md 4).
    let mut r = region(8, 2, 0.5);
    let [nx, nz] = r.columns_along();
    for k in 0..nz {
        for i in 0..nx {
            set(&mut r, i, k, 0.0, if i < nx / 4 { 10.5 } else { 10.0 });
        }
    }
    let v0 = r.volume();
    let mut g = Grid::default();
    let mut slept = None;
    for t in 1..=60 * 120 {
        step_region(&mut r, DT, &mut g);
        if r.tiles.iter().all(WaterTile::asleep) {
            slept = Some(t);
            break;
        }
    }
    let t = slept.expect("asleep within 2 minutes");
    assert!(t > 60 * 10, "it sloshed for a while first: {t} ticks");
    assert!(((r.volume() - v0) / v0).abs() < 1e-12);
    let (lo, hi) = (0..nx).fold((f64::MAX, f64::MIN), |(a, b), i| {
        let s = surface(&r, i, nz / 2);
        (math::min(a, s), math::max(b, s))
    });
    assert!(hi - lo < 0.01, "level within a centimetre: {lo} to {hi}");
}

#[test]
fn a_reused_window_after_reshaping_matches_a_fresh_solver_bit_for_bit() {
    let mut reused = Grid::default();
    for (tx, tz) in [(4, 2), (2, 4), (1, 1), (4, 1), (2, 2)] {
        let mut r = region(tx, tz, 0.5);
        let [nx, nz] = r.columns_along();
        for k in 0..nz {
            for i in 0..nx {
                set(&mut r, i, k, 0.0, if i < nx / 2 { 1.0 } else { 0.0 });
            }
        }
        // Add sleeping columns so the reused active mask also has to be completely overwritten.
        if r.tiles.len() > 2 {
            r.tiles.last_mut().unwrap().still = SLEEP_TICKS;
        }
        let mut expected = r.clone();
        for _ in 0..8 {
            step_region(&mut r, DT, &mut reused);
            step_region(&mut expected, DT, &mut Grid::default());
            assert_eq!(r, expected, "window {nx} x {nz}");
        }
    }
}
