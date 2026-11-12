//! The game UI in the world (docs/spec/ui.md): the render feed carries every element of the UI
//! scene with its stack and parent, an invisible stack hides what is under it; `ui.press` is
//! refused for what is no enabled, visible button and changes nothing then, and otherwise counts the
//! press, flips a toggle and reaches the rule as `ui.pressed` in the next tick; a seat of a game
//! with players lists the screen and presses through the same write, and the run's recording
//! replays identical.
#![cfg(feature = "transpile")]
mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use amoris_assets::frame::{UiKind, UiView};
use amoris_link::Source;
use amoris_persist::replay::{
    MemorySink, Record, RecordOptions, Replay, ReplayMode, ReplayOutcome, VerifyOptions, verify,
};
use amoris_runtime::{Command, Extractor, Game, Project};
use serde_json::{Value, json};

fn fixture() -> PathBuf {
    common::repo().join("tests/fixtures/scenes/ui-hud")
}

fn game(root: &Path) -> Game {
    let p = Project::load(root).unwrap_or_else(|e| panic!("{e:#?}"));
    let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
    Game::new(setup, 1).unwrap_or_else(|e| panic!("{e:#?}"))
}

fn call(
    g: &mut Game,
    source: Source,
    name: &str,
    params: Value,
) -> Result<Value, amoris_contract::Problem> {
    g.apply(&Command::new(source, 1, name, params))
}

fn get(g: &mut Game, entity: &str, fields: &[&str]) -> Value {
    call(
        g,
        Source::Developer(9),
        "world.get",
        json!({"entity": entity, "fields": fields}),
    )
    .unwrap_or_else(|e| panic!("{e:#?}"))["fields"]
        .clone()
}

fn id(g: &mut Game, name: &str) -> u64 {
    call(
        g,
        Source::Developer(9),
        "world.get",
        json!({"entity": name, "components": []}),
    )
    .unwrap_or_else(|e| panic!("{e:#?}"))["id"]
        .as_u64()
        .unwrap_or_else(|| panic!("{name} has an id"))
}

fn ui_of(
    g: &mut Game,
    x: &mut Extractor,
    feed: &amoris_assets::Feed,
    mb: &amoris_assets::Mailbox,
) -> Vec<UiView> {
    g.present(x, feed);
    mb.take().ui.unwrap_or_default()
}

#[test]
fn the_feed_carries_every_element_and_a_hidden_stack_hides_its_children() {
    common::big_stack(|| {
        let mut g = game(&fixture());
        let feed = amoris_assets::Feed::new();
        let mb = feed.subscribe();
        let mut x = Extractor::new();
        let ui = ui_of(&mut g, &mut x, &feed, &mb);
        let menu = id(&mut g, "Menu");
        let sound = id(&mut g, "Sound");
        let crate_ = id(&mut g, "Crate");
        let kinds =
            |id: u64| -> Vec<UiKind> { ui.iter().filter(|u| u.id == id).map(|u| u.kind).collect() };
        assert_eq!(kinds(menu), vec![UiKind::Stack]);
        assert_eq!(
            kinds(sound),
            vec![UiKind::Button, UiKind::Text],
            "button, then its label"
        );
        assert_eq!(kinds(crate_), vec![UiKind::Image]);
        let stack = ui.iter().find(|u| u.id == menu).unwrap();
        assert!(
            stack.column && stack.reference_height == 720.0 && stack.anchor == 3,
            "{stack:?}"
        );
        let button = ui
            .iter()
            .find(|u| u.id == sound && u.kind == UiKind::Button)
            .unwrap();
        assert_eq!(button.parent, menu);
        assert!(button.enabled && !button.on);
        let title = ui.iter().find(|u| u.text.starts_with("按钮")).unwrap();
        assert_eq!(title.asset, "fonts/DroidSansFallback.ttf");
        let emblem = ui
            .iter()
            .find(|u| u.kind == UiKind::Image && u.anchor == 0)
            .unwrap();
        assert_eq!(
            (emblem.asset.as_str(), emblem.size),
            ("images/emblem.png", [96.0, 96.0])
        );
        let crate_image = ui.iter().find(|u| u.id == crate_).unwrap();
        assert_eq!(crate_image.anchor, 9, "over the entity");
        assert!((crate_image.world[1] - 1.5).abs() < 1e-6, "{crate_image:?}");
        // Sorted by entity.
        assert!(ui.windows(2).all(|w| w[0].id <= w[1].id));

        // The menu hidden: neither it nor anything under it is drawn, nor pressed.
        call(
            &mut g,
            Source::Editor,
            "world.edit",
            json!({"ops": [{"set": {"entity": "Menu", "component": "UiStack", "value": {"visible": false}}}]}),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        let hidden = ui_of(&mut g, &mut x, &feed, &mb);
        assert!(
            hidden.iter().all(|u| u.id != menu && u.parent != menu),
            "{hidden:#?}"
        );
        assert!(
            hidden.iter().any(|u| u.kind == UiKind::Stack),
            "the vitals row stays"
        );
        let e = call(
            &mut g,
            Source::Player(0),
            "ui.press",
            json!({"entity": "Sound"}),
        )
        .unwrap_err();
        assert_eq!(e.code, "ui.hidden", "{e:#?}");
    });
}

#[test]
fn presses_reach_the_rule_and_the_recording_replays() {
    common::big_stack(|| {
        let mut g = game(&fixture());
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        g.step().unwrap();
        let before = g.world_hash().unwrap();
        // Refusals change nothing.
        for (entity, code) in [("Quit", "ui.disabled"), ("Title", "ui.not_a_button")] {
            let e = call(
                &mut g,
                Source::Player(0),
                "ui.press",
                json!({"entity": entity}),
            )
            .unwrap_err();
            assert_eq!(e.code, code, "{entity}: {e:#?}");
        }
        let e = call(
            &mut g,
            Source::Player(0),
            "ui.press",
            json!({"entity": "Sond"}),
        )
        .unwrap_err();
        assert_eq!(e.code, "sim.entity_not_found", "{e:#?}");
        let e = call(
            &mut g,
            Source::Player(0),
            "ui.press",
            json!({"button": "Sound"}),
        )
        .unwrap_err();
        assert!(e.message.contains("entity"), "{e:#?}");
        assert_eq!(
            g.world_hash().unwrap(),
            before,
            "a refused press changes nothing"
        );

        let r = call(
            &mut g,
            Source::Player(0),
            "ui.press",
            json!({"entity": "Sound"}),
        )
        .unwrap();
        assert_eq!(
            (r["on"].clone(), r["presses"].clone()),
            (json!(true), json!(1)),
            "{r}"
        );
        // The event is the boundary's; the rule reads it in the next tick.
        assert_eq!(
            get(&mut g, "Sound", &["UiText.text"])["UiText.text"],
            "Sound: off"
        );
        g.step().unwrap();
        let s = get(
            &mut g,
            "Sound",
            &["UiText.text", "UiButton.on", "UiButton.presses"],
        );
        assert_eq!(s["UiText.text"], "Sound: on", "{s}");
        assert_eq!(
            (s["UiButton.on"].clone(), s["UiButton.presses"].clone()),
            (json!(true), json!(1))
        );
        assert_eq!(
            get(&mut g, "Status", &["UiText.text"])["UiText.text"],
            "按下 sound ×1"
        );
        // A button that is no toggle counts without turning on.
        let r = call(
            &mut g,
            Source::Developer(0),
            "ui.press",
            json!({"entity": "Start"}),
        )
        .unwrap();
        assert_eq!(
            (r["on"].clone(), r["presses"].clone()),
            (json!(false), json!(1)),
            "{r}"
        );
        call(
            &mut g,
            Source::Player(0),
            "ui.press",
            json!({"entity": "Sound"}),
        )
        .unwrap();
        for _ in 0..3 {
            g.step().unwrap();
        }
        assert_eq!(
            get(&mut g, "Sound", &["UiText.text"])["UiText.text"],
            "Sound: off"
        );
        assert_eq!(
            get(&mut g, "Status", &["UiText.text"])["UiText.text"],
            "按下 sound ×2"
        );

        g.finish_recording().unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap();
        let mut fresh = Game::for_replay(&replay).unwrap();
        let v = verify(&replay, &mut fresh, VerifyOptions::mode(ReplayMode::Verify));
        assert_eq!(v.outcome, ReplayOutcome::Identical, "{v:#?}");
        let presses: Vec<String> = replay
            .records()
            .iter()
            .filter_map(|r| match r {
                Record::Tick { writes, .. } => Some(writes),
                _ => None,
            })
            .flatten()
            .filter(|w| w.name == "ui.press")
            .map(|w| w.params.clone())
            .collect();
        let sound = id(&mut g, "Sound");
        assert_eq!(presses.len(), 3, "{presses:?}");
        assert_eq!(
            presses[0],
            format!("{{\"entity\":{sound}}}"),
            "recorded by id"
        );
    });
}

/// The UI scene with a first-person seat `player`: a game with players.
fn seated() -> PathBuf {
    let root = std::env::temp_dir().join(format!("amoris-ui-seat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    let src = fixture();
    std::fs::copy(src.join("scripts/main.ts"), root.join("scripts/main.ts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        "name = 'ui-seat'\nrate = 60\nseed = 1\n[player]\nactions = 'first_person'\n",
    )
    .unwrap();
    std::fs::write(
        root.join("perception.json"),
        json!({"observers": [{"name": "player", "doc": "A player.",
            "sight": {"range_m": 20, "fov_deg": 70, "eye_m": {"x": 0, "y": 0, "z": 0}, "occlusion": false},
            "attention_m": 10, "memory_s": 10, "memory_capacity": 8, "event_capacity": 16,
            "budget_tokens": 300, "instruments": []}],
            "instruments": [], "kinds": [], "events": []})
        .to_string(),
    )
    .unwrap();
    let mut scene: Value =
        serde_json::from_str(&std::fs::read_to_string(src.join("scene.json")).unwrap()).unwrap();
    scene["entities"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "Steve", "components": {
        "Transform": {"position": [0, 0.1, 3]}, "Walker": {},
        "Observer": {"profile": "player", "seat": "player"}}}));
    std::fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    root
}

#[test]
fn a_seat_lists_the_screen_and_presses_its_buttons() {
    common::big_stack(|| {
        let root = seated();
        let mut g = game(&root);
        assert!(g.has_players());
        g.step().unwrap();
        let list = call(&mut g, Source::Player(0), "ui.list", json!({})).unwrap();
        let elements = list["elements"].as_array().unwrap();
        let sound = elements
            .iter()
            .find(|e| e["kind"] == "button" && e["label"] == "Sound: off")
            .unwrap_or_else(|| panic!("{list:#}"));
        assert_eq!(
            (sound["toggle"].clone(), sound["on"].clone()),
            (json!(true), json!(false))
        );
        let quit = elements.iter().find(|e| e["name"] == "Quit").unwrap();
        assert_eq!(quit["enabled"], json!(false));
        assert!(
            elements
                .iter()
                .any(|e| e["kind"] == "image" && e["image"] == "images/emblem.png")
        );
        assert!(
            elements
                .iter()
                .any(|e| e["kind"] == "bar" && e["value"] == json!(72.0))
        );
        assert!(
            elements
                .iter()
                .any(|e| e["kind"] == "stack" && e["direction"] == "row")
        );
        // A seat sends the UI's commands and the player tools, nothing else.
        let e = call(
            &mut g,
            Source::Player(0),
            "world.get",
            json!({"entity": "Sound"}),
        )
        .unwrap_err();
        assert_eq!(e.code, "permission.denied", "{e:#?}");
        let r = call(
            &mut g,
            Source::Player(0),
            "ui.press",
            json!({"entity": sound["entity"]}),
        )
        .unwrap();
        assert_eq!(r["on"], json!(true));
        g.step().unwrap();
        let list = call(&mut g, Source::Player(0), "ui.list", json!({})).unwrap();
        assert!(
            list["elements"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["label"] == "Sound: on" && e["presses"] == json!(1)),
            "{list:#}"
        );
        let _ = std::fs::remove_dir_all(&root);
    });
}
