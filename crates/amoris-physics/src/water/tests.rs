use super::solve::{Grid, substeps_for};
use super::*;

const DT: f64 = 1.0 / 60.0;

/// A region of `tx` by `tz` tiles of `cell_m` columns over a flat bed at 0, dry.
pub(crate) fn region(tx: u32, tz: u32, cell_m: f64) -> WaterRegion {
    let mut r = WaterRegion {
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

/// FNV-1a over every tile's bits, in tile and column order.
pub(crate) fn hash(r: &WaterRegion) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |x: u64| {
        for b in x.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for t in &r.tiles {
        for v in [&t.bed, &t.depth, &t.u, &t.w] {
            for x in v.iter() {
                eat(x.to_bits());
            }
        }
        eat(u64::from(t.still));
    }
    h
}

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

    // A 1 cm bump wakes nothing by itself (it is not a source or an edit); woken, it rings out
    // and the lake sleeps again with its velocities zeroed.
    set(&mut r, 5, 5, 0.0, 0.11);
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
            "bed",
            Box::new(|r| {
                r.bed = BedSource::Colliders {
                    top: 10.0,
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
