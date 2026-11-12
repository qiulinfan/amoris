//! Morph targets in the game (docs/spec/animation.md 5a), on `tests/fixtures/scenes/morph`: the
//! graph's weights reach `MorphWeights` and the render feed; a script's override holds a target
//! until it is released, and the script reads what the last evaluation gave; a restore runs on to
//! the same weights.
#![cfg(feature = "transpile")]
mod common;

use std::sync::Arc;

use amoris_assets::Feed;
use amoris_assets::anim::MorphWeights;
use amoris_link::Source;
use amoris_runtime::{Command, Extractor, Game, Project};
use serde_json::{Value, json};

fn get(g: &mut Game, entity: &str, component: &str) -> Value {
    let r = g
        .apply(&Command::new(
            Source::Developer(0),
            1,
            "world_get",
            json!({"entity": entity, "components": [component]}),
        ))
        .unwrap_or_else(|e| panic!("{e:#?}"));
    r["components"][component].clone()
}

/// The figure's (entity 1) `MorphWeights` as JSON, or null (scripts reach it only through
/// `ctx.anim`, so `world_get` does not show it).
fn morph(g: &Game) -> Value {
    let w = g.world();
    amoris_sim::entity::entity(w, amoris_sim::EntityId::new(1).unwrap())
        .and_then(|e| w.get::<MorphWeights>(e))
        .map_or(Value::Null, |m| serde_json::to_value(m).unwrap())
}

/// The weight `name` holds in a `MorphWeights` value.
fn weight(m: &Value, name: &str) -> f64 {
    m["weights"]
        .as_array()
        .and_then(|w| w.iter().find(|x| x["name"] == json!(name)))
        .and_then(|x| x["weight"].as_f64())
        .unwrap_or_else(|| panic!("no weight {name} in {m}"))
}

#[test]
fn weights_reach_the_world_the_feed_and_scripts_and_survive_a_restore() {
    common::big_stack(|| {
        let root = common::repo().join("tests/fixtures/scenes/morph");
        let setup = Project::load(&root)
            .and_then(|p| p.setup(false))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        let setup = Arc::new(setup);
        let mut g = Game::new(setup.clone(), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        let feed = Feed::new();
        let viewport = feed.subscribe();
        let mut x = Extractor::new();
        let mut drawn: Vec<f32> = Vec::new();
        let mut snap = None;
        let mut seen = Vec::new();
        for _ in 0..200 {
            let r = g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            let tick = r.tick.0;
            g.present(&mut x, &feed);
            let f = viewport.take();
            if let Some(a) = f
                .instances
                .iter()
                .find(|u| u.id == 1)
                .and_then(|u| u.anim.clone())
            {
                drawn = a.morph;
            }
            let m = morph(&g);
            if m.is_null() {
                assert!(tick == 0, "no MorphWeights after tick {tick}");
                continue;
            }
            assert_eq!(drawn.len(), 3, "the plate's two weights and the body's one");
            let names: Vec<&str> = m["weights"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| w["name"].as_str().unwrap())
                .collect();
            assert_eq!(names, ["right", "lift"]);
            let lift = weight(&m, "lift");
            // The world and the feed hold the same weights.
            assert_eq!(lift, f64::from(drawn[1]), "tick {tick}");
            assert_eq!(weight(&m, "right"), f64::from(drawn[0]), "tick {tick}");
            // The override holds "lift" from the tick the script set it until it released it.
            let held = (40..130).contains(&tick);
            assert_eq!(lift == 0.75, held, "tick {tick}: lift {lift}");
            assert_eq!(m["overrides"].as_array().unwrap().len(), usize::from(held));
            let gauge = get(&mut g, "Gauge", "Gauge");
            if tick > 40 && tick <= 130 {
                assert_eq!(gauge["lift"], json!(0.75), "tick {tick}: {gauge}");
            }
            if tick == 60 {
                snap = Some(g.snapshot().unwrap_or_else(|e| panic!("{e:#?}")));
            }
            if tick >= 60 {
                seen.push((m, gauge));
            }
        }
        assert_eq!(get(&mut g, "Gauge", "Gauge")["missing"], json!(1));
        // A restore runs on to the same weights and readings.
        g.restore(snap.as_ref().unwrap())
            .unwrap_or_else(|e| panic!("{e:#?}"));
        let mut again = vec![(morph(&g), get(&mut g, "Gauge", "Gauge"))];
        let last = seen.len() - 1;
        for _ in 0..last {
            g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            again.push((morph(&g), get(&mut g, "Gauge", "Gauge")));
        }
        assert_eq!(seen.len(), again.len());
        for (i, (a, b)) in seen.iter().zip(&again).enumerate() {
            assert_eq!(a, b, "{} ticks after the restore", i);
        }
    });
}
