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

const SCRIPTED: &str = r#"
import { component, field, game, system } from "amoris";

export const Lake = component("Lake", { version: 1, doc: "The region.", fields: {} });
export const Land = component("Land", { version: 1, doc: "The volume.", fields: {} });
export const Probe = component("Probe", {
    version: 1, doc: "What the system saw.",
    fields: {
        taken: field.f64(-1, "What remove took at tick 2."),
        outside: field.f64(-1, "What remove took outside the region."),
        depth: field.f64(-1, "The depth read at tick 3."),
        surface: field.f64(-1, "The surface read at tick 3."),
        speed: field.f64(-1, "The speed read at tick 3."),
        far: field.f64(-1, "The depth outside the region."),
        cell: field.i32(-1, "The placed water's cell at tick 3."),
    },
});

export const pour = system({
    name: "pour", phase: "update", doc: "Adds, takes and reads water; places field water.",
    run(ctx) {
        const r = ctx.single("Lake");
        const p = ctx.single("Probe");
        const v = ctx.single("Land");
        if (ctx.tick === 2) {
            ctx.water.add(r, 10.25, 10.25, 1.0);
            const taken = ctx.water.remove(r, 10.25, 10.25, 0.25);
            const outside = ctx.water.remove(r, 100, 100, 1.0);
            ctx.world.set(p, "Probe", { taken, outside });
            const water = ctx.voxels.block(v, "water");
            ctx.voxels.set(v, 12, 2, 12, water);
            ctx.voxels.set(v, 40, 6, 3, water);
        }
        if (ctx.tick === 3) {
            const vel = ctx.water.velocity(r, 10.75, 10.25);
            ctx.world.set(p, "Probe", {
                depth: ctx.water.depth(r, 10.25, 10.25),
                surface: ctx.water.surface(r, 10.25, 10.25),
                speed: Math.abs(vel[0]) + Math.abs(vel[1]),
                far: ctx.water.depth(r, -5, 3),
                cell: ctx.voxels.get(v, 12, 2, 12),
            });
        }
    },
});

export default game({ components: [Lake, Land, Probe], systems: [pour] });
"#;

fn probe(g: &mut Game) -> Value {
    g.apply(&amoris_runtime::Command::new(
        amoris_link::Source::Developer(9),
        1,
        "world.get",
        json!({"entity": "Probe", "fields": [
            "Probe.taken", "Probe.outside", "Probe.depth", "Probe.surface", "Probe.speed",
            "Probe.far", "Probe.cell"]}),
    ))
    .unwrap_or_else(|e| panic!("{e:#?}"))["fields"]
        .clone()
}

#[test]
fn scripts_read_add_and_remove_water_and_place_field_liquids() {
    common::big_stack(|| {
        let mut ents = lake([0.0, 0.0, 0.0]);
        let mut v = pond();
        v.fill([8, 2, 8], [15, 3, 15], 0, 0).unwrap();
        ents[0]["components"]["Voxels"] = serde_json::to_value(v).unwrap();
        ents[0]["components"]["Land"] = json!({});
        ents[1]["components"]["Lake"] = json!({});
        ents.as_array_mut()
            .unwrap()
            .push(json!({"name": "Probe", "components": {"Probe": {}}}));
        let root = project("scripted", SCRIPTED, ents);
        let mut g = game(&root).unwrap_or_else(|e| panic!("{e:#?}"));
        g.step().unwrap();
        g.step().unwrap();
        // Tick 2: a cubic metre in, a quarter taken back; the placed block's water (8/9 of a cell)
        // poured, the one outside every region cleared and reported.
        let p = probe(&mut g);
        assert_eq!(p["Probe.taken"], 0.25);
        assert_eq!(p["Probe.outside"], 0.0);
        let uncovered: Vec<_> = g
            .inbox()
            .iter()
            .filter(|e| e.kind.as_str() == "water.uncovered")
            .collect();
        assert_eq!(uncovered.len(), 1);
        let r = region(&g);
        assert!(
            (r.volume() - (0.75 + 8.0 / 9.0)).abs() < 1e-9,
            "{}",
            r.volume()
        );
        assert_eq!(volume(&g).get([40, 6, 3]), 0);
        g.step().unwrap();
        let p = probe(&mut g);
        let d = p["Probe.depth"].as_f64().unwrap();
        assert!(d > 0.0 && d < 3.0, "spreading: {d}");
        assert!((p["Probe.surface"].as_f64().unwrap() - (2.0 + d)).abs() < 1e-12);
        assert!(p["Probe.speed"].as_f64().unwrap() > 0.0);
        assert_eq!(p["Probe.far"], 0.0);
        assert_eq!(p["Probe.cell"], 0, "a field liquid's cell reads as air");
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn sources_pour_and_drains_take_at_most_what_is_there() {
    common::big_stack(|| {
        let mut ents = lake([0.0, 0.0, 0.0]);
        ents[0]["components"]["Voxels"] = serde_json::to_value(pond()).unwrap();
        let list = ents.as_array_mut().unwrap();
        list.push(json!({"name": "Spring", "components": {
            "WaterSource": {"region": 2, "at": [3.25, 3.25], "rate_m3s": 0.6}}}));
        list.push(json!({"name": "Sink", "components": {
            "WaterDrain": {"region": 2, "at": [12.25, 12.25], "rate_m3s": 0.5}}}));
        list.push(json!({"name": "DrySink", "components": {
            "WaterDrain": {"region": 2, "at": [28.25, 28.25], "rate_m3s": 5.0}}}));
        let root = project("springs", MAIN, ents);
        let mut g = game(&root).unwrap_or_else(|e| panic!("{e:#?}"));
        g.step().unwrap();
        let v0 = region(&g).volume();
        for _ in 0..60 {
            g.step().unwrap();
        }
        // A second: 0.6 in, 0.5 out of the pond; the dry sink takes nothing.
        let v1 = region(&g).volume();
        assert!((v1 - v0 - 0.1).abs() < 1e-9, "{v0} then {v1}");
        std::fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn the_feed_carries_every_tile_first_and_then_the_tiles_stepped() {
    use amoris_assets::Feed;
    use amoris_runtime::Extractor;
    common::big_stack(|| {
        let mut ents = lake([0.0, 0.0, 0.0]);
        ents[0]["components"]["Voxels"] = serde_json::to_value(pond()).unwrap();
        let root = project("feed", MAIN, ents);
        let mut g = game(&root).unwrap_or_else(|e| panic!("{e:#?}"));
        let feed = Feed::new();
        let viewport = feed.subscribe();
        let mut x = Extractor::new();
        g.step().unwrap();
        g.present(&mut x, &feed);
        let first = viewport.take();
        assert!(first.reset);
        assert_eq!(first.water.len(), 1);
        let w = &first.water[0];
        assert_eq!((w.size, w.tiles.len()), ([64, 64], 16));
        assert!(w.look.is_some());
        let at = |w: &amoris_assets::frame::WaterUpdate, key: [u32; 2], c: usize| {
            w.tiles.iter().find(|t| t.key == key).unwrap().columns[c]
        };
        // Column (20, 20) is tile (1, 1)'s column (4, 4): the pond, 2 m deep over a bed at 2 m.
        let c = at(w, [1, 1], 4 + 16 * 4);
        assert!(
            (c[1] - 1.888_889).abs() < 1e-5 && (c[0] - 3.888_889).abs() < 1e-5,
            "{c:?}"
        );
        // Still water sleeps: once asleep, frames carry no tiles.
        for _ in 0..70 {
            g.step().unwrap();
        }
        g.present(&mut x, &feed);
        let _ = viewport.take();
        g.step().unwrap();
        g.present(&mut x, &feed);
        let quiet = viewport.take();
        assert!(
            quiet.water.iter().all(|w| w.tiles.is_empty()),
            "{:?}",
            quiet.water.len()
        );
        // A drop into tile (0, 0)'s corner wakes it: that tile and its neighbours are sent.
        g.apply(&amoris_runtime::Command::new(
            amoris_link::Source::Editor,
            1,
            "world.edit",
            json!({"label": "spring", "ops": [{"spawn": {"name": "Spring", "components": {
                "WaterSource": {"region": 2, "at": [0.25, 0.25], "rate_m3s": 1.0}}}}]}),
        ))
        .unwrap_or_else(|e| panic!("{e:#?}"));
        g.step().unwrap();
        g.present(&mut x, &feed);
        let woke = viewport.take();
        let keys: Vec<[u32; 2]> = woke.water[0].tiles.iter().map(|t| t.key).collect();
        assert_eq!(keys, vec![[0, 0], [1, 0], [0, 1]], "{keys:?}");
        std::fs::remove_dir_all(root).unwrap();
    });
}
