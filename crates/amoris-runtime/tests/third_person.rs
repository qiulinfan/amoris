//! The `third_person` catalog end to end (docs/spec/player.md 5.1): a seat declared by a project
//! moves through `player.act` relative to the camera, the world and its body, sprints, guards,
//! dodges, interacts and locks on (by toggle and through a kind's affordance), the game's rules see
//! each pulse once, the seat perceives its own heading, look and lock, and the recorded run
//! replays to the same world hash at every tick.
#![cfg(feature = "transpile")]
mod common;

use std::path::PathBuf;
use std::sync::Arc;

use amoris_link::Source;
use amoris_persist::replay::{MemorySink, RecordOptions, Replay};
use amoris_runtime::{Command, Game, Project};
use serde_json::{Value, json};

const MAIN: &str = r#"
import { component, field, game, system } from "amoris";

export const Tally = component("Tally", {
    version: 1,
    doc: "What the seat did, as the rules saw it.",
    fields: {
        dodges: field.u32(0, "Dodge pulses."),
        interacts: field.u32(0, "Interact pulses."),
        primaries: field.u32(0, "Primary pulses."),
        guarded: field.u32(0, "Ticks guarded."),
        opened: field.entity("The last thing interacted with by name."),
    },
});

export const count = system({
    name: "count", phase: "update", doc: "Counts the seat's verbs.",
    queries: { seats: { with: ["ThirdPerson", "Walker", "Tally"], fields: [] } },
    run(ctx, { seats }) {
        seats.each((_r, e) => {
            const t = ctx.world.get(e, "ThirdPerson");
            const w = ctx.world.get(e, "Walker");
            const k = ctx.world.get(e, "Tally");
            if (!t || !w || !k) return;
            ctx.world.set(e, "Tally", {
                dodges: k.dodges + (t.dodge ? 1 : 0),
                interacts: k.interacts + (t.interact ? 1 : 0),
                primaries: k.primaries + (w.primary ? 1 : 0),
                guarded: k.guarded + (t.guard ? 1 : 0),
                opened: t.interact_target ?? k.opened,
            });
        });
    },
});

export default game({ components: [Tally], systems: [count] });
"#;

const PERCEPTION: &str = r#"{
  "observers": [{"name": "player", "doc": "A third-person player.",
    "sight": {"range_m": 60, "fov_deg": 360, "eye_m": {"x": 0, "y": 0, "z": 0}, "occlusion": false},
    "positions": true, "attention_m": 40, "memory_s": 60, "memory_capacity": 16,
    "event_capacity": 16, "budget_tokens": 600,
    "instruments": ["position_m", "heading_deg", "look_deg", "locked"]}],
  "instruments": [
    {"name": "position_m", "doc": "The eye.", "unit": {"unit": "position"}, "precision": 2,
     "source": {"from": "field", "component": "Transform", "path": "position"}},
    {"name": "heading_deg", "doc": "Where the body faces.", "unit": {"unit": "bearing"},
     "precision": 0, "source": {"from": "derived", "function": "third_person.heading_deg"}},
    {"name": "look_deg", "doc": "Where the camera looks.", "unit": {"unit": "bearing"},
     "precision": 0, "source": {"from": "derived", "function": "third_person.look_deg"}},
    {"name": "locked", "doc": "Locked on.", "unit": {"unit": "bool"}, "precision": 0,
     "source": {"from": "derived", "function": "third_person.locked"}}
  ],
  "kinds": [
    {"kind": "foe", "doc": "Someone to fight.", "facts": [],
     "affordances": [{"verb": "lock_on", "doc": "Lock on to it.",
       "effect": {"effect": "pulse", "control": "lock_on"},
       "requires": [{"requires": "seen"}, {"requires": "within", "max_m": 20}]}]},
    {"kind": "chest", "doc": "A chest.", "facts": [],
     "affordances": [{"verb": "open", "doc": "Open it.",
       "effect": {"effect": "pulse", "control": "interact_with"},
       "requires": [{"requires": "within", "max_m": 3}]}]}
  ],
  "events": []
}"#;

fn project(tag: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("amoris-third-person-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(
        root.join("project.toml"),
        "name = 'third'\nrate = 60\nseed = 1\n[player]\nactions = 'third_person'\ncamera = 'game'\n",
    )
    .unwrap();
    std::fs::write(root.join("scripts/main.ts"), MAIN).unwrap();
    std::fs::write(root.join("perception.json"), PERCEPTION).unwrap();
    // 1 floor, 2 hero, 3 its body, 4 its camera, 5 a foe to the north-east, 6 a chest to the east.
    let scene = json!({"format": "amoris-scene", "version": 1, "entities": [
        {"name": "Floor", "components": {
            "Transform": {"position": [0, -0.5, 0]},
            "RigidBody": {"kind": "Fixed"},
            "Collider": {"shape": {"Cuboid": {"half_extents": [40.0, 0.5, 40.0]}}}}},
        {"name": "Hero", "components": {
            "Transform": {"position": [0, 1.62, 0]},
            "Walker": {"jump_mps": 0.0, "gravity_mps2": 22.0},
            "ThirdPerson": {},
            "Tally": {},
            "Observer": {"profile": "player", "seat": "player"}}},
        {"name": "Body", "components": {"Transform": {}, "Avatar": {"walker": 2, "turn_rate": 50.0}}},
        {"name": "Cam", "components": {
            "Transform": {"position": [0, 3, 4]}, "Camera": {}, "CameraRig": {"target": 2}}},
        {"name": "Foe", "components": {
            "Transform": {"position": [6, 1.62, -6]}, "Lockable": {},
            "Perceivable": {"kind": "foe", "detect_m": 60, "height_m": 1.8, "priority": 50}}},
        {"name": "Chest", "components": {
            "Transform": {"position": [-2, 0.4, 0]},
            "Perceivable": {"kind": "chest", "detect_m": 60, "height_m": 0.8, "priority": 10}}}
    ]});
    std::fs::write(root.join("scene.json"), scene.to_string()).unwrap();
    root
}

fn get(g: &mut Game, entity: &str, fields: &[&str]) -> Value {
    g.apply(&Command::new(
        Source::Developer(9),
        1,
        "world.get",
        json!({"entity": entity, "fields": fields}),
    ))
    .unwrap_or_else(|e| panic!("{e:#?}"))["fields"]
        .clone()
}

/// An instrument's value in a JSON observation.
fn instrument(o: &Value, name: &str) -> Value {
    o["instruments"]
        .as_array()
        .and_then(|a| a.iter().find(|i| i["name"] == json!(name)))
        .map_or(Value::Null, |i| i["value"].clone())
}

fn num(v: &Value, path: &str) -> f64 {
    v[path]
        .as_f64()
        .unwrap_or_else(|| panic!("{path} in {v:#}"))
}

#[test]
fn a_seat_moves_acts_and_locks_on_and_the_run_replays() {
    common::big_stack(|| {
        let root = project("play");
        let p = Project::load(&root).unwrap_or_else(|e| panic!("{e:#?}"));
        let setup = Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")));
        let mut g = Game::new(setup, 1).unwrap_or_else(|e| panic!("{e:#?}"));
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        let mut seq = 0;
        let mut call = |g: &mut Game, name: &str, params: Value| {
            seq += 1;
            g.apply(&Command::new(Source::Player(0), seq, name, params))
        };
        let steps = |g: &mut Game, n: u32| {
            for _ in 0..n {
                g.step().unwrap();
            }
        };
        let at = |g: &mut Game| {
            let v = get(g, "Hero", &["Transform.position"]);
            let p = &v["Transform.position"];
            [p[0].as_f64().unwrap(), p[2].as_f64().unwrap()]
        };
        steps(&mut g, 10);

        // The catalog as an agent reads it.
        let d = call(&mut g, "player.describe", json!({"part": "controls"}))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        let text = d.to_string();
        for c in [
            "\"frame\"",
            "\"zoom\"",
            "\"dodge\"",
            "\"lock_on\"",
            "\"interact_with\"",
        ] {
            assert!(text.contains(c), "{c} in {d:#}");
        }

        // Camera-relative: the orbit turned to the west, forward walks west.
        call(
            &mut g,
            "player.act",
            json!({"actions": [{"do": "set", "controls": {"yaw": std::f64::consts::FRAC_PI_2, "forward": 1.0}}]}),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        let a = at(&mut g);
        steps(&mut g, 30);
        let b = at(&mut g);
        assert!(
            b[0] < a[0] - 1.0 && (b[1] - a[1]).abs() < 0.01,
            "{a:?} {b:?}"
        );
        // The body turned to its motion: it faces west, a bearing of 270.
        let o = call(&mut g, "player.observe", json!({"projection": "json"}))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(instrument(&o, "heading_deg"), json!(270), "{o:#}");
        assert_eq!(instrument(&o, "look_deg"), json!(270), "{o:#}");
        assert_eq!(instrument(&o, "locked"), json!(false), "{o:#}");

        // World-relative: east, a bearing of 90, is strafe 1, whatever the camera.
        call(
            &mut g,
            "player.act",
            json!({"actions": [{"do": "set", "controls": {"frame": "world", "forward": 0.0, "strafe": 1.0, "run": true}}]}),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        steps(&mut g, 30);
        let c = at(&mut g);
        assert!(
            c[0] > b[0] + 2.0 && (c[1] - b[1]).abs() < 0.01,
            "sprints east: {b:?} {c:?}"
        );
        // Body-relative: ahead of the body, which faces east now.
        call(
            &mut g,
            "player.act",
            json!({"actions": [{"do": "set", "controls": {"frame": "body", "strafe": 0.0, "forward": 1.0, "run": false}}]}),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        steps(&mut g, 30);
        let d = at(&mut g);
        assert!(
            d[0] > c[0] + 1.0 && (d[1] - c[1]).abs() < 0.05,
            "ahead: {c:?} {d:?}"
        );
        call(
            &mut g,
            "player.act",
            json!({"actions": [{"do": "set", "controls": {"forward": 0.0, "frame": "camera"}}]}),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));

        // The verbs: each pulse reaches the rules once; the guard holds.
        call(
            &mut g,
            "player.act",
            json!({"actions": [
                {"do": "pulse", "control": "dodge"},
                {"do": "pulse", "control": "interact"},
                {"do": "pulse", "control": "primary"},
                {"do": "set", "controls": {"guard": true, "zoom": 1.5}}]}),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        steps(&mut g, 5);
        let t = get(
            &mut g,
            "Hero",
            &[
                "Tally.dodges",
                "Tally.interacts",
                "Tally.primaries",
                "Tally.guarded",
                "ThirdPerson.zoom",
            ],
        );
        assert_eq!(
            [
                num(&t, "Tally.dodges"),
                num(&t, "Tally.interacts"),
                num(&t, "Tally.primaries"),
                num(&t, "Tally.guarded")
            ],
            [1.0, 1.0, 1.0, 5.0],
            "{t:#}"
        );
        assert_eq!(num(&t, "ThirdPerson.zoom"), 1.5);

        // Lock on by toggle: the foe is the only lockable thing, and in front once the camera
        // looks north.
        call(
            &mut g,
            "player.act",
            json!({"actions": [{"do": "set", "controls": {"yaw": 0.0, "guard": false}}, {"do": "pulse", "control": "lock"}]}),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        steps(&mut g, 30);
        let l = get(&mut g, "Hero", &["ThirdPerson.lock"]);
        assert_eq!(l["ThirdPerson.lock"], json!(5), "{l:#}");
        let o = call(&mut g, "player.observe", json!({"projection": "json"}))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(instrument(&o, "locked"), json!(true), "{o:#}");
        // The camera and the body turned to the foe, north-east of the hero: a bearing near 45.
        let look = instrument(&o, "look_deg").as_f64().unwrap();
        let heading = instrument(&o, "heading_deg").as_f64().unwrap();
        assert!(
            (20.0..70.0).contains(&look) && (look - heading).abs() < 2.0,
            "{o:#}"
        );
        // Let go, then lock on through the foe's affordance, as an agent names it.
        call(
            &mut g,
            "player.act",
            json!({"actions": [{"do": "pulse", "control": "lock"}]}),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        steps(&mut g, 1);
        assert!(get(&mut g, "Hero", &["ThirdPerson.lock"])["ThirdPerson.lock"].is_null());
        let aff = call(&mut g, "player.affordances", json!({"entity": "Foe"}))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        assert!(aff.to_string().contains("lock_on"), "{aff:#}");
        call(
            &mut g,
            "player.act",
            json!({"actions": [{"do": "use", "entity": "Foe", "verb": "lock_on"}]}),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        steps(&mut g, 1);
        assert_eq!(
            get(&mut g, "Hero", &["ThirdPerson.lock"])["ThirdPerson.lock"],
            json!(5)
        );
        // The chest is out of reach; walking to it in the world frame brings it within 3 m.
        let far = call(
            &mut g,
            "player.act",
            json!({"actions": [{"do": "use", "entity": "Chest", "verb": "open"}]}),
        )
        .unwrap_err();
        assert_eq!(far.code, "action.unavailable", "{far:#?}");
        let here = at(&mut g);
        call(
            &mut g,
            "player.act",
            json!({"actions": [{"do": "pulse", "control": "lock"}, {"do": "set", "controls": {"frame": "world", "strafe": -1.0}}]}),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        let mut walked = 0;
        while at(&mut g)[0] > -0.5 && walked < 600 {
            steps(&mut g, 1);
            walked += 1;
        }
        assert!(walked < 600, "reached the chest from {here:?}");
        call(
            &mut g,
            "player.act",
            json!({"actions": [{"do": "set", "controls": {"strafe": 0.0}}, {"do": "use", "entity": "Chest", "verb": "open"}]}),
        )
        .unwrap_or_else(|e| panic!("{e:#?}"));
        steps(&mut g, 2);
        let t = get(&mut g, "Hero", &["Tally.opened"]);
        assert_eq!(t["Tally.opened"], json!(6), "{t:#}");

        // A value out of a control's range is refused.
        let e = call(
            &mut g,
            "player.act",
            json!({"actions": [{"do": "set", "controls": {"zoom": 3.0}}]}),
        )
        .unwrap_err();
        assert_eq!(e.code, "request.out_of_range", "{e:#?}");

        g.finish_recording().unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap();
        let mut fresh = Game::for_replay(&replay).unwrap();
        let v = amoris_persist::replay::verify(
            &replay,
            &mut fresh,
            amoris_persist::replay::VerifyOptions::mode(amoris_persist::replay::ReplayMode::Verify),
        );
        assert_eq!(
            v.outcome,
            amoris_persist::replay::ReplayOutcome::Identical,
            "{v:#?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    });
}

/// A third-person project's input map is the third-person seat's (docs/spec/input.md 3.1):
/// Thornkeep's loads with its own `interact` and touch buttons over the seat's defaults, and a
/// first-person input is refused at its pointer.
#[test]
fn a_third_person_projects_input_map_is_its_seats() {
    use amoris_runtime::player::SeatKind;
    let thornkeep = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../samples/thornkeep");
    let p = Project::load(&thornkeep).unwrap_or_else(|e| panic!("{e:#?}"));
    let map = p.manifest.player.unwrap().input_map().unwrap();
    assert_eq!(map.seat, SeatKind::ThirdPerson);
    assert_eq!(map.bindings["interact"], ["KeyF"]);
    assert_eq!(map.bindings["dodge"], ["KeyC", "PadEast"]);
    assert!(!map.bindings.contains_key("rise") && !map.bindings.contains_key("slot_1"));
    let touch: Vec<&str> = map.touch.buttons.iter().map(|b| b.input.as_str()).collect();
    assert_eq!(
        touch,
        ["primary", "dodge", "guard", "secondary", "lock", "flask"]
    );

    let root = project("bindings");
    std::fs::write(
        root.join("project.toml"),
        "name = 'third'\nrate = 60\nseed = 1\n[player]\nactions = 'third_person'\n\
         [player.bindings]\nslot_1 = ['Digit1']\n",
    )
    .unwrap();
    let e = Project::load(&root).unwrap_err();
    assert_eq!(e.code, "request.invalid_value");
    assert_eq!(e.detail["path"], "/player/bindings/slot_1");
    std::fs::remove_dir_all(root).unwrap();
}
