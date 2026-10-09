//! The player layer end to end on the sailing course (docs/spec/player.md; charter 2, proof target
//! 2): the course declares its seat; a player reads the game definition and observes through its
//! own perception only; a scripted agent plays the whole course through `player.*` commands alone
//! (observe, act, wait, describe), and the recording of that run replays to the same world hash at
//! every tick.
#![cfg(feature = "transpile")]
mod common;

use std::sync::{Arc, OnceLock};

use pocket_link::Source;
use pocket_persist::replay::{MemorySink, RecordOptions, Replay};
use pocket_runtime::skipper::Skipper;
use pocket_runtime::{Command, Game, GameSetup, Project};
use serde_json::{Value, json};

/// The sailing course, compiled once per test binary.
fn course() -> Arc<GameSetup> {
    static SETUP: OnceLock<Arc<GameSetup>> = OnceLock::new();
    SETUP
        .get_or_init(|| {
            let p = Project::load(&common::repo().join("samples/sailing-course"))
                .unwrap_or_else(|e| panic!("{e:#?}"));
            Arc::new(p.setup(false).unwrap_or_else(|e| panic!("{e:#?}")))
        })
        .clone()
}

/// A player at the skipper's seat: its commands as `Source::Player(0)`.
struct Player<'g> {
    game: &'g mut Game,
    seq: u64,
}

impl Player<'_> {
    fn call(&mut self, name: &str, params: Value) -> Result<Value, pocket_contract::Problem> {
        self.seq += 1;
        self.game
            .apply(&Command::new(Source::Player(0), self.seq, name, params))
    }

    fn ok(&mut self, name: &str, params: Value) -> Value {
        self.call(name, params)
            .unwrap_or_else(|e| panic!("{name}: {e:#?}"))
    }
}

#[test]
fn a_player_is_its_seat_and_sees_through_its_own_perception() {
    common::big_stack(|| {
        let mut g = Game::new(course(), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        let mut p = Player {
            game: &mut g,
            seq: 0,
        };
        let s = p.ok("player.session", json!({}));
        assert_eq!(s["role"], json!("player"), "{s:#}");
        assert_eq!(s["seat"], json!("skipper"), "{s:#}");
        assert_eq!(s["index"], json!(0), "{s:#}");
        assert_eq!(
            s["decision"]["reasons"][0]["reason"],
            json!("start"),
            "{s:#}"
        );
        let d = p.ok("player.describe", json!({}));
        assert_eq!(d["contract"], json!("0.1"), "{d:#}");
        // The whole definition is beyond the default 2000 tokens: names only, and the call for
        // the rest (README, The game definition).
        assert_eq!(d["seats"], json!(["skipper"]), "{d:#}");
        assert!(d["more"].is_string(), "{d:#}");
        assert!(d["tokens"].as_u64().unwrap() <= 2000, "{d:#}");
        let seats = p.ok("player.describe", json!({"part": "seats"}));
        assert_eq!(seats["seats"][0]["observer"], json!("skipper"), "{seats:#}");
        assert_eq!(
            seats["seats"][0]["intents"],
            json!(["come_to_heading", "trim_sail", "sail_to"]),
            "{seats:#}"
        );
        // At tick 0 nothing has been seen: the charted marks and the Isle are known from the chart.
        let o = p.ok("player.observe", json!({"projection": "text"}));
        let text = o.as_str().unwrap();
        assert!(text.starts_with("tick 0 "), "{text}");
        assert!(text.contains("chart Mark1#5 mark"), "{text}");
        p.ok("player.wait", json!({"ticks": 1}));
        // After the first tick: Mark1 and Mark2 are in sight, Mark3 lies behind the Isle and shows
        // only on the chart, and Crate4 behind it is unknown altogether.
        let near = p.ok("player.nearby", json!({"budget_tokens": 4000}));
        let vis = |name: &str| {
            near["entities"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["name"] == json!(name))
                .map(|e| e["visibility"].clone())
        };
        assert_eq!(vis("Mark1"), Some(json!("seen")), "{near:#}");
        assert_eq!(vis("Mark2"), Some(json!("seen")), "{near:#}");
        assert_eq!(vis("Mark3"), Some(json!("charted")), "{near:#}");
        assert_eq!(vis("Crate1"), Some(json!("seen")), "{near:#}");
        assert_eq!(vis("Crate4"), None, "{near:#}");
        let e = p
            .call("player.describe", json!({"entity": "Crate4"}))
            .unwrap_err();
        assert_eq!(e.code, "perception.unknown_entity", "{e:#?}");
        // An RL policy reads the same perception as tensors (projection.md, Tensor projection).
        let t = p.ok("player.observe", json!({"projection": "tensor"}));
        let t: Value = serde_json::from_str(t.as_str().unwrap()).unwrap();
        assert!(t.to_string().contains("rays"), "{t}");
        // The omniscient view is not a player's, nor another seat.
        let e = p
            .call("player.observe", json!({"omniscient": true}))
            .unwrap_err();
        assert_eq!(e.code, "perception.omniscient_forbidden", "{e:#?}");
        let e = p
            .call("player.observe", json!({"seat": "lookout"}))
            .unwrap_err();
        assert_eq!(e.code, "seat.not_yours", "{e:#?}");
        // A misspelt field is refused with the right suggestion.
        let e = p
            .call("player.wait", json!({"untill": "decision"}))
            .unwrap_err();
        assert_eq!(e.code, "request.unknown_field", "{e:#?}");
        assert!(format!("{e:?}").contains("until"), "{e:#?}");
        // A developer sees the marked omniscient view and steps no seat by itself.
        let dev = g
            .apply(&Command::new(
                Source::Developer(0),
                1,
                "player.observe",
                json!({"omniscient": true, "budget_tokens": 200}),
            ))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        assert_eq!(dev["omniscient"], json!(true), "{dev:#}");
    });
}

/// `player.act` validates the whole call before anything of it applies (actions.md, Validation
/// and atomicity): one bad action refuses the call and the world hash is unchanged.
#[test]
fn a_call_with_one_bad_action_applies_nothing() {
    common::big_stack(|| {
        let mut g = Game::new(course(), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        g.run(2).unwrap();
        let before = g.world_hash().unwrap();
        let mut p = Player {
            game: &mut g,
            seq: 0,
        };
        let e = p
            .call(
                "player.act",
                json!({"actions": [
                    {"do": "set", "controls": {"hoist": 1}},
                    {"do": "start", "intent": "sail_too", "target": "Mark1"}
                ]}),
            )
            .unwrap_err();
        assert_eq!(e.code, "action.unknown_intent", "{e:#?}");
        // A crate 40 m away cannot be taken aboard: refused with every unmet requirement.
        let e = p
            .call(
                "player.act",
                json!({"actions": [{"do": "use", "entity": "Crate1", "verb": "take_aboard"}]}),
            )
            .unwrap_err();
        assert_eq!(e.code, "action.unavailable", "{e:#?}");
        assert_eq!(g.world_hash().unwrap(), before);
    });
}

/// The scripted agent: the reference skipper (`pocket_runtime::skipper`), a decision model outside
/// the tick that plays through the player tools only, and what the test counts of its run.
#[derive(Default)]
struct Agent {
    skipper: Skipper,
    decisions: u32,
    acts: u32,
    observed_bytes: Vec<usize>,
}

impl Agent {
    fn decide(&mut self, obs: &Value) -> Option<Value> {
        self.skipper.decide(obs)
    }
}

/// Charter 2, proof target 2, on this line: the scripted agent sails the whole course through the
/// player tools alone, and the recording of the run replays to the same world hash at every tick.
#[test]
fn a_scripted_agent_sails_the_course_through_player_tools_and_the_run_replays() {
    common::big_stack(|| {
        let mut g = Game::new(course(), 1).unwrap_or_else(|e| panic!("{e:#?}"));
        let sink = MemorySink::new();
        g.record(Box::new(sink.clone()), RecordOptions::default())
            .unwrap();
        let mut agent = Agent::default();
        let mut p = Player {
            game: &mut g,
            seq: 0,
        };
        let def = p.ok(
            "player.describe",
            json!({"part": "intents", "name": "sail_to"}),
        );
        assert_eq!(def["intents"][0]["name"], json!("sail_to"), "{def:#}");
        let mut outcome = Value::Null;
        for _ in 0..200 {
            let text = p.ok("player.observe", json!({"projection": "text"}));
            agent.observed_bytes.push(text.as_str().map_or(0, str::len));
            let obs = p.ok("player.observe", json!({"budget_tokens": 1500}));
            agent.decisions += 1;
            if let Some(act) = agent.decide(&obs) {
                let r = p.ok("player.act", act.clone());
                agent.acts += 1;
                println!("tick {} act {} -> {}", obs["tick"], act, r["outcomes"]);
            }
            let w = p.ok("player.wait", json!({}));
            println!(
                "  wait -> tick {} stopped {} decision {}",
                w["tick"], w["stopped"], w["decision"]["reasons"]
            );
            if w["stopped"] == json!("done") {
                outcome = w["outcome"].clone();
                break;
            }
        }
        println!(
            "{} decisions, {} acts, observation text bytes {:?}, outcome {outcome}",
            agent.decisions, agent.acts, agent.observed_bytes
        );
        assert_eq!(outcome["terminated"], json!(true), "{outcome:#}");
        assert_eq!(outcome["result"][1]["value"], json!(3), "{outcome:#}");
        let session = p.ok("player.session", json!({}));
        assert_eq!(
            session["stats"]["decisions_answered"],
            json!(agent.decisions),
            "{session:#}"
        );
        let e = p
            .call(
                "player.act",
                json!({"actions": [{"do": "set", "controls": {"rudder": 0.1}}]}),
            )
            .unwrap_err();
        assert_eq!(e.code, "time.episode_over", "{e:#?}");
        // The crates the agent saw on its way came aboard (Crate4 hid behind the Isle).
        let tally = g
            .apply(&Command::new(
                Source::Developer(0),
                1,
                "world.get",
                json!({"entity": "Sloop", "components": ["Tally"]}),
            ))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        assert!(
            tally["components"]["Tally"]["taken"].as_u64().unwrap() >= 2,
            "{tally:#}"
        );
        g.finish_recording().unwrap();
        let replay = Replay::read(&sink.bytes()).unwrap();
        let mut fresh = Game::for_replay(&replay).unwrap();
        let v = pocket_persist::replay::verify(
            &replay,
            &mut fresh,
            pocket_persist::replay::VerifyOptions::mode(pocket_persist::replay::ReplayMode::Verify),
        );
        assert_eq!(
            v.outcome,
            pocket_persist::replay::ReplayOutcome::Identical,
            "{v:#?}"
        );
    });
}
