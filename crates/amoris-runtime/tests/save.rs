//! `project.save` (docs/spec/simulation.md 7.4): the world written back as a scene whose entity
//! fields name the entities' places in the file, in engine components as in project ones, so a
//! scene saved after a destroy loads with every reference on the entity it named.
#![cfg(feature = "transpile")]
mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use amoris_link::Source;
use amoris_runtime::{Command, Game, GameBuilder, Project};
use serde_json::{Value, json};

const MAIN: &str = r#"
import { component, field, game } from "amoris";

export const Tag = component("Tag", {
    version: 1, doc: "Names another entity.",
    fields: { on: field.entity("The entity it is on.") },
});

export default game({ components: [Tag], systems: [] });
"#;

/// A temporary project of one script and a scene.
fn project(name: &str, scene: &Value) -> PathBuf {
    let root = std::env::temp_dir().join(format!("amoris-save-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        format!("name = 'save-{name}'\nrate = 60\nseed = 1\n"),
    )
    .unwrap();
    std::fs::write(root.join("scripts/main.ts"), MAIN).unwrap();
    let scene = json!({"format": "amoris-scene", "version": 1, "entities": scene});
    std::fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    root
}

/// The game of the project as it is on disk.
fn game(root: &Path) -> Game {
    let p = Project::load(root).unwrap_or_else(|e| panic!("{e:#?}"));
    let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
    GameBuilder::new(setup)
        .project(root)
        .build()
        .unwrap_or_else(|e| panic!("{e:#?}"))
}

fn call(g: &mut Game, name: &str, params: Value) -> Value {
    g.apply(&Command::new(Source::Developer(9), 1, name, params))
        .unwrap_or_else(|e| panic!("{name}: {e:#?}"))
}

/// The name of the entity `Component.field` of the entity named `entity` holds, or null.
fn named(g: &mut Game, entity: &str, path: &str) -> Value {
    let r = call(g, "world.get", json!({"entity": entity, "fields": [path]}));
    match r["fields"][path].as_u64() {
        Some(id) => call(g, "world.get", json!({"entity": id}))["name"].clone(),
        None => Value::Null,
    }
}

/// Every entity field the scene writes, as `(entity, Component.field)`.
const FIELDS: &[(&str, &str)] = &[
    ("Rider", "Walker.mount"),
    ("Body", "Avatar.walker"),
    ("Body", "Avatar.target"),
    ("Body", "Attach.to"),
    ("Horse", "Tag.on"),
    ("Cam", "CameraRig.target"),
    ("Cam", "CameraRig.look_at"),
    ("Lake", "WaterRegion.bed.Voxels.volume"),
    ("Gauge", "WaterProbe.region"),
    ("Spring", "WaterSource.region"),
];

#[test]
fn a_saved_scene_names_the_same_entities_after_a_destroy() {
    common::big_stack(|| {
        let scene = json!([
            {"name": "Old"},
            {"name": "Rider", "components": {"Walker": {"mount": 4}}},
            {"name": "Body", "components": {
                "Avatar": {"walker": 2, "target": 1},
                "Attach": {"to": 2, "joint": "hand"}}},
            {"name": "Horse", "components": {"Tag": {"on": 3}}},
            {"name": "Cam", "components": {"CameraRig": {"target": 2, "look_at": 4}}},
            {"name": "Bed", "components": {"Voxels": {}}},
            {"name": "Lake", "components": {"WaterRegion": {"cell_m": 1, "size": [16, 16],
                "bed": {"Voxels": {"volume": 6, "per": 1, "y_lo": 0, "y_hi": 0}}}}},
            {"name": "Gauge", "components": {"WaterProbe": {"region": 7, "at": [2.0, 2.0]}}},
            {"name": "Spring", "components": {
                "WaterSource": {"region": 7, "at": [1.0, 1.0], "rate_m3s": 0.1}}},
        ]);
        let root = project("renumber", &scene);
        let mut g = game(&root);
        let before: Vec<Value> = FIELDS.iter().map(|(e, f)| named(&mut g, e, f)).collect();
        assert_eq!(
            before,
            [
                json!("Horse"),
                json!("Rider"),
                json!("Old"),
                json!("Rider"),
                json!("Body"),
                json!("Rider"),
                json!("Horse"),
                json!("Bed"),
                json!("Lake"),
                json!("Lake")
            ]
        );
        call(
            &mut g,
            "world.edit",
            json!({"ops": [{"destroy": {"entity": "Old"}}]}),
        );
        let saved = call(&mut g, "project.save", json!({}));
        assert_eq!(saved["entities"], json!(8));

        // The file: every reference is the place of its entity, and the one to Old is null.
        let text = std::fs::read_to_string(root.join("scene.json")).unwrap();
        let file: Value = serde_json::from_str(&text).unwrap();
        let comp = |i: usize, c: &str| file["entities"][i]["components"][c].clone();
        assert_eq!(file["entities"][0]["name"], json!("Rider"));
        assert_eq!(comp(0, "Walker")["mount"], json!(3));
        assert_eq!(comp(1, "Avatar")["walker"], json!(1));
        assert_eq!(comp(1, "Avatar")["target"], Value::Null);
        assert_eq!(comp(1, "Attach")["to"], json!(1));
        assert_eq!(comp(2, "Tag")["on"], json!(2));
        assert_eq!(comp(3, "CameraRig")["target"], json!(1));
        assert_eq!(comp(3, "CameraRig")["look_at"], json!(3));
        // Inside a tagged variant, and the optional regions of a probe and a source.
        assert_eq!(comp(5, "WaterRegion")["bed"]["Voxels"]["volume"], json!(5));
        assert_eq!(comp(6, "WaterProbe")["region"], json!(6));
        assert_eq!(comp(7, "WaterSource")["region"], json!(6));
        // A number that is no entity stays: the walker's hotbar slot shares no schema with ids.
        assert_eq!(comp(0, "Walker")["slot"], json!(0));

        // The saved scene loads (the region's volume is checked against the world as it does),
        // and each field names the entity it named, by name.
        let mut again = game(&root);
        let after: Vec<Value> = FIELDS
            .iter()
            .map(|(e, f)| named(&mut again, e, f))
            .collect();
        let mut want = before;
        want[2] = Value::Null;
        assert_eq!(after, want);
        // Saving what was loaded writes the same file.
        call(&mut again, "project.save", json!({}));
        assert_eq!(
            std::fs::read_to_string(root.join("scene.json")).unwrap(),
            text
        );
        let _ = std::fs::remove_dir_all(&root);
    });
}

/// The engine components whose JSON Schema holds an entity id, so that one added later is looked
/// at: `project.save` renumbers each place `values::entity_refs` finds in them, which a new shape
/// (ids as a map's keys, say) could escape. `Controls` and `ObserverEvents` are in the registry
/// but not among the components a scene names, so no scene holds them.
#[test]
fn the_engine_components_that_hold_entity_ids() {
    common::big_stack(|| {
        let root = project("pinned", &json!([]));
        let mut g = game(&root);
        let schemas = call(&mut g, "world.schema", json!({}));
        let holding: Vec<&str> = schemas
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["origin"] == json!("Engine"))
            .filter(|c| c["schema"].to_string().contains("/EntityId\""))
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            holding,
            [
                "Attach",
                "Avatar",
                "Board",
                "CameraRig",
                "Controls",
                "ObserverEvents",
                "Walker",
                "WaterDrain",
                "WaterProbe",
                "WaterRegion",
                "WaterSource",
            ]
        );
        let _ = std::fs::remove_dir_all(&root);
    });
}
