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

/// A percept as the agent reads it from a JSON observation.
struct Seen {
    id: u64,
    kind: String,
    visibility: String,
    range_m: f64,
    bearing_deg: f64,
    can: Vec<String>,
}

fn percepts(obs: &Value) -> Vec<Seen> {
    obs["entities"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| Seen {
                    id: p["id"].as_u64().unwrap_or(0),
                    kind: p["kind"].as_str().unwrap_or_default().to_owned(),
                    visibility: p["visibility"].as_str().unwrap_or_default().to_owned(),
                    range_m: p["range_m"].as_f64().unwrap_or(f64::MAX),
                    bearing_deg: p["bearing_deg"].as_f64().unwrap_or(0.0),
                    can: p["can"]
                        .as_array()
                        .map(|c| {
                            c.iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn instrument<'a>(obs: &'a Value, name: &str) -> &'a Value {
    obs["instruments"]
        .as_array()
        .and_then(|a| a.iter().find(|r| r["name"] == json!(name)))
        .map_or(&Value::Null, |r| &r["value"])
}

/// The target of the seat's live `sail_to`: its id and the name the seat knows it by.
fn sailing_to(obs: &Value) -> Option<(u64, String)> {
    obs["intents"].as_array()?.iter().find_map(|i| {
        (i["intent"] == json!("sail_to")).then(|| {
            let t = &i["target"];
            Some((
                t["id"].as_u64()?,
                t["name"].as_str().unwrap_or_default().to_owned(),
            ))
        })?
    })
}

fn off_angle(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    if d > 180.0 { 360.0 - d } else { d }
}

/// The scripted agent: a decision model outside the tick that plays through the player tools
/// only. At each decision it observes, then: takes aboard a crate it can; else sails to the
/// nearest crate it sees within 45 m that is not upwind (twice at most per crate); else sails to
/// the course's next mark; then waits for its next decision point.
#[derive(Default)]
struct Agent {
    tries: std::collections::BTreeMap<u64, u32>,
    decisions: u32,
    acts: u32,
    observed_bytes: Vec<usize>,
}

impl Agent {
    fn decide(&mut self, obs: &Value) -> Option<Value> {
        let seen = percepts(obs);
        if let Some(c) = seen
            .iter()
            .find(|p| p.kind == "crate" && p.can.iter().any(|v| v == "take_aboard"))
        {
            return Some(
                json!({"actions": [{"do": "use", "entity": c.id, "verb": "take_aboard"}]}),
            );
        }
        let wind_from = instrument(obs, "wind_from_deg").as_f64().unwrap_or(270.0);
        let target = sailing_to(obs);
        let target_id = target.as_ref().map(|t| t.0);
        let crate_ = seen
            .iter()
            .filter(|p| p.kind == "crate" && p.visibility == "seen" && p.range_m < 45.0)
            .filter(|p| off_angle(p.bearing_deg, wind_from) > 70.0)
            .filter(|p| self.tries.get(&p.id).copied().unwrap_or(0) < 2 || target_id == Some(p.id))
            .min_by(|a, b| a.range_m.total_cmp(&b.range_m));
        if let Some(c) = crate_ {
            if target_id == Some(c.id) {
                return None;
            }
            *self.tries.entry(c.id).or_default() += 1;
            return Some(json!({"actions": [{"do": "start", "intent": "sail_to",
                "target": c.id, "params": {"arrive_m": 2}, "tag": "crate"}]}));
        }
        // The next mark is on the chart, so it can be named before it is in sight.
        let next = instrument(obs, "next_mark").as_str().unwrap_or("none");
        if next == "none" || target.as_ref().is_some_and(|t| t.1 == next) {
            return None;
        }
        Some(
            json!({"actions": [{"do": "start", "intent": "sail_to", "target": next,
            "params": {"arrive_m": 8}, "tag": "mark"}]}),
        )
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
