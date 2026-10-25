//! Terrain and water declared by their features (water.md 65): before any tick, as the paused edit
//! world holds them, the feed carries the terrain's heights and each resident region shown from the
//! terrain's beds and its pours; an edit of the terrain sends both again.
mod common;

use std::sync::Arc;

use amoris_assets::Feed;
use amoris_assets::frame::WaterUpdate;
use amoris_link::Source;
use amoris_runtime::{Command, Extractor, Game, Project};
use serde_json::json;

/// Columns of a previewed region holding water.
fn wet(u: &WaterUpdate) -> usize {
    u.tiles
        .iter()
        .flat_map(|t| &t.columns)
        .filter(|c| c[1] > 0.0)
        .count()
}

#[test]
fn the_edit_world_shows_the_terrain_and_its_poured_water_before_a_tick() {
    common::big_stack(|| {
        let setup = Project::load(&common::repo().join("samples/water-valley"))
            .unwrap_or_else(|e| panic!("{e:#?}"))
            .setup(false)
            .unwrap_or_else(|e| panic!("{e:#?}"));
        let mut g = Game::new(Arc::new(setup), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        let feed = Feed::new();
        let view = feed.subscribe();
        let mut x = Extractor::new();
        g.present(&mut x, &feed);
        let f = view.take();
        assert_eq!(f.terrain.len(), 1);
        assert_eq!(f.terrain[0].heights.len(), 385 * 193);
        assert_eq!(f.terrain[0].layers.len(), 3);
        // Both regions, previewed: the lake poured to 4.2 m and the plunge pool to -2 m.
        assert!(f.resident_water.is_empty());
        let mut regions: Vec<_> = f.water.iter().collect();
        regions.sort_by_key(|u| u.id);
        assert_eq!(regions.len(), 2);
        let (lake, pool) = (wet(regions[0]), wet(regions[1]));
        eprintln!("wet columns: the lake's region {lake}, the falls' {pool}");
        assert!(lake > 4000 && pool > 1000, "{lake} {pool}");
        // The ground half a metre lower, the same pour spreads further, shown again with it.
        g.apply(&Command::new(
            Source::Editor,
            1,
            "world.edit",
            json!({"label": "lower", "ops": [{"set": {"entity": "Ground", "component": "Terrain",
                "value": {"base_m": 5.5}}}]}),
        ))
        .unwrap_or_else(|e| panic!("{e:#?}"));
        g.present(&mut x, &feed);
        let f = view.take();
        assert_eq!(f.terrain.len(), 1);
        let deeper = f
            .water
            .iter()
            .min_by_key(|u| u.id)
            .map(wet)
            .expect("the lake's preview sent again");
        assert!(deeper > lake, "{deeper} against {lake}");
    });
}

/// What the valley's terrain costs to work out (liquids.md 17): its heights, its collider, its
/// regions' beds and pours, release builds. `cargo test --release -p amoris-runtime --features
/// transpile --test terrain time_terrain -- --ignored --nocapture`.
#[test]
#[ignore]
#[allow(clippy::disallowed_methods)] // the test's own clock stands outside every tick
fn time_terrain() {
    let scene: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(common::repo().join("samples/water-valley/scene.json")).unwrap(),
    )
    .unwrap();
    let terrain: amoris_physics::Terrain =
        serde_json::from_value(scene["entities"][0]["components"]["Terrain"].clone()).unwrap();
    let mut lake: amoris_physics::WaterRegion =
        serde_json::from_value(scene["entities"][1]["components"]["WaterRegion"].clone()).unwrap();
    let ms = |f: &mut dyn FnMut()| {
        let start = std::time::Instant::now();
        for _ in 0..10 {
            f();
        }
        start.elapsed().as_secs_f64() * 100.0
    };
    let heights = ms(&mut || {
        std::hint::black_box(terrain.heights());
    });
    let collider = ms(&mut || {
        std::hint::black_box(amoris_physics::terrain::collider(&terrain));
    });
    let beds = ms(&mut || {
        lake.tiles.clear();
        amoris_physics::water::author::from_terrain(&mut lake, [0.0, 0.0], &terrain, [0.0; 3]);
    });
    eprintln!(
        "the valley's terrain ({} x {} cells): heights {heights:.2} ms, collider {collider:.2} ms; \
         a region's beds and pour (192 x 192 columns) {beds:.2} ms",
        terrain.size[0], terrain.size[1]
    );
}
