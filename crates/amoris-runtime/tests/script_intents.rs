//! A game's own intent in TypeScript end to end (docs/spec/script-host.md 5.5; player.md 5), on
//! `tests/fixtures/intents/errand`: an agent's seat reads the script intent in the game
//! definition, starts it through `player.act` and through an affordance, waits for it to finish,
//! and the rule's `ctx.intent` chains the next errand; a snapshot restored mid-intent, a fork and
//! the recording's replay go on to the same hashes; a restore that derives the player layer again
//! brings the script's intents back into the catalog.
#![cfg(feature = "transpile")]
mod common;

use std::sync::{Arc, OnceLock};

use amoris_interface::action::{ActionCatalog, IntentStatus, IntentTable};
use amoris_link::Source;
use amoris_persist::replay::{MemorySink, RecordOptions, Replay};
use amoris_runtime::{Command, Game, GameSetup, Project};
use serde_json::{Value, json};

/// The errand fixture, compiled once per test binary.
fn errand() -> Arc<GameSetup> {
    static SETUP: OnceLock<Arc<GameSetup>> = OnceLock::new();
    SETUP
        .get_or_init(|| {
            let p = Project::load(&common::repo().join("tests/fixtures/intents/errand"))
                .unwrap_or_else(|e| panic!("{e:#?}"));
            Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")))
        })
        .clone()
}

/// The seat `player`'s commands, as `Source::Player(0)`.
fn call(g: &mut Game, seq: &mut u64, name: &str, params: Value) -> Value {
    *seq += 1;
    g.apply(&Command::new(Source::Player(0), *seq, name, params))
        .unwrap_or_else(|e| panic!("{name}: {e:#?}"))
}

/// Each instance's intent, tag and status, in id order.
fn instances(g: &Game) -> Vec<(String, Option<String>, IntentStatus)> {
    g.world()
        .resource::<IntentTable>()
        .by_id
        .values()
        .map(|i| (i.intent.clone(), i.tag.clone(), i.status))
        .collect()
}

#[test]
fn an_agents_seat_starts_and_completes_a_script_intent() {
    common::big_stack(|| {
        let mut g = Game::new(errand(), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        let mut seq = 0;
        // The game definition shows the script's intent as any other.
        let d = call(
            &mut g,
            &mut seq,
            "player.describe",
            json!({"part": "intents", "name": "walk_to"}),
        );
        let walk = &d["intents"][0];
        assert_eq!(walk["name"], json!("walk_to"), "{d:#}");
        assert_eq!(walk["channels"], json!(["body"]), "{d:#}");
        assert_eq!(
            walk["failures"],
            json!(["errand.stuck", "perception.target_lost", "intent.timeout"]),
            "{d:#}"
        );
        assert!(
            walk["params"]["properties"]["arrive_m"].is_object(),
            "{d:#}"
        );
        // A parameter it does not take is refused, as for an engine intent.
        seq += 1;
        let e = g
            .apply(&Command::new(
                Source::Player(0),
                seq,
                "player.act",
                json!({"actions": [{"do": "start", "intent": "walk_to", "target": "WaypointA",
                                    "params": {"pace": 1}}]}),
            ))
            .unwrap_err();
        assert_eq!(e.code, "request.unknown_field", "{e:#?}");
        let a = call(
            &mut g,
            &mut seq,
            "player.act",
            json!({"actions": [{"do": "start", "intent": "walk_to", "target": "WaypointA",
                                "params": {"arrive_m": 1}, "tag": "first"}]}),
        );
        assert_eq!(a["outcomes"][0]["did"], json!("start"), "{a:#}");
        // The seat waits for its next decision point: the walk's end.
        let w = call(&mut g, &mut seq, "player.wait", json!({}));
        let reasons = w["decision"]["reasons"].to_string();
        assert!(reasons.contains("intent.succeeded"), "{w:#}");
        let mine = call(&mut g, &mut seq, "player.intents", json!({}));
        let first = mine["intents"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["tag"] == json!("first"))
            .unwrap_or_else(|| panic!("{mine:#}"));
        assert_eq!(first["status"], json!("succeeded"), "{mine:#}");
        assert!(
            first["progress"][0]["value"].as_f64().unwrap() <= 1.0,
            "{mine:#}"
        );
        // The press, a tick later, visits A, and the rule's ctx.intent walks on to B and C.
        g.run(400).unwrap();
        assert_eq!(
            instances(&g),
            [
                (
                    "walk_to".into(),
                    Some("first".into()),
                    IntentStatus::Succeeded
                ),
                (
                    "walk_to".into(),
                    Some("chained".into()),
                    IntentStatus::Succeeded
                ),
                (
                    "walk_to".into(),
                    Some("chained".into()),
                    IntentStatus::Succeeded
                ),
            ]
        );
        // An affordance starts it too: the waypoint's `visit`.
        let aff = call(
            &mut g,
            &mut seq,
            "player.affordances",
            json!({"entity": "WaypointA"}),
        );
        assert_eq!(aff["affordances"][0]["verb"], json!("visit"), "{aff:#}");
        assert_eq!(aff["affordances"][0]["available"], json!(true), "{aff:#}");
        call(
            &mut g,
            &mut seq,
            "player.act",
            json!({"actions": [{"do": "use", "entity": "WaypointA", "verb": "visit"}]}),
        );
        g.run(600).unwrap();
        let visits = g
            .apply(&Command::new(
                Source::Developer(0),
                1,
                "world.get",
                json!({"entity": "WaypointA", "components": ["Waypoint"]}),
            ))
            .unwrap();
        assert_eq!(
            visits["components"]["Waypoint"]["visits"],
            json!(2),
            "{visits:#}"
        );
        // The recording replays to the same world, the rule's acts recomputed rather than read.
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
    });
}

/// The hashes of the next `n` ticks.
fn hashes(g: &mut Game, n: u32) -> Vec<String> {
    (0..n)
        .map(|_| {
            g.step().unwrap_or_else(|e| panic!("{e:#?}"));
            format!("{:?}", g.world_hash().unwrap())
        })
        .collect()
}

#[test]
fn a_snapshot_restored_mid_intent_goes_on_identically() {
    common::big_stack(|| {
        let mut a = Game::new(errand(), 1).unwrap();
        let mut seq = 0;
        call(
            &mut a,
            &mut seq,
            "player.act",
            json!({"actions": [{"do": "start", "intent": "walk_to", "target": "WaypointB"}]}),
        );
        a.run(60).unwrap();
        // Mid-walk: the executor's state is in the instance.
        let table = a.world().resource::<IntentTable>().clone();
        let live = table.by_id.values().next().unwrap();
        assert_eq!(live.status, IntentStatus::Active);
        assert!(live.state.get("best").is_some(), "{:?}", live.state);
        let snap = a.snapshot().unwrap();
        let mut fork = a.fork().unwrap();
        let ahead = hashes(&mut a, 300);
        assert_eq!(hashes(&mut fork, 300), ahead);
        let mut b = Game::new(errand(), 1).unwrap();
        b.restore(&snap).unwrap();
        assert_eq!(b.world().resource::<IntentTable>(), &table);
        assert_eq!(hashes(&mut b, 300), ahead);
        assert!(
            instances(&b)
                .iter()
                .all(|(_, _, s)| *s == IntentStatus::Succeeded),
            "{:?}",
            instances(&b)
        );
    });
}

#[test]
fn a_restore_that_derives_the_player_layer_again_keeps_the_scripts_intents() {
    common::big_stack(|| {
        let has_walk = |g: &Game| {
            g.world()
                .get_resource::<ActionCatalog>()
                .is_some_and(|c| c.intent("walk_to").is_some() && c.code("errand.stuck").is_some())
        };
        let mut a = Game::new(errand(), 1).unwrap();
        assert!(has_walk(&a));
        let snap = a.snapshot().unwrap();
        // Another spec (the same game, with another decision filter) derives the catalog again.
        let mut other = (*errand()).clone();
        other.player.as_mut().unwrap().decisions.idle_s = Some(3.0);
        let b = Game::new(Arc::new(other), 1).unwrap();
        a.restore(&b.snapshot().unwrap()).unwrap();
        assert!(has_walk(&a));
        a.restore(&snap).unwrap();
        assert!(has_walk(&a));
    });
}

#[test]
fn a_game_without_players_may_not_declare_intents() {
    common::big_stack(|| {
        let mut bare = (*errand()).clone();
        bare.player = None;
        let e = Game::new(Arc::new(bare), 1).err().expect("refused");
        assert_eq!(e.code, "script.bad_definition", "{e:#?}");
        assert!(e.message.contains("[player]"), "{}", e.message);
    });
}
