//! The engine's water end to end (docs/spec/water.md): a region over a block volume takes its beds
//! at load and keeps them through a snapshot, a misaligned region is refused at load, and the flow
//! continues the same through a fork and a restore.
#![cfg(feature = "transpile")]
mod common;

use std::path::PathBuf;
use std::sync::Arc;

use amoris_physics::WaterRegion;
use amoris_runtime::{Game, Project};
use amoris_sim::voxels::{Block, Voxels};
use serde_json::{Value, json};

const MAIN: &str = r#"
import { game } from "amoris";
export default game({ components: [], systems: [] });
"#;

/// A volume of 1 m cells: stone up to y 4 under x and z 0..32, with a pit 3 blocks deep at
/// x 8..16, z 8..16.
pub fn ground() -> Voxels {
    let mut v = Voxels::default();
    v.palette.push(Block {
        name: "stone".into(),
        solid: true,
        faces: ["b#s".into(), "b#s".into(), "b#s".into()],
        ..Block::default()
    });
    v.fill([0, 0, 0], [31, 4, 31], 1, 0).unwrap();
    v.fill([8, 2, 8], [15, 4, 15], 0, 0).unwrap();
    v
}

/// A project named `name` with `main` as its script and these scene entities.
pub fn project(name: &str, main: &str, entities: Value) -> PathBuf {
    let root = std::env::temp_dir().join(format!("amoris-water-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        format!("name = '{name}'\nrate = 60\nseed = 1\n"),
    )
    .unwrap();
    std::fs::write(root.join("scripts/main.ts"), main).unwrap();
    let scene = json!({"format": "amoris-scene", "version": 1, "entities": entities});
    std::fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    root
}

fn game(root: &std::path::Path) -> Result<Game, amoris_contract::Problem> {
    let p = Project::load(root)?;
    let setup = Arc::new(p.setup(false)?);
    Game::new(setup, 1)
}

fn region(g: &Game) -> WaterRegion {
    let mut q = g.world().try_query::<&WaterRegion>().unwrap();
    q.single(g.world()).unwrap().clone()
}

fn lake(corner: [f64; 3]) -> Value {
    json!([
        {"name": "Land", "components": {"Transform": {"position": [0, 0, 0]}, "Voxels": ground()}},
        {"name": "Lake", "components": {
            "Transform": {"position": corner},
            "WaterRegion": {"cell_m": 0.5, "size": [64, 64],
                "bed": {"Voxels": {"volume": 1, "per": 2, "y_lo": 0, "y_hi": 15}}}}}
    ])
}

#[test]
fn a_region_takes_its_beds_from_the_volume_and_keeps_them() {
    common::big_stack(|| {
        let root = project("beds", MAIN, lake([0.0, 0.0, 0.0]));
        let mut g = game(&root).unwrap_or_else(|e| panic!("{e:#?}"));
        assert!(region(&g).tiles.is_empty(), "no tiles until the first tick");
        g.step().unwrap();
        let r = region(&g);
        assert_eq!(r.tiles.len(), 16);
        let (t, c) = r.locate(16, 16);
        assert_eq!(r.tiles[t].bed[c], 2.0);
        let (t, c) = r.locate(0, 0);
        assert_eq!(r.tiles[t].bed[c], 5.0);
        // Beds are persisted: a restore brings back the same region.
        let snap = g.snapshot().unwrap();
        let hash = g.world_hash().unwrap();
        g.step().unwrap();
        g.restore(&snap).unwrap();
        assert_eq!(region(&g), r);
        assert_eq!(g.world_hash().unwrap(), hash);
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn a_misaligned_region_is_refused_at_load() {
    common::big_stack(|| {
        let root = project("misaligned", MAIN, lake([0.25, 0.0, 0.0]));
        let e = game(&root).err().expect("refused");
        assert_eq!(e.code, "water.misaligned", "{e:#?}");
        std::fs::remove_dir_all(root).unwrap();
    });
}

/// `ground()` with a field water in its palette and a pond of it in the pit, two cells deep.
fn pond() -> Voxels {
    let mut v = ground();
    v.palette.push(Block {
        name: "water".into(),
        shape: amoris_sim::voxels::BlockShape::Liquid,
        faces: ["b#w".into(), String::new(), String::new()],
        flow: Some(amoris_sim::voxels::Flow {
            spread: 0,
            model: amoris_sim::voxels::FlowModel::Field,
            ..Default::default()
        }),
        ..Block::default()
    });
    v.fill([8, 2, 8], [15, 3, 15], 2, 0).unwrap();
    v
}

fn volume(g: &Game) -> Voxels {
    let mut q = g.world().try_query::<&Voxels>().unwrap();
    q.single(g.world()).unwrap().clone()
}

#[test]
fn field_liquids_load_as_water_shared_over_their_blocks() {
    common::big_stack(|| {
        let mut ents = lake([0.0, 0.0, 0.0]);
        ents[0]["components"]["Voxels"] = serde_json::to_value(pond()).unwrap();
        let root = project("field", MAIN, ents);
        let mut g = game(&root).unwrap_or_else(|e| panic!("{e:#?}"));
        g.step().unwrap();
        let r = region(&g);
        let (t, c) = r.locate(20, 20);
        assert_eq!(r.tiles[t].bed[c], 2.0);
        assert!((r.tiles[t].depth[c] - (1.0 + 8.0 / 9.0)).abs() < 1e-12);
        assert!((r.volume() - 64.0 * (1.0 + 8.0 / 9.0)).abs() < 1e-9);
        let v = volume(&g);
        assert_eq!(v.get([12, 3, 12]), 0, "the cells are cleared");
        assert_eq!(v.get([12, 2, 12]), 0);
        // A still pond stays where it is and falls asleep.
        for _ in 0..70 {
            g.step().unwrap();
        }
        let r2 = region(&g);
        assert!(r2.tiles.iter().all(|t| t.asleep()));
        assert!((r2.volume() - r.volume()).abs() < 1e-9);
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn a_field_liquid_outside_every_region_is_refused_at_load() {
    common::big_stack(|| {
        let mut ents = lake([0.0, 0.0, 0.0]);
        let mut v = pond();
        v.set([40, 6, 3], 2, 0).unwrap();
        ents[0]["components"]["Voxels"] = serde_json::to_value(v).unwrap();
        let root = project("uncovered", MAIN, ents);
        let e = game(&root).err().expect("refused");
        assert_eq!(e.code, "water.uncovered", "{e:#?}");
        std::fs::remove_dir_all(root).unwrap();
    });
}
