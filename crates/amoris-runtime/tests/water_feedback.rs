#![cfg(feature = "transpile")]
mod common;
use amoris_link::Source;
use amoris_runtime::{Command, GameBuilder, Project};
use serde_json::json;
use std::sync::Arc;

#[test]
fn feedback_authoring_omits_runtime_fields_and_rejects_forged_inputs() {
    common::big_stack(|| {
        let mut setup = Project::load(&common::repo().join("samples/water-feedback"))
            .unwrap()
            .setup(false)
            .unwrap();
        let out = common::repo().join(format!(
            "out/water-feedback/authoring-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&out).unwrap();
        let mut game = GameBuilder::new(Arc::new(setup.clone()))
            .project(&out)
            .build()
            .unwrap();
        let state = game
            .apply(&Command::new(
                Source::Developer(0),
                1,
                "world.get",
                json!({"entity":"Lake","fields":["WaterFeedback"]}),
            ))
            .unwrap();
        assert!(state["fields"]["WaterFeedback"]["pending"].is_array());
        let error=game.apply(&Command::new(Source::Developer(0),2,"world.edit",json!({"label":"forge budget","ops":[
        {"set":{"entity":"Lake","component":"WaterFeedback","value":{"applied_total":[100,100]}}}
    ]}))).unwrap_err();
        assert_eq!(error.code, "water.feedback_readonly");
        game.apply(&Command::new(
            Source::Developer(0),
            3,
            "project.save",
            json!({}),
        ))
        .unwrap();
        let scene: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("scene.json")).unwrap()).unwrap();
        assert_eq!(
            scene["entities"][1]["components"]["WaterFeedback"],
            json!({})
        );
        setup.scene = serde_json::from_value(scene).unwrap();
        GameBuilder::new(Arc::new(setup)).build().unwrap();
    });
}

#[test]
fn feedback_refuses_nonresident_regions_instead_of_reading_old_tiles() {
    common::big_stack(|| {
        let mut setup = Project::load(&common::repo().join("samples/water-feedback"))
            .unwrap()
            .setup(false)
            .unwrap();
        setup.scene.entities[1].components["WaterRegion"]["solver"] = json!("cpu");
        let mut game = GameBuilder::new(Arc::new(setup)).build().unwrap();
        assert_eq!(
            game.step().unwrap_err().code,
            "water.feedback_requires_resident"
        );
    });
}
