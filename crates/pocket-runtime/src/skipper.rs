//! The reference skipper (docs/spec/player.md; shared/benchmark/tasks.md, reference policies): a
//! scripted decision model outside the tick that plays the sailing course through the player tools
//! alone, as an LLM agent would, reading only what `player.observe` answers. It is the course's
//! end-to-end test's player and the player bench's decision model (docs/bench/player.md).
//!
//! At each decision point it reads a JSON observation and decides one act: take aboard a crate it
//! can; else sail to the nearest crate it sees within [`CRATE_RANGE_M`] that does not lie upwind
//! (at most [`CRATE_TRIES`] starts per crate); else sail to the course's next mark (on the chart,
//! so it can be named before it is in sight); else nothing. The caller then waits for the next
//! decision point.

use std::collections::BTreeMap;

use serde_json::{Value, json};

/// How far a crate may be for the skipper to go for it, metres.
pub const CRATE_RANGE_M: f64 = 45.0;
/// How far off the wind's eye a crate must bear to be worth sailing to, degrees.
pub const UPWIND_DEG: f64 = 70.0;
/// Starts per crate before the skipper gives it up.
pub const CRATE_TRIES: u32 = 2;

/// A percept as the skipper reads it from a JSON observation.
struct Seen {
    id: u64,
    kind: String,
    visibility: String,
    range_m: f64,
    bearing_deg: f64,
    can: Vec<String>,
}

fn percepts(obs: &Value) -> Vec<Seen> {
    let Some(list) = obs["entities"].as_array() else {
        return Vec::new();
    };
    list.iter()
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
}

/// An instrument's value in a JSON observation.
pub fn instrument<'a>(obs: &'a Value, name: &str) -> &'a Value {
    obs["instruments"]
        .as_array()
        .and_then(|a| a.iter().find(|r| r["name"] == json!(name)))
        .map_or(&Value::Null, |r| &r["value"])
}

/// The target of the seat's live `sail_to`: its id and the name the seat knows it by.
fn sailing_to(obs: &Value) -> Option<(u64, String)> {
    obs["intents"].as_array()?.iter().find_map(|i| {
        if i["intent"] != json!("sail_to") {
            return None;
        }
        let t = &i["target"];
        Some((
            t["id"].as_u64()?,
            t["name"].as_str().unwrap_or_default().to_owned(),
        ))
    })
}

/// The angle between two bearings, 0 to 180 degrees.
fn off_angle(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    if d > 180.0 { 360.0 - d } else { d }
}

/// The skipper and what it remembers between decisions: how often it set out for each crate.
#[derive(Clone, Debug, Default)]
pub struct Skipper {
    tries: BTreeMap<u64, u32>,
}

impl Skipper {
    pub fn new() -> Skipper {
        Skipper::default()
    }

    /// The act (`player.act` parameters) for this JSON observation, or `None` to let time pass.
    pub fn decide(&mut self, obs: &Value) -> Option<Value> {
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
        let tries = &self.tries;
        let crate_ = seen
            .iter()
            .filter(|p| p.kind == "crate" && p.visibility == "seen" && p.range_m < CRATE_RANGE_M)
            .filter(|p| off_angle(p.bearing_deg, wind_from) > UPWIND_DEG)
            .filter(|p| {
                tries.get(&p.id).copied().unwrap_or(0) < CRATE_TRIES || target_id == Some(p.id)
            })
            .min_by(|a, b| a.range_m.total_cmp(&b.range_m).then(a.id.cmp(&b.id)));
        if let Some(c) = crate_ {
            if target_id == Some(c.id) {
                return None;
            }
            *self.tries.entry(c.id).or_default() += 1;
            return Some(json!({"actions": [{"do": "start", "intent": "sail_to",
                "target": c.id, "params": {"arrive_m": 2}, "tag": "crate"}]}));
        }
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
